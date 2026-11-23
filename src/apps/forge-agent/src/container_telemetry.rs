//! Logs and metrics of every Docker container on the node.
//!
//! Only the root helper touches the Docker socket. While collection is on,
//! its [`Collector`] watches `/events` and follows the output of every
//! running container (demultiplexed stdout and stderr, with Docker's
//! timestamps) into a bounded buffer that drops the oldest lines and counts
//! them. Once a minute it samples each container's resources.
//!
//! The agent polls the helper with the closed `container_telemetry` request
//! before each heartbeat: the request carries the node settings (on/off and
//! the exclude list, from the server) and acknowledges the batch the server
//! stored. The helper keeps one batch pending until it is acknowledged and
//! sends it again otherwise. Only an acknowledged batch moves the
//! per-container cursor, which the helper keeps in its root-only state
//! folder: after a restart of the helper, the agent or the server, following
//! resumes after the last stored line, so nothing is dropped and nothing
//! already stored is sent again (except a batch whose acknowledgement was
//! lost on the way back).
//!
//! The request is allowed without a signed command: it only reads, and what
//! it reads goes to Forge through the agent anyway. Turning collection off
//! or excluding containers only collects less.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};

/// Most lines buffered in the helper.
pub const MAX_BUFFER_LINES: usize = 20_000;
/// Most bytes of lines buffered in the helper.
pub const MAX_BUFFER_BYTES: usize = 16 * 1024 * 1024;
/// Most containers with a stored cursor.
pub const MAX_CURSORS: usize = 4_096;
/// Longest partial line held while waiting for its newline.
const MAX_PARTIAL: usize = forge_proto::messages::MAX_CONTAINER_LINE_BYTES;

/// The agent's poll: node settings plus the batch to acknowledge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryRequest {
    pub enabled: bool,
    /// A container name, or `key=value` for a label.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// The `seq` the server stored; 0 for none.
    #[serde(default)]
    pub ack: u64,
}

impl TelemetryRequest {
    pub fn validate(&self) -> Result<(), String> {
        use forge_proto::messages::{MAX_CONTAINER_EXCLUDES, MAX_CONTAINER_FIELD_BYTES};
        if self.exclude.len() > MAX_CONTAINER_EXCLUDES {
            return Err(format!("at most {MAX_CONTAINER_EXCLUDES} exclude entries"));
        }
        if self.exclude.iter().any(|e| e.is_empty() || e.len() > MAX_CONTAINER_FIELD_BYTES) {
            return Err("an exclude entry is 1 to 256 bytes".into());
        }
        Ok(())
    }
}

/// Output stream of a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// The labels Forge attributes a container by.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Labels {
    #[serde(default)]
    pub forge_app: String,
    #[serde(default)]
    pub forge_stack: String,
    #[serde(default)]
    pub compose_project: String,
    #[serde(default)]
    pub compose_service: String,
}

impl Labels {
    /// The four labels out of a container's label map.
    #[must_use]
    pub fn from_map(m: &HashMap<String, String>) -> Self {
        let get = |k: &str| m.get(k).cloned().unwrap_or_default();
        Self {
            forge_app: get("forge.app"),
            forge_stack: get(forge_catalog::stacks::STACK_LABEL),
            compose_project: get("com.docker.compose.project"),
            compose_service: get("com.docker.compose.service"),
        }
    }
}

/// What a line or sample says about its container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// Short (12 hex digits) id.
    pub id: String,
    pub name: String,
    pub image: String,
    pub labels: Labels,
}

/// One collected line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub container: Meta,
    pub stream: Stream,
    /// Docker's timestamp, epoch nanoseconds.
    pub ts_ns: i64,
    pub line: String,
}

/// Lines handed to the agent; sent again until acknowledged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub seq: u64,
    pub lines: Vec<Line>,
    /// Lines dropped since the previous batch (buffer full).
    pub dropped: u64,
}

/// One container's resources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub container: Meta,
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub rx_per_sec: u64,
    pub tx_per_sec: u64,
    pub read_per_sec: u64,
    pub write_per_sec: u64,
    pub restarts: u32,
}

/// The helper's answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TelemetryResponse {
    pub batch: Option<Batch>,
    /// The latest per-minute samples, once.
    #[serde(default)]
    pub samples: Vec<Sample>,
    /// The Docker bridge network's gateway address (`docker0`), where the
    /// agent's log listener also binds for containers (D-142). Absent
    /// without Docker or from an older helper.
    #[serde(default)]
    pub bridge_gateway: Option<std::net::IpAddr>,
}

/// Whether the exclude list leaves a container out: an entry `key=value`
/// matches a label, any other entry the container's name.
#[must_use]
pub fn excluded(name: &str, labels: &HashMap<String, String>, exclude: &[String]) -> bool {
    exclude.iter().any(|e| match e.split_once('=') {
        Some((k, v)) => labels.get(k).is_some_and(|l| l == v),
        None => e == name,
    })
}

/// Epoch nanoseconds of Docker's RFC 3339 timestamp
/// (`2024-05-01T10:00:00.123456789Z`).
#[must_use]
pub fn parse_ts_ns(s: &str) -> Option<i64> {
    let (secs, frac) = match s.split_once('.') {
        Some((a, b)) => (a.to_owned() + "Z", b.trim_end_matches('Z')),
        None => (s.to_owned(), ""),
    };
    if !frac.bytes().all(|b| b.is_ascii_digit()) || frac.len() > 9 {
        return None;
    }
    let ms = forge_core::logs::parse_rfc3339_ms(&secs)?;
    let nanos: i64 = if frac.is_empty() { 0 } else { format!("{frac:0<9}").parse().ok()? };
    ms.checked_mul(1_000_000)?.checked_add(nanos)
}

/// A line of `logs?timestamps=1`: Docker's timestamp, a space, the text.
#[must_use]
pub fn split_timestamp(raw: &[u8]) -> Option<(i64, String)> {
    let space = raw.iter().position(|b| *b == b' ')?;
    let ts = parse_ts_ns(std::str::from_utf8(&raw[..space]).ok()?)?;
    let mut text = &raw[space + 1..];
    if text.last() == Some(&b'\r') {
        text = &text[..text.len() - 1];
    }
    Some((ts, String::from_utf8_lossy(text).into_owned()))
}

/// Splits Docker's log stream into whole lines as chunks arrive: the
/// 8-byte frame headers of a non-TTY container (stream, 0, 0, 0, length),
/// or the raw bytes of a TTY container (all stdout).
#[derive(Debug, Default)]
pub struct Demux {
    tty: bool,
    /// Bytes of an incomplete frame header or body.
    pending: Vec<u8>,
    /// The incomplete last line of stdout and of stderr.
    partial: [Vec<u8>; 2],
}

impl Demux {
    #[must_use]
    pub fn new(tty: bool) -> Self {
        Self { tty, ..Self::default() }
    }

    /// Feeds a chunk; returns the lines it completed (without newline).
    pub fn push(&mut self, chunk: &[u8]) -> Vec<(Stream, Vec<u8>)> {
        let mut out = Vec::new();
        if self.tty {
            self.text(Stream::Stdout, chunk, &mut out);
            return out;
        }
        self.pending.extend_from_slice(chunk);
        let mut i = 0;
        while self.pending.len() - i >= 8 {
            let h = &self.pending[i..i + 8];
            let len = u32::from_be_bytes([h[4], h[5], h[6], h[7]]) as usize;
            if self.pending.len() - i - 8 < len {
                break;
            }
            let stream = if h[0] == 2 { Stream::Stderr } else { Stream::Stdout };
            let body = self.pending[i + 8..i + 8 + len].to_vec();
            self.text(stream, &body, &mut out);
            i += 8 + len;
        }
        self.pending.drain(..i);
        out
    }

    fn text(&mut self, stream: Stream, mut data: &[u8], out: &mut Vec<(Stream, Vec<u8>)>) {
        let part = &mut self.partial[usize::from(stream == Stream::Stderr)];
        while let Some(nl) = data.iter().position(|b| *b == b'\n') {
            part.extend_from_slice(&data[..nl]);
            out.push((stream, std::mem::take(part)));
            data = &data[nl + 1..];
        }
        part.extend_from_slice(data);
        // A line that never ends is cut, so memory stays bounded.
        if part.len() > MAX_PARTIAL {
            out.push((stream, std::mem::take(part)));
        }
    }
}

/// Committed cursors: per container id, the timestamp (ns) of the last line
/// the server stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursors(pub BTreeMap<String, i64>);

impl Cursors {
    /// Moves `id`'s cursor forward to `ts` (never back).
    pub fn advance(&mut self, id: &str, ts: i64) {
        let e = self.0.entry(id.to_owned()).or_insert(ts);
        *e = (*e).max(ts);
        // Bounded: the oldest positions go first.
        while self.0.len() > MAX_CURSORS {
            let Some(oldest) = self.0.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone()) else { break };
            self.0.remove(&oldest);
        }
    }
}

/// The buffer, the pending batch and the cursors.
#[derive(Debug, Default)]
pub struct Buffer {
    lines: VecDeque<Line>,
    bytes: usize,
    dropped: u64,
    seq: u64,
    pending: Option<Batch>,
    /// What the server stored.
    committed: Cursors,
    /// The newest line read per container in this run (buffered or not).
    read: HashMap<String, i64>,
}

impl Buffer {
    #[must_use]
    pub fn new(committed: Cursors) -> Self {
        Self { committed, ..Self::default() }
    }

    #[must_use]
    pub fn committed(&self) -> &Cursors {
        &self.committed
    }

    /// Where following `id` resumes: after the newest line read or stored.
    #[must_use]
    pub fn resume_after(&self, id: &str) -> Option<i64> {
        match (self.committed.0.get(id), self.read.get(id)) {
            (Some(a), Some(b)) => Some(*a.max(b)),
            (a, b) => a.or(b).copied(),
        }
    }

    /// Adds a line unless it is at or before `id`'s resume point (already
    /// read or stored); drops the oldest lines when full. Returns whether
    /// it was taken.
    pub fn push(&mut self, line: Line) -> bool {
        if self.resume_after(&line.container.id).is_some_and(|t| line.ts_ns <= t) {
            return false;
        }
        self.read.insert(line.container.id.clone(), line.ts_ns);
        self.bytes += line.line.len();
        self.lines.push_back(line);
        while self.lines.len() > MAX_BUFFER_LINES || self.bytes > MAX_BUFFER_BYTES {
            let Some(old) = self.lines.pop_front() else { break };
            self.bytes -= old.line.len();
            self.dropped += 1;
        }
        true
    }

    /// Applies `ack` and returns the batch to send: the pending one again,
    /// or a new one of at most `max` lines. Returns whether the committed
    /// cursors changed (to be saved).
    pub fn take(&mut self, ack: u64, max: usize) -> (Option<Batch>, bool) {
        let mut changed = false;
        if ack != 0
            && let Some(p) = self.pending.take_if(|p| p.seq == ack)
        {
            for l in &p.lines {
                self.committed.advance(&l.container.id, l.ts_ns);
            }
            changed = !p.lines.is_empty();
        }
        if self.pending.is_none() && (!self.lines.is_empty() || self.dropped > 0) {
            let n = self.lines.len().min(max);
            let lines: Vec<Line> = self.lines.drain(..n).collect();
            self.bytes -= lines.iter().map(|l| l.line.len()).sum::<usize>();
            self.seq += 1;
            self.pending = Some(Batch { seq: self.seq, lines, dropped: std::mem::take(&mut self.dropped) });
        }
        (self.pending.clone(), changed)
    }

    /// Drops what is buffered and pending (collection turned off).
    pub fn clear(&mut self) {
        self.lines.clear();
        self.bytes = 0;
        self.dropped = 0;
        self.pending = None;
        self.read.clear();
    }

    /// Forgets a removed container.
    pub fn forget(&mut self, id: &str) -> bool {
        self.read.remove(id);
        self.committed.0.remove(id).is_some()
    }
}

#[cfg(unix)]
pub use collector::Collector;

#[cfg(unix)]
mod collector {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use http_body_util::BodyExt;
    use serde::Deserialize;
    use serde_json::Value;
    use tokio::task::JoinSet;

    use super::{
        Buffer, Cursors, Demux, Line, Meta, Sample, TelemetryRequest, TelemetryResponse, excluded, split_timestamp,
    };

    const CURSOR_FILE: &str = "container-cursors.json";
    const MAX_CURSOR_FILE: u64 = 1024 * 1024;
    const STATS_EVERY: Duration = Duration::from_secs(60);
    const RETRY: Duration = Duration::from_secs(5);
    const MAX_RETRY: Duration = Duration::from_secs(120);
    const QUICK: Duration = Duration::from_secs(10);
    /// Longest `/events` record held while waiting for its newline.
    const MAX_EVENT: usize = 64 * 1024;

    #[derive(Debug, Deserialize)]
    struct Listed {
        #[serde(rename = "Id")]
        id: String,
    }

    #[derive(Debug, Deserialize)]
    struct Inspect {
        #[serde(rename = "Id")]
        id: String,
        #[serde(rename = "Name", default)]
        name: String,
        #[serde(rename = "Config")]
        config: InspectConfig,
        #[serde(rename = "RestartCount", default)]
        restarts: u32,
    }

    #[derive(Debug, Deserialize)]
    struct InspectConfig {
        #[serde(rename = "Image", default)]
        image: String,
        #[serde(rename = "Labels", default)]
        labels: Option<HashMap<String, String>>,
        #[serde(rename = "Tty", default)]
        tty: bool,
    }

    #[derive(Debug, Deserialize)]
    struct Event {
        #[serde(rename = "Action", default)]
        action: String,
        #[serde(rename = "Actor")]
        actor: Actor,
    }

    #[derive(Debug, Deserialize)]
    struct Actor {
        #[serde(rename = "ID", default)]
        id: String,
    }

    fn short(id: &str) -> String {
        id.chars().take(12).collect()
    }

    impl Inspect {
        fn meta(&self) -> Meta {
            let labels = self.config.labels.clone().unwrap_or_default();
            Meta {
                id: short(&self.id),
                name: self.name.trim_start_matches('/').to_owned(),
                image: self.config.image.clone(),
                labels: super::Labels::from_map(&labels),
            }
        }

        fn excluded(&self, exclude: &[String]) -> bool {
            let labels = self.config.labels.clone().unwrap_or_default();
            excluded(self.name.trim_start_matches('/'), &labels, exclude)
        }
    }

    /// Counters of the previous sample, for rates.
    #[derive(Debug, Clone, Copy)]
    struct Counters {
        at: Instant,
        rx: u64,
        tx: u64,
        read: u64,
        write: u64,
    }

    #[derive(Debug, Default)]
    struct Shared {
        buffer: Buffer,
        samples: Vec<Sample>,
        counters: HashMap<String, Counters>,
    }

    /// The collection task and the settings it runs with.
    type Running = (TelemetryRequest, tokio::task::JoinHandle<()>);

    /// The helper's collector. Cheap to share.
    #[derive(Debug, Clone)]
    pub struct Collector {
        socket: PathBuf,
        dir: PathBuf,
        shared: Arc<Mutex<Shared>>,
        /// The running collection and the settings it runs with.
        run: Arc<tokio::sync::Mutex<Option<Running>>>,
    }

    impl Collector {
        /// A collector for the Docker socket at `socket`, keeping cursors in
        /// the root-only folder `dir`. Nothing runs until the agent asks.
        #[must_use]
        pub fn new(socket: &Path, dir: &Path) -> Self {
            let committed = load_cursors(dir);
            Self {
                socket: socket.to_path_buf(),
                dir: dir.to_path_buf(),
                shared: Arc::new(Mutex::new(Shared { buffer: Buffer::new(committed), ..Shared::default() })),
                run: Arc::default(),
            }
        }

        fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
            f(&mut self.shared.lock().unwrap_or_else(PoisonError::into_inner))
        }

        /// One poll of the agent.
        pub async fn handle(&self, req: TelemetryRequest) -> Result<TelemetryResponse, String> {
            req.validate()?;
            self.configure(&req).await;
            if !req.enabled {
                return Ok(TelemetryResponse::default());
            }
            let (batch, changed, samples) = self.with(|s| {
                let (b, c) = s.buffer.take(req.ack, forge_proto::messages::MAX_CONTAINER_LOG_LINES);
                (b, c, std::mem::take(&mut s.samples))
            });
            if changed {
                self.save().await;
            }
            let bridge_gateway = self.bridge_gateway().await;
            Ok(TelemetryResponse { batch, samples, bridge_gateway })
        }

        /// The default bridge network's gateway, read from the engine.
        /// Only private addresses are reported: the listener must never
        /// bind a public one.
        async fn bridge_gateway(&self) -> Option<std::net::IpAddr> {
            #[derive(Deserialize)]
            struct Network {
                #[serde(rename = "IPAM")]
                ipam: Ipam,
            }
            #[derive(Deserialize)]
            struct Ipam {
                #[serde(rename = "Config", default)]
                config: Option<Vec<IpamConfig>>,
            }
            #[derive(Deserialize)]
            struct IpamConfig {
                #[serde(rename = "Gateway", default)]
                gateway: String,
            }
            let (status, body) =
                crate::docker::call_at(&self.socket, "GET", "/networks/bridge", None, QUICK).await.ok()?;
            if status != 200 {
                return None;
            }
            let n: Network = serde_json::from_slice(&body).ok()?;
            n.ipam
                .config
                .unwrap_or_default()
                .iter()
                .filter_map(|c| c.gateway.parse::<std::net::IpAddr>().ok())
                .find(|ip| matches!(ip, std::net::IpAddr::V4(v4) if v4.is_private()))
        }

        /// Starts, restarts or stops the collection when the settings change.
        async fn configure(&self, req: &TelemetryRequest) {
            let mut run = self.run.lock().await;
            let same = run.as_ref().is_some_and(|(r, h)| {
                r.enabled == req.enabled && r.exclude == req.exclude && (!r.enabled || !h.is_finished())
            });
            if same {
                return;
            }
            if let Some((_, h)) = run.take() {
                h.abort();
            }
            if req.enabled {
                let me = self.clone();
                let exclude = req.exclude.clone();
                *run = Some((req.clone(), tokio::spawn(async move { me.collect(exclude).await })));
            } else {
                self.with(|s| {
                    s.buffer.clear();
                    s.samples.clear();
                });
                *run = Some((req.clone(), tokio::spawn(async {})));
            }
        }

        async fn save(&self) {
            let data = self.with(|s| serde_json::to_vec(s.buffer.committed()));
            let Ok(data) = data else { return };
            let dir = self.dir.clone();
            let r = tokio::task::spawn_blocking(move || {
                crate::self_update::ensure_private_dir(&dir)?;
                crate::self_update::write_nofollow(&dir.join(CURSOR_FILE), &data)
            })
            .await;
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => eprintln!("forge-agent-helper: saving container log cursors: {e}"),
                Err(e) => eprintln!("forge-agent-helper: saving container log cursors: {e}"),
            }
        }

        /// Follows every running container and samples them, until aborted.
        async fn collect(self, exclude: Vec<String>) {
            let mut stats = tokio::time::interval(STATS_EVERY);
            stats.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut wait = RETRY;
            let mut last_error = String::new();
            loop {
                let mut followers: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
                let mut tasks = JoinSet::new();
                let started = Instant::now();
                if let Err(e) = self.watch(&exclude, &mut followers, &mut tasks, &mut stats).await {
                    // Said once, not every retry (a node without Docker).
                    if e != last_error {
                        eprintln!("forge-agent-helper: container telemetry: {e}");
                        last_error = e;
                    }
                }
                // Dropping the set stops every follower; positions are kept.
                drop(tasks);
                wait = if started.elapsed() > MAX_RETRY { RETRY } else { (wait * 2).min(MAX_RETRY) };
                tokio::time::sleep(wait).await;
            }
        }

        async fn watch(
            &self,
            exclude: &[String],
            followers: &mut HashMap<String, tokio::task::AbortHandle>,
            tasks: &mut JoinSet<()>,
            stats: &mut tokio::time::Interval,
        ) -> Result<(), String> {
            let filters = r#"{"type":["container"],"event":["start","destroy"]}"#;
            let path = format!(
                "/events?filters={}",
                url::form_urlencoded::byte_serialize(filters.as_bytes()).collect::<String>()
            );
            let mut events = crate::docker::stream_at(&self.socket, &path, QUICK)
                .await?
                .ok_or_else(|| "Docker has no events endpoint".to_owned())?;
            // Containers already running when the watch starts.
            let (status, body) = crate::docker::call_at(&self.socket, "GET", "/containers/json", None, QUICK).await?;
            if status != 200 {
                return Err(crate::docker::docker_error(status, &body));
            }
            let running: Vec<Listed> = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
            for c in running {
                self.follow(c.id, exclude, followers, tasks);
            }
            let mut partial: Vec<u8> = Vec::new();
            loop {
                tokio::select! {
                    frame = events.frame() => {
                        let Some(frame) = frame else { return Err("Docker closed the event stream".into()) };
                        let frame = frame.map_err(|e| e.to_string())?;
                        let Ok(data) = frame.into_data() else { continue };
                        partial.extend_from_slice(&data);
                        while let Some(nl) = partial.iter().position(|b| *b == b'\n') {
                            let rec: Vec<u8> = partial.drain(..=nl).collect();
                            match serde_json::from_slice::<Event>(&rec) {
                                Ok(ev) if ev.action == "start" => self.follow(ev.actor.id, exclude, followers, tasks),
                                Ok(ev) if ev.action == "destroy" => self.forget(&ev.actor.id).await,
                                Ok(_) => {}
                                Err(e) => eprintln!("forge-agent-helper: unreadable Docker event: {e}"),
                            }
                        }
                        if partial.len() > MAX_EVENT {
                            partial.clear();
                        }
                    }
                    _ = stats.tick() => self.sample(exclude).await,
                    Some(_) = tasks.join_next() => {}
                }
                followers.retain(|_, h| !h.is_finished());
            }
        }

        fn follow(
            &self,
            id: String,
            exclude: &[String],
            followers: &mut HashMap<String, tokio::task::AbortHandle>,
            tasks: &mut JoinSet<()>,
        ) {
            if !crate::helper::valid_container_id(&id) || followers.get(&id).is_some_and(|h| !h.is_finished()) {
                return;
            }
            let me = self.clone();
            let exclude = exclude.to_vec();
            let key = id.clone();
            let h = tasks.spawn(async move {
                if let Err(e) = me.follow_one(&id, &exclude).await {
                    eprintln!("forge-agent-helper: following container {}: {e}", super::short_id(&id));
                }
            });
            followers.insert(key, h);
        }

        async fn inspect(&self, id: &str) -> Result<Option<Inspect>, String> {
            let (status, body) =
                crate::docker::call_at(&self.socket, "GET", &format!("/containers/{id}/json"), None, QUICK).await?;
            match status {
                200 => serde_json::from_slice(&body).map(Some).map_err(|e| e.to_string()),
                404 => Ok(None),
                _ => Err(crate::docker::docker_error(status, &body)),
            }
        }

        async fn follow_one(&self, id: &str, exclude: &[String]) -> Result<(), String> {
            let Some(c) = self.inspect(id).await? else { return Ok(()) };
            if c.excluded(exclude) {
                return Ok(());
            }
            let meta = c.meta();
            let after = self.with(|s| s.buffer.resume_after(&meta.id));
            // `since` is whole seconds at most this precise; lines up to the
            // resume point are skipped by the buffer.
            let since = after
                .map(|ns| format!("&since={}.{:09}", ns.div_euclid(1_000_000_000), ns.rem_euclid(1_000_000_000)))
                .unwrap_or_default();
            let path = format!("/containers/{id}/logs?follow=1&stdout=1&stderr=1&timestamps=1{since}");
            let Some(mut body) = crate::docker::stream_at(&self.socket, &path, QUICK).await? else { return Ok(()) };
            let mut demux = Demux::new(c.config.tty);
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|e| e.to_string())?;
                let Ok(data) = frame.into_data() else { continue };
                let lines: Vec<Line> = demux
                    .push(&data)
                    .into_iter()
                    .filter_map(|(stream, raw)| {
                        let (ts_ns, line) = split_timestamp(&raw)?;
                        Some(Line { container: meta.clone(), stream, ts_ns, line })
                    })
                    .collect();
                if !lines.is_empty() {
                    self.with(|s| {
                        for l in lines {
                            s.buffer.push(l);
                        }
                    });
                }
            }
            Ok(())
        }

        async fn forget(&self, id: &str) {
            if self.with(|s| {
                s.counters.remove(&super::short_id(id));
                s.buffer.forget(&super::short_id(id))
            }) {
                self.save().await;
            }
        }

        /// Samples every running, not excluded container (bounded).
        async fn sample(&self, exclude: &[String]) {
            let Ok((200, body)) = crate::docker::call_at(&self.socket, "GET", "/containers/json", None, QUICK).await
            else {
                return;
            };
            let Ok(running) = serde_json::from_slice::<Vec<Listed>>(&body) else { return };
            let mut out = Vec::new();
            for c in running.into_iter().take(forge_proto::messages::MAX_CONTAINER_SAMPLES) {
                if !crate::helper::valid_container_id(&c.id) {
                    continue;
                }
                let Ok(Some(i)) = self.inspect(&c.id).await else { continue };
                if i.excluded(exclude) {
                    continue;
                }
                let path = format!("/containers/{}/stats?stream=false", c.id);
                let Ok((200, raw)) = crate::docker::call_at(&self.socket, "GET", &path, None, QUICK).await else {
                    continue;
                };
                let Ok(stats) = serde_json::from_slice::<Value>(&raw) else { continue };
                let meta = i.meta();
                let now = Counters { at: Instant::now(), ..counters(&stats) };
                let prev = self.with(|s| s.counters.insert(meta.id.clone(), now));
                out.push(sample(meta, i.restarts, &stats, prev, now));
            }
            self.with(|s| s.samples = out);
        }
    }

    fn load_cursors(dir: &Path) -> Cursors {
        crate::self_update::read_nofollow(&dir.join(CURSOR_FILE), MAX_CURSOR_FILE)
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default()
    }

    fn u(s: &Value, p: &str) -> u64 {
        s.pointer(p).and_then(Value::as_u64).unwrap_or(0)
    }

    /// Network and block-device byte counters of a stats answer.
    fn counters(s: &Value) -> Counters {
        let (mut rx, mut tx) = (0u64, 0u64);
        if let Some(nets) = s.get("networks").and_then(Value::as_object) {
            for n in nets.values() {
                rx = rx.saturating_add(u(n, "/rx_bytes"));
                tx = tx.saturating_add(u(n, "/tx_bytes"));
            }
        }
        let (mut read, mut write) = (0u64, 0u64);
        for e in s.pointer("/blkio_stats/io_service_bytes_recursive").and_then(Value::as_array).into_iter().flatten() {
            match e.get("op").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
                Some("read") => read = read.saturating_add(u(e, "/value")),
                Some("write") => write = write.saturating_add(u(e, "/value")),
                _ => {}
            }
        }
        Counters { at: Instant::now(), rx, tx, read, write }
    }

    fn sample(meta: Meta, restarts: u32, s: &Value, prev: Option<Counters>, now: Counters) -> Sample {
        let rate = |a: u64, b: u64| match prev {
            Some(p) => {
                let secs = now.at.duration_since(p.at).as_secs_f64();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                // reason: a byte rate; counters that went back (restart) give 0
                if secs > 0.0 { (b.saturating_sub(a) as f64 / secs) as u64 } else { 0 }
            }
            None => 0,
        };
        let usage = u(s, "/memory_stats/usage");
        // The page cache Docker's CLI leaves out (cgroup v2, then v1).
        let cache = s
            .pointer("/memory_stats/stats/inactive_file")
            .or_else(|| s.pointer("/memory_stats/stats/total_inactive_file"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        #[allow(clippy::cast_possible_truncation)] // reason: a percentage fits f32
        let cpu = crate::docker::stats_cpu_percent(s) as f32;
        Sample {
            container: meta,
            cpu_percent: cpu,
            mem_used: usage.saturating_sub(cache),
            mem_limit: u(s, "/memory_stats/limit"),
            rx_per_sec: rate(prev.map_or(0, |p| p.rx), now.rx),
            tx_per_sec: rate(prev.map_or(0, |p| p.tx), now.tx),
            read_per_sec: rate(prev.map_or(0, |p| p.read), now.read),
            write_per_sec: rate(prev.map_or(0, |p| p.write), now.write),
            restarts,
        }
    }
}

/// The short (12 character) form of a container id.
#[must_use]
pub fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

/// A helper answer as spool rows (D-142).
pub mod rows {
    use forge_core::spool::{self, ContainerLine, ContainerMinuteRow, Row};

    use super::{Batch, Labels, Sample, Stream};

    fn labels(l: &Labels) -> spool::Labels {
        spool::Labels {
            forge_app: l.forge_app.clone(),
            forge_stack: l.forge_stack.clone(),
            compose_project: l.compose_project.clone(),
            compose_service: l.compose_service.clone(),
        }
    }

    /// The batch's lines, one row each.
    pub fn lines(b: &Batch) -> impl Iterator<Item = Row> + '_ {
        b.lines.iter().map(|l| {
            Row::ContainerLog(ContainerLine {
                ts: l.ts_ns.div_euclid(1_000_000),
                stream: match l.stream {
                    Stream::Stdout => spool::Stream::Stdout,
                    Stream::Stderr => spool::Stream::Stderr,
                },
                cid: l.container.id.clone(),
                name: l.container.name.clone(),
                image: l.container.image.clone(),
                labels: labels(&l.container.labels),
                line: l.line.clone(),
            })
        })
    }

    /// The samples as rows of the minute starting at `minute` (epoch ms).
    pub fn samples(s: &[Sample], minute: i64) -> impl Iterator<Item = Row> + '_ {
        s.iter().take(forge_proto::messages::MAX_CONTAINER_SAMPLES).map(move |s| {
            Row::ContainerMinute(ContainerMinuteRow {
                minute,
                cid: s.container.id.clone(),
                name: s.container.name.clone(),
                image: s.container.image.clone(),
                labels: labels(&s.container.labels),
                cpu: s.cpu_percent,
                mem_used: s.mem_used,
                mem_limit: s.mem_limit,
                rx: s.rx_per_sec,
                tx: s.tx_per_sec,
                block_read: s.read_per_sec,
                block_write: s.write_per_sec,
                restarts: s.restarts,
            })
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn frame(stream: u8, data: &[u8]) -> Vec<u8> {
        let mut f = vec![stream, 0, 0, 0];
        f.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        f.extend_from_slice(data);
        f
    }

    fn meta(id: &str) -> Meta {
        Meta { id: id.into(), name: format!("{id}-name"), image: "img".into(), labels: Labels::default() }
    }

    fn line(id: &str, ts_ns: i64) -> Line {
        Line { container: meta(id), stream: Stream::Stdout, ts_ns, line: format!("line {ts_ns}") }
    }

    #[test]
    fn demultiplexes_frames_split_anywhere() {
        let mut raw = frame(1, b"2024-05-01T10:00:00.5Z out one\n2024-05-01T10:00:01Z out ");
        raw.extend(frame(2, b"2024-05-01T10:00:02Z err\n"));
        raw.extend(frame(1, b"two\n"));
        // Fed one byte at a time, the result is the same.
        let mut d = Demux::new(false);
        let mut got = Vec::new();
        for b in &raw {
            got.extend(d.push(std::slice::from_ref(b)));
        }
        let text: Vec<(Stream, String)> = got.into_iter().map(|(s, l)| (s, String::from_utf8(l).unwrap())).collect();
        assert_eq!(
            text,
            vec![
                (Stream::Stdout, "2024-05-01T10:00:00.5Z out one".to_owned()),
                (Stream::Stderr, "2024-05-01T10:00:02Z err".to_owned()),
                (Stream::Stdout, "2024-05-01T10:00:01Z out two".to_owned()),
            ],
            "stdout's partial line waits for its end; stderr is separate"
        );
    }

    #[test]
    fn tty_output_is_stdout_and_long_lines_are_cut() {
        let mut d = Demux::new(true);
        let got = d.push(b"2024-05-01T10:00:00Z a\r\nb");
        assert_eq!(got.len(), 1);
        assert_eq!(split_timestamp(&got[0].1).unwrap().1, "a");
        let long = vec![b'x'; MAX_PARTIAL + 10];
        let got = Demux::new(true).push(&long);
        assert_eq!(got.len(), 1, "an endless line is flushed");
        assert!(got[0].1.len() <= MAX_PARTIAL + 10);
    }

    #[test]
    fn parses_docker_timestamps() {
        assert_eq!(parse_ts_ns("1970-01-01T00:00:01Z"), Some(1_000_000_000));
        assert_eq!(parse_ts_ns("1970-01-01T00:00:01.000000002Z"), Some(1_000_000_002));
        assert_eq!(parse_ts_ns("1970-01-01T00:00:01.5Z"), Some(1_500_000_000));
        assert_eq!(parse_ts_ns("nope"), None);
        assert_eq!(
            split_timestamp(b"1970-01-01T00:00:02Z hello world").unwrap(),
            (2_000_000_000, "hello world".into())
        );
        assert!(split_timestamp(b"no timestamp").is_none());
    }

    #[test]
    fn excludes_by_name_or_label() {
        let labels: HashMap<String, String> = [("tier".to_owned(), "db".to_owned())].into();
        assert!(excluded("pg", &labels, &["pg".into()]));
        assert!(excluded("x", &labels, &["tier=db".into()]));
        assert!(!excluded("x", &labels, &["tier=web".into(), "pg".into()]));
    }

    #[test]
    fn a_batch_is_resent_until_acknowledged_and_only_then_moves_the_cursor() {
        let mut b = Buffer::new(Cursors::default());
        assert!(b.push(line("c1", 10)));
        assert!(b.push(line("c1", 20)));
        let (first, changed) = b.take(0, 100);
        let first = first.unwrap();
        assert_eq!((first.seq, first.lines.len(), changed), (1, 2, false));
        assert!(b.committed().0.is_empty(), "nothing is committed before the ack");
        // No ack (heartbeat failed): the same batch again.
        b.push(line("c1", 30));
        assert_eq!(b.take(0, 100).0.unwrap(), first);
        // A wrong ack changes nothing.
        assert_eq!(b.take(99, 100).0.unwrap(), first);
        let (next, changed) = b.take(1, 100);
        assert!(changed);
        assert_eq!(b.committed().0["c1"], 20);
        let next = next.unwrap();
        assert_eq!((next.seq, next.lines.len()), (2, 1));
        assert_eq!(next.lines[0].ts_ns, 30);
    }

    #[test]
    fn resumes_after_a_restart_without_drops_or_duplicates() {
        // Run 1 stores up to 20; 30 was read but never acknowledged.
        let mut run1 = Buffer::new(Cursors::default());
        for t in [10, 20] {
            run1.push(line("c1", t));
        }
        run1.take(0, 100);
        run1.take(1, 100);
        run1.push(line("c1", 30));
        let saved = serde_json::to_vec(run1.committed()).unwrap();

        // Run 2 starts from the saved cursors and re-reads from `since`.
        let mut run2 = Buffer::new(serde_json::from_slice(&saved).unwrap());
        assert_eq!(run2.resume_after("c1"), Some(20));
        let taken: Vec<bool> = [10, 20, 30, 40].iter().map(|t| run2.push(line("c1", *t))).collect();
        assert_eq!(taken, vec![false, false, true, true], "stored lines are skipped, the rest kept");
        let batch = run2.take(0, 100).0.unwrap();
        assert_eq!(batch.lines.iter().map(|l| l.ts_ns).collect::<Vec<_>>(), vec![30, 40]);
        // A follower restarted in the same run skips what it already read.
        assert_eq!(run2.resume_after("c1"), Some(40));
        assert!(!run2.push(line("c1", 40)));
    }

    #[test]
    fn the_buffer_is_bounded_and_counts_drops() {
        let mut b = Buffer::new(Cursors::default());
        for t in 0..i64::try_from(MAX_BUFFER_LINES).unwrap() + 5 {
            b.push(line("c1", t + 1));
        }
        let batch = b.take(0, forge_proto::messages::MAX_CONTAINER_LOG_LINES).0.unwrap();
        assert_eq!(batch.dropped, 5);
        assert_eq!(batch.lines.len(), forge_proto::messages::MAX_CONTAINER_LOG_LINES);
        assert_eq!(batch.lines[0].ts_ns, 6, "the oldest went first");
        let rows: Vec<_> = rows::lines(&batch).collect();
        assert_eq!(rows.len(), forge_proto::messages::MAX_CONTAINER_LOG_LINES);
        match &rows[0] {
            forge_core::spool::Row::ContainerLog(l) => assert_eq!(l.ts, 0, "nanoseconds become milliseconds"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn cursors_are_bounded_and_forgotten() {
        let mut c = Cursors::default();
        c.advance("a", 5);
        c.advance("a", 3);
        assert_eq!(c.0["a"], 5, "never back");
        for i in 0..MAX_CURSORS + 3 {
            c.advance(&format!("k{i}"), i64::try_from(i).unwrap() + 10);
        }
        assert_eq!(c.0.len(), MAX_CURSORS);
        assert!(!c.0.contains_key("a"), "the oldest position went first");
        let mut b = Buffer::new(c);
        assert!(b.forget("k100"));
        assert_eq!(b.resume_after("k100"), None);
    }

    #[test]
    fn disabling_drops_what_is_buffered() {
        let mut b = Buffer::new(Cursors::default());
        b.push(line("c1", 1));
        b.take(0, 10);
        b.clear();
        assert_eq!(b.take(1, 10).0, None, "the pending batch is gone; the ack is moot");
    }

    #[test]
    fn requests_are_bounded() {
        let ok = TelemetryRequest { enabled: true, exclude: vec!["a".into()], ack: 0 };
        assert!(ok.validate().is_ok());
        let many = TelemetryRequest { exclude: (0..65).map(|i| format!("c{i}")).collect(), ..ok.clone() };
        assert!(many.validate().is_err());
        let empty = TelemetryRequest { exclude: vec![String::new()], ..ok };
        assert!(empty.validate().is_err());
    }
}
