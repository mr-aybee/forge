//! The agent's analytics side (D-142): the spool, the local log listener,
//! the uploader, and the rows the agent writes itself (node minutes, top
//! processes, container output and samples). Heartbeats carry none of this
//! data any more; they report the spool's state and bring the settings.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, RwLock};

use forge_core::analytics::{MinuteBucket, NodeSample};
use forge_core::spool::{Row, TopProcessRow};
use forge_proto::messages::{AnalyticsAgentConfig, NodeMetrics, SpoolStatus};
use rustls::ClientConfig;
use tokio::sync::watch;

use crate::container_telemetry::{TelemetryResponse, rows};
use crate::log_listener::{self, Gate};
use crate::spool::{FreeDisk, Limits, Spool, SpoolError, SystemFreeDisk};
use crate::spool_upload::{self, MakeSink, Target, WorkerSink};

/// Wall clock, epoch ms.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The running analytics side of one agent.
#[derive(Debug)]
pub struct NodeAnalytics {
    spool: Arc<Spool>,
    gate: Arc<Gate>,
    target: watch::Sender<Option<Target>>,
    addrs: watch::Sender<Vec<SocketAddr>>,
    tls: Arc<RwLock<Arc<ClientConfig>>>,
    gateway: Option<IpAddr>,
    extra: Option<IpAddr>,
    bucket: Option<MinuteBucket>,
    /// The server sent analytics settings: data is written to the spool.
    enabled: bool,
    started: bool,
    stop: watch::Sender<bool>,
}

impl Drop for NodeAnalytics {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

impl NodeAnalytics {
    /// Opens the spool in `dir` (recovering a previous run). `url` is the
    /// worker from enrollment, if known.
    pub fn open(dir: &Path, node: uuid::Uuid, url: Option<String>, tls: Arc<ClientConfig>) -> std::io::Result<Self> {
        Self::open_with(dir, node, url, tls, Arc::new(SystemFreeDisk))
    }

    /// [`open`](Self::open) with a given view of free disk (tests).
    pub fn open_with(
        dir: &Path,
        node: uuid::Uuid,
        url: Option<String>,
        tls: Arc<ClientConfig>,
        free: Arc<dyn FreeDisk>,
    ) -> std::io::Result<Self> {
        let (spool, _) = Spool::open(dir, node, Limits::default(), free, now_ms())?;
        Ok(Self {
            spool: Arc::new(spool),
            gate: Arc::new(Gate::default()),
            target: watch::channel(url.filter(|u| !u.is_empty()).map(|url| Target { url, generation: 0 })).0,
            addrs: watch::channel(log_listener::addresses(None, None)).0,
            tls: Arc::new(RwLock::new(tls)),
            gateway: None,
            extra: None,
            bucket: None,
            enabled: false,
            started: false,
            stop: watch::channel(false).0,
        })
    }

    #[must_use]
    pub fn spool(&self) -> &Arc<Spool> {
        &self.spool
    }

    /// Whether the server sent analytics settings.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Starts the seal timer, the uploader and the listener (inside the
    /// runtime, once).
    pub fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let tls = self.tls.clone();
        let make: MakeSink = Box::new(move |t: &Target| {
            let tls = tls.read().map_err(|_| anyhow::anyhow!("tls lock"))?.clone();
            Ok(Box::new(WorkerSink::new(&t.url, tls)?) as Box<dyn spool_upload::Sink>)
        });
        tokio::spawn(spool_upload::run(self.spool.clone(), self.target.subscribe(), make, self.stop.subscribe()));
        tokio::spawn(log_listener::run(
            self.spool.clone(),
            self.gate.clone(),
            self.addrs.subscribe(),
            self.stop.subscribe(),
        ));
        tokio::spawn(tick(self.spool.clone(), self.stop.subscribe()));
    }

    /// The agent's certificate was renewed: uploads use the new one.
    pub fn set_tls(&self, tls: Arc<ClientConfig>) {
        if let Ok(mut t) = self.tls.write() {
            *t = tls;
        }
        self.target.send_modify(|t| {
            if let Some(t) = t {
                t.generation += 1;
            }
        });
    }

    /// Applies the settings of a heartbeat response; `None` turns the
    /// spooling of node data off (the listener stays, with no keys).
    pub fn apply(&mut self, cfg: Option<&AnalyticsAgentConfig>) {
        let Some(cfg) = cfg else {
            self.enabled = false;
            self.gate.set_keys(&[]);
            return;
        };
        self.enabled = true;
        let url = Some(cfg.url.clone()).filter(|u| !u.is_empty());
        self.target.send_if_modified(|t| {
            let same = t.as_ref().map(|t| &t.url) == url.as_ref();
            if !same {
                *t = url.map(|url| Target { url, generation: 0 });
            }
            !same
        });
        if let Some(s) = &cfg.spool {
            self.spool.set_limits(Limits::from_tuning(s));
        }
        self.gate.set_keys(&cfg.keys);
        let listener = cfg.listener.clone().unwrap_or_default();
        self.gate.set_limits(log_listener::Limits::from_tuning(&listener));
        self.extra = listener.extra_address.parse().ok();
        self.update_addresses();
    }

    fn update_addresses(&self) {
        let want = log_listener::addresses(self.gateway, self.extra);
        self.addrs.send_if_modified(|a| {
            let changed = *a != want;
            if changed {
                a.clone_from(&want);
            }
            changed
        });
    }

    /// Folds a heartbeat's sample into the open minute and writes the
    /// closed minute and the top processes to the spool.
    pub async fn record_node(&mut self, m: &NodeMetrics, now: i64) {
        if !self.enabled {
            return;
        }
        let minute = MinuteBucket::minute_of(now);
        let mut rows = Vec::new();
        match &mut self.bucket {
            Some(b) if b.minute() == minute => {}
            other => {
                if let Some(b) = other.take() {
                    rows.push(Row::NodeMinute(b.close()));
                }
                *other = Some(MinuteBucket::new(minute));
            }
        }
        if let Some(b) = &mut self.bucket {
            b.add(&NodeSample {
                cpu: m.cpu_percent,
                mem_used: m.mem_used_bytes,
                mem_total: m.mem_total_bytes,
                swap_used: m.swap_used_bytes,
                disk_used: m.disk_used_bytes,
                disk_total: m.disk_total_bytes,
                rx: m.net_rx_bytes_per_sec,
                tx: m.net_tx_bytes_per_sec,
                load1: m.load1,
            });
        }
        rows.extend(m.top_processes.iter().map(|p| {
            Row::TopProcess(TopProcessRow {
                minute,
                pid: p.pid,
                name: p.name.clone(),
                user: p.user.clone(),
                cpu: p.cpu_percent,
                mem: p.mem_bytes,
                cmd: p.cmd.clone(),
            })
        }));
        if let Err(e) = self.append(rows, now, false).await {
            tracing::debug!(error = %e, "node minute not spooled");
        }
    }

    /// Writes the helper's container lines and samples and seals them to
    /// disk. Returns the batch `seq` to acknowledge to the helper: only
    /// lines that are on disk are acknowledged, so the helper sends the
    /// rest again.
    pub async fn record_containers(&mut self, t: &TelemetryResponse, now: i64) -> u64 {
        if t.bridge_gateway != self.gateway {
            self.gateway = t.bridge_gateway;
            self.update_addresses();
        }
        if !self.enabled {
            return 0;
        }
        let minute = MinuteBucket::minute_of(now);
        let mut rows: Vec<Row> = rows::samples(&t.samples, minute).collect();
        let Some(batch) = &t.batch else {
            if let Err(e) = self.append(rows, now, false).await {
                tracing::debug!(error = %e, "container samples not spooled");
            }
            return 0;
        };
        if batch.dropped > 0 {
            tracing::warn!(dropped = batch.dropped, "the helper dropped container log lines (buffer full)");
        }
        rows.extend(rows::lines(batch));
        match self.append(rows, now, true).await {
            Ok(()) => batch.seq,
            Err(e) => {
                tracing::warn!(error = %e, "container lines not spooled; the helper keeps them");
                0
            }
        }
    }

    async fn append(&self, rows: Vec<Row>, now: i64, seal: bool) -> Result<(), SpoolError> {
        if rows.is_empty() && !seal {
            return Ok(());
        }
        let spool = self.spool.clone();
        tokio::task::spawn_blocking(move || {
            for r in &rows {
                spool.append(r, now)?;
            }
            if seal { spool.seal(now) } else { Ok(()) }
        })
        .await
        .map_err(|e| SpoolError::Io(e.to_string()))?
    }

    /// The heartbeat's report.
    #[must_use]
    pub fn status(&self) -> SpoolStatus {
        self.spool.status(now_ms())
    }
}

/// Seals the buffer by age and expires old files, once a second.
async fn tick(spool: Arc<Spool>, mut stop: watch::Receiver<bool>) {
    let mut every = tokio::time::interval(std::time::Duration::from_secs(1));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = every.tick() => {}
            _ = stop.changed() => return,
        }
        let s = spool.clone();
        match tokio::task::spawn_blocking(move || s.tick(now_ms())).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "sealing the spool failed"),
            Err(e) => tracing::warn!(error = %e, "spool timer"),
        }
    }
}
