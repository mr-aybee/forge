//! ClickHouse over HTTP.

use std::fmt::Write as _;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::analytics::{
    AnalyticsStore, ContainerMinute, ContainerPoint, ContainerSeries, EngineStats, NodeMinute, NodePoint,
    NodeProcessMinute, ProcessUsage, Resolution, Retention, TopBy,
};
use forge_core::logs::{LogBucket, LogEvent, LogLevel, LogPage, LogQuery, LogSource, LogStream};
use forge_core::{DomainError, DomainResult, ServerId, Timestamp};
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::schema::{MIGRATIONS, TTLS};

/// Where the ClickHouse HTTP interface is and how to sign in.
#[derive(Clone)]
pub struct ClickHouseTarget {
    /// `http(s)://host:port`, without user, password or path.
    pub endpoint: String,
    pub user: String,
    pub password: String,
    pub database: String,
    /// Engine name reported by [`AnalyticsStore::engine`].
    pub engine: &'static str,
}

impl std::fmt::Debug for ClickHouseTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClickHouseTarget")
            .field("endpoint", &self.endpoint)
            .field("user", &self.user)
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

impl ClickHouseTarget {
    /// From `https://user@host:8443/database` plus a password kept apart.
    pub fn from_url(url: &str, password: Option<&str>, engine: &'static str) -> DomainResult<Self> {
        let u = url::Url::parse(url).map_err(|e| DomainError::Invalid(format!("analytics url: {e}")))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(DomainError::Invalid("a ClickHouse url starts with http:// or https://".into()));
        }
        let database = u.path().trim_start_matches('/').to_string();
        if database.is_empty() || !database.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(DomainError::Invalid("analytics database name must be letters, digits and _".into()));
        }
        let host = u.host_str().ok_or_else(|| DomainError::Invalid("analytics url has no host".into()))?;
        let port = u.port_or_known_default().unwrap_or(8123);
        let user = if u.username().is_empty() { "default".to_string() } else { u.username().to_string() };
        Ok(Self {
            endpoint: format!("{}://{host}:{port}", u.scheme()),
            user,
            password: password.unwrap_or_default().to_string(),
            database,
            engine,
        })
    }
}

/// The tables spool rows go to.
pub(crate) const APP_LOGS: &str = "app_logs";
pub(crate) const NODE_MINUTES: &str = "node_metrics_1m";
pub(crate) const TOP_PROCESSES: &str = "node_top_processes_1m";
pub(crate) const CONTAINER_MINUTES: &str = "container_metrics_1m";

/// A ClickHouse analytics store.
#[derive(Debug, Clone)]
pub struct ClickHouse {
    http: reqwest::Client,
    target: ClickHouseTarget,
}

fn db_err(e: impl std::fmt::Display) -> DomainError {
    DomainError::Internal(format!("analytics: {e}"))
}

/// One query: SQL with `{name:Type}` placeholders and its parameters.
struct Query<'a> {
    sql: String,
    params: Vec<(&'a str, String)>,
}

/// Engine settings sent with every query; a search overrides some of them
/// with its [`SearchLimits`](forge_core::logs::SearchLimits).
const BASE_SETTINGS: [(&str, &str); 4] = [
    ("output_format_json_quote_64bit_integers", "0"),
    ("max_execution_time", "20"),
    ("max_result_rows", "100000"),
    ("result_overflow_mode", "break"),
];

impl ClickHouse {
    pub fn new(target: ClickHouseTarget) -> DomainResult<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(db_err)?;
        Ok(Self { http, target })
    }

    /// Runs `q` against `database` (`None`: the server's default database).
    async fn send(&self, database: Option<&str>, q: Query<'_>, body: Option<String>) -> DomainResult<String> {
        self.send_with(database, q, body, &[]).await
    }

    /// [`send`](Self::send) with engine settings that replace the base ones.
    async fn send_with(
        &self,
        database: Option<&str>,
        q: Query<'_>,
        body: Option<String>,
        settings: &[(&str, String)],
    ) -> DomainResult<String> {
        let mut req = self
            .http
            .post(format!("{}/", self.target.endpoint))
            .header("X-ClickHouse-User", &self.target.user)
            .header("X-ClickHouse-Key", &self.target.password);
        for (k, v) in BASE_SETTINGS {
            if !settings.iter().any(|(o, _)| *o == k) {
                req = req.query(&[(k, v)]);
            }
        }
        for (k, v) in settings {
            req = req.query(&[(*k, v.as_str())]);
        }
        // A search with a longer engine timeout needs a longer HTTP timeout.
        if let Some((_, t)) = settings.iter().find(|(k, _)| *k == "max_execution_time")
            && let Ok(secs) = t.parse::<u64>()
        {
            req = req.timeout(Duration::from_secs(secs.saturating_add(10)));
        }
        if let Some(db) = database {
            req = req.query(&[("database", db)]);
        }
        // An insert carries its rows in the body after the statement; any
        // other statement is the body itself, or with parameters a form.
        req = match body {
            // Rows travel zstd-compressed: the engine decompresses them
            // natively, and the link carries a fraction of the bytes.
            Some(rows) => {
                for (k, v) in &q.params {
                    req = req.query(&[(format!("param_{k}"), v)]);
                }
                let packed = zstd::bulk::compress(rows.as_bytes(), 1).map_err(db_err)?;
                req.query(&[("query", q.sql.as_str())]).header("Content-Encoding", "zstd").body(packed)
            }
            None if q.params.is_empty() => req.body(q.sql),
            None => req.header(CONTENT_TYPE, FORM_CONTENT_TYPE).body(form(&q.sql, &q.params)?),
        };
        let res = req.send().await.map_err(db_err)?;
        let status = res.status();
        let text = res.text().await.map_err(db_err)?;
        if !status.is_success() {
            let first = text.lines().next().unwrap_or_default();
            return Err(DomainError::Internal(format!("analytics: {status}: {first}")));
        }
        Ok(text)
    }

    async fn exec(&self, sql: String) -> DomainResult<()> {
        self.send(Some(&self.target.database), Query { sql, params: Vec::new() }, None).await.map(drop)
    }

    async fn rows<T: DeserializeOwned>(&self, q: Query<'_>) -> DomainResult<Vec<T>> {
        self.rows_with(q, &[]).await
    }

    async fn rows_with<T: DeserializeOwned>(&self, q: Query<'_>, settings: &[(&str, String)]) -> DomainResult<Vec<T>> {
        let mut q = q;
        q.sql.push_str(" FORMAT JSONEachRow");
        let text = self.send_with(Some(&self.target.database), q, None, settings).await?;
        text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).map_err(db_err)).collect()
    }

    async fn insert<T: serde::Serialize>(&self, table: &str, rows: &[T]) -> DomainResult<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut body = String::with_capacity(rows.len() * 160);
        ndjson(rows, &mut body)?;
        let q = Query { sql: format!("INSERT INTO {table} FORMAT JSONEachRow"), params: Vec::new() };
        // Asynchronous inserts (D-142): the engine groups many small inserts
        // into larger parts. `wait_for_async_insert` answers only once the
        // rows are written. These rows carry no file id. The tables keep a
        // deduplication window for spool files (D-145), so a block identical
        // to a recent one is dropped; that only happens when the same rows
        // are written again (log rows carry unique ids).
        let settings = [("async_insert", "1".to_owned()), ("wait_for_async_insert", "1".to_owned())];
        self.send_with(Some(&self.target.database), q, Some(body), &settings).await.map(drop)
    }

    /// Inserts `body` (JSONEachRow lines) into `table` once per `token`: a
    /// second insert with the same token within the table's deduplication
    /// window is acknowledged without writing (D-145).
    pub(crate) async fn insert_once(&self, table: &str, body: String, token: &str) -> DomainResult<()> {
        if body.is_empty() {
            return Ok(());
        }
        let q = Query { sql: format!("INSERT INTO {table} FORMAT JSONEachRow"), params: Vec::new() };
        self.send_with(Some(&self.target.database), q, Some(body), &crate::spool::insert_settings(token))
            .await
            .map(drop)
    }

    async fn applied(&self) -> DomainResult<u32> {
        #[derive(Deserialize)]
        struct V {
            v: u32,
        }
        let v: Vec<V> =
            self.rows(Query { sql: "SELECT max(version) AS v FROM forge_schema".into(), params: Vec::new() }).await?;
        Ok(v.first().map_or(0, |x| x.v))
    }
}

/// Boundary of the form a parameterised statement is sent in.
const FORM_BOUNDARY: &str = "forge-analytics-form-5f0c9a3e71d24b86";
const FORM_CONTENT_TYPE: &str = "multipart/form-data; boundary=forge-analytics-form-5f0c9a3e71d24b86";

/// A statement and its parameters as a `multipart/form-data` body. The
/// engine reads `query` and `param_*` fields from it like URL parameters,
/// but a body has no length limit: a search scoped to the nodes a caller
/// may view lists every one of them, and at about 1,400 nodes that list
/// passed the 64 KiB a URL may hold, so every search failed.
fn form<K: AsRef<str>>(sql: &str, params: &[(K, String)]) -> DomainResult<String> {
    let fields = std::iter::once(("query".to_owned(), sql))
        .chain(params.iter().map(|(k, v)| (format!("param_{}", k.as_ref()), v.as_str())));
    let mut out =
        String::with_capacity(sql.len() + params.iter().map(|(k, v)| k.as_ref().len() + v.len() + 96).sum::<usize>());
    for (name, value) in fields {
        if value.contains(FORM_BOUNDARY) || name.contains(['"', '\r', '\n']) {
            return Err(DomainError::Invalid(format!("analytics: the {name} value cannot be sent")));
        }
        out.push_str("--");
        out.push_str(FORM_BOUNDARY);
        out.push_str("\r\nContent-Disposition: form-data; name=\"");
        out.push_str(&name);
        out.push_str("\"\r\n\r\n");
        out.push_str(value);
        out.push_str("\r\n");
    }
    out.push_str("--");
    out.push_str(FORM_BOUNDARY);
    out.push_str("--\r\n");
    Ok(out)
}

fn fill(sql: &str, r: Retention) -> String {
    sql.replace("{node_days}", &r.node_metrics_days.max(1).to_string())
        .replace("{top_days}", &r.top_processes_days.max(1).to_string())
        .replace("{rollup_days}", &r.node_rollup_days.max(1).to_string())
        .replace("{log_days}", &r.app_logs_days.max(1).to_string())
}

/// The day count in a table's `engine_full` TTL clause
/// (`TTL minute + toIntervalDay(7)`).
fn ttl_days(engine_full: &str) -> Option<u32> {
    let rest = &engine_full[engine_full.find(" TTL ")?..];
    let start = rest.find("toIntervalDay(")? + "toIntervalDay(".len();
    let digits: String = rest[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Engine settings for one search; `0` keeps the engine's default.
fn search_settings(q: &LogQuery) -> Vec<(&'static str, String)> {
    let l = q.limits;
    let mut out = Vec::new();
    if l.timeout_secs > 0 {
        out.push(("max_execution_time", l.timeout_secs.to_string()));
    }
    if l.threads > 0 {
        out.push(("max_threads", l.threads.to_string()));
    }
    if l.memory_bytes > 0 {
        out.push(("max_memory_usage", l.memory_bytes.to_string()));
    }
    out
}

/// Seconds since the epoch of `t` (the tables use `DateTime`).
fn secs(t: Timestamp) -> i64 {
    t.0.div_euclid(1000)
}

#[derive(serde::Serialize)]
pub(crate) struct MinuteRow {
    server_id: String,
    minute: i64,
    cpu_avg: f32,
    cpu_max: f32,
    mem_used: u64,
    mem_total: u64,
    swap_used: u64,
    disk_used: u64,
    disk_total: u64,
    rx: u64,
    tx: u64,
    load1: f32,
    samples: u32,
}

#[derive(serde::Serialize)]
pub(crate) struct ProcRow<'a> {
    server_id: String,
    minute: i64,
    pid: u32,
    name: &'a str,
    user: &'a str,
    cpu: f32,
    mem: u64,
    cmd: &'a str,
}

/// `DateTime64(3)` input: decimal seconds as a string (an integer would be
/// read as raw ticks).
fn dt64(ms: i64) -> String {
    format!("{}.{:03}", ms.div_euclid(1000), ms.rem_euclid(1000))
}

#[derive(serde::Serialize)]
pub(crate) struct LogRow<'a> {
    service_id: Option<String>,
    ts: String,
    id: String,
    node_ts: Option<String>,
    ingest_ts: String,
    level: &'static str,
    message: &'a str,
    message_template: &'a str,
    exception: &'a str,
    trace_id: &'a str,
    span_id: &'a str,
    event_type: u32,
    server_id: Option<String>,
    properties: String,
    source: &'static str,
    container_id: &'a str,
    container_name: &'a str,
    stack: &'a str,
    stream: &'static str,
}

#[derive(serde::Serialize)]
pub(crate) struct ContainerRow<'a> {
    server_id: String,
    container_name: &'a str,
    minute: i64,
    container_id: &'a str,
    image: &'a str,
    service_id: Option<String>,
    stack: &'a str,
    cpu: f32,
    mem_used: u64,
    mem_limit: u64,
    rx: u64,
    tx: u64,
    block_read: u64,
    block_write: u64,
    restarts: u32,
}

impl From<&NodeMinute> for MinuteRow {
    fn from(r: &NodeMinute) -> Self {
        Self {
            server_id: r.server_id.to_string(),
            minute: secs(r.minute),
            cpu_avg: r.cpu_avg,
            cpu_max: r.cpu_max,
            mem_used: r.mem_used,
            mem_total: r.mem_total,
            swap_used: r.swap_used,
            disk_used: r.disk_used,
            disk_total: r.disk_total,
            rx: r.rx,
            tx: r.tx,
            load1: r.load1,
            samples: r.samples,
        }
    }
}

impl<'a> From<&'a NodeProcessMinute> for ProcRow<'a> {
    fn from(r: &'a NodeProcessMinute) -> Self {
        Self {
            server_id: r.server_id.to_string(),
            minute: secs(r.minute),
            pid: r.pid,
            name: &r.name,
            user: &r.user,
            cpu: r.cpu,
            mem: r.mem,
            cmd: &r.cmd,
        }
    }
}

impl<'a> From<&'a LogEvent> for LogRow<'a> {
    fn from(e: &'a LogEvent) -> Self {
        Self {
            service_id: e.service_id.map(|s| s.to_string()),
            ts: dt64(e.ts.0),
            id: e.id.to_string(),
            node_ts: e.node_ts.map(|t| dt64(t.0)),
            ingest_ts: dt64(e.ingest_ts.0),
            level: e.level.as_str(),
            message: &e.message,
            message_template: &e.message_template,
            exception: e.exception.as_deref().unwrap_or_default(),
            trace_id: e.trace_id.as_deref().unwrap_or_default(),
            span_id: e.span_id.as_deref().unwrap_or_default(),
            event_type: e.event_type,
            server_id: e.server_id.map(|s| s.to_string()),
            properties: serde_json::Value::Object(e.properties.clone()).to_string(),
            source: e.source.as_str(),
            container_id: e.container_id.as_deref().unwrap_or_default(),
            container_name: e.container_name.as_deref().unwrap_or_default(),
            stack: e.stack.as_deref().unwrap_or_default(),
            stream: e.stream.map_or("", LogStream::as_str),
        }
    }
}

impl<'a> From<&'a ContainerMinute> for ContainerRow<'a> {
    fn from(r: &'a ContainerMinute) -> Self {
        Self {
            server_id: r.server_id.to_string(),
            container_name: &r.container_name,
            minute: secs(r.minute),
            container_id: &r.container_id,
            image: &r.image,
            service_id: r.service_id.map(|s| s.to_string()),
            stack: r.stack.as_deref().unwrap_or_default(),
            cpu: r.cpu,
            mem_used: r.mem_used,
            mem_limit: r.mem_limit,
            rx: r.rx,
            tx: r.tx,
            block_read: r.block_read,
            block_write: r.block_write,
            restarts: r.restarts,
        }
    }
}

/// Appends `rows` to `out` as JSONEachRow lines.
pub(crate) fn ndjson<T: serde::Serialize>(rows: impl IntoIterator<Item = T>, out: &mut String) -> DomainResult<()> {
    for r in rows {
        out.push_str(&serde_json::to_string(&r).map_err(db_err)?);
        out.push('\n');
    }
    Ok(())
}

#[derive(Deserialize)]
struct ContainerOut {
    container_name: String,
    container_id: String,
    image: String,
    service_id: Option<forge_core::ServiceId>,
    stack: String,
    t: i64,
    cpu_avg: f32,
    cpu_max: f32,
    mem_used: u64,
    mem_limit: u64,
    rx: u64,
    tx: u64,
    block_read: u64,
    block_write: u64,
    restarts: u32,
}

/// Groups rows ordered by `(container_name, t)` into one series per container.
fn group_series(rows: Vec<ContainerOut>) -> Vec<ContainerSeries> {
    forge_core::analytics::group_series(rows.into_iter().map(|r| forge_core::analytics::ContainerRowOut {
        container_name: r.container_name,
        container_id: r.container_id,
        image: r.image,
        service_id: r.service_id,
        stack: r.stack,
        point: ContainerPoint {
            t: r.t,
            cpu_avg: r.cpu_avg,
            cpu_max: r.cpu_max,
            mem_used: r.mem_used,
            mem_limit: r.mem_limit,
            rx: r.rx,
            tx: r.tx,
            block_read: r.block_read,
            block_write: r.block_write,
            restarts: r.restarts,
        },
    }))
}

#[derive(Deserialize)]
struct LogOut {
    service_id: Option<forge_core::ServiceId>,
    // Aliases differ from the column names: ClickHouse resolves an alias
    // before a column in WHERE, which would compare numbers with times.
    #[serde(rename = "ts_ms")]
    ts: i64,
    id: uuid::Uuid,
    #[serde(rename = "node_ts_ms")]
    node_ts: Option<i64>,
    #[serde(rename = "ingest_ts_ms")]
    ingest_ts: i64,
    level: String,
    message: String,
    message_template: String,
    exception: String,
    trace_id: String,
    span_id: String,
    event_type: u32,
    server_id: Option<ServerId>,
    properties: String,
    source: String,
    container_id: String,
    container_name: String,
    stack: String,
    stream: String,
}

impl LogOut {
    fn into_event(self) -> LogEvent {
        let props = match serde_json::from_str::<serde_json::Value>(&self.properties) {
            Ok(serde_json::Value::Object(m)) => m,
            // A bag that cannot be read is shown empty rather than failing the page.
            _ => serde_json::Map::new(),
        };
        let some = |s: String| (!s.is_empty()).then_some(s);
        LogEvent {
            id: self.id,
            service_id: self.service_id,
            ts: Timestamp(self.ts),
            node_ts: self.node_ts.map(Timestamp),
            ingest_ts: Timestamp(self.ingest_ts),
            level: LogLevel::parse_lenient(&self.level).unwrap_or(LogLevel::Information),
            message: self.message,
            message_template: self.message_template,
            exception: some(self.exception),
            trace_id: some(self.trace_id),
            span_id: some(self.span_id),
            event_type: self.event_type,
            server_id: self.server_id,
            properties: props,
            source: LogSource::parse(&self.source).unwrap_or_default(),
            container_id: some(self.container_id),
            container_name: some(self.container_name),
            stack: some(self.stack),
            stream: LogStream::parse(&self.stream),
        }
    }
}

const LOG_COLUMNS: &str = "service_id, toUnixTimestamp64Milli(ts) AS ts_ms, id,
    toUnixTimestamp64Milli(node_ts) AS node_ts_ms, toUnixTimestamp64Milli(ingest_ts) AS ingest_ts_ms, level, message,
    message_template, exception, trace_id, span_id, event_type, server_id, properties, source, container_id,
    container_name, stack, stream";

/// The WHERE clause shared by event pages and the histogram. Every value is
/// a query parameter; only fixed SQL and generated parameter names are
/// written into the text.
fn log_where(q: &LogQuery) -> (String, Vec<(String, String)>) {
    // Whole services, single nodes of a service (`<app>:<server>` pairs),
    // or the unattributed lines of nodes the caller may view.
    let mut sql = String::from(
        "(service_id IN {apps:Array(UUID)} OR (service_id IS NOT NULL AND server_id IS NOT NULL AND \
         has({inst:Array(String)}, concat(toString(assumeNotNull(service_id)), ':', toString(assumeNotNull(server_id))))) \
         OR (service_id IS NULL AND server_id IN {open:Array(UUID)}))",
    );
    let list = |ids: Vec<String>| format!("[{}]", ids.join(","));
    let mut p = vec![
        ("apps".to_string(), list(q.services.iter().map(|a| format!("'{a}'")).collect())),
        ("inst".to_string(), list(q.instances.iter().map(|(a, s)| format!("'{a}:{s}'")).collect())),
        ("open".to_string(), list(q.open_nodes.iter().map(|s| format!("'{s}'")).collect())),
    ];
    if !q.nodes.is_empty() {
        sql.push_str(" AND server_id IN {nodes:Array(UUID)}");
        p.push(("nodes".into(), list(q.nodes.iter().map(|s| format!("'{s}'")).collect())));
    }
    if let Some(c) = &q.container {
        sql.push_str(" AND (container_name = {container:String} OR startsWith(container_id, {container:String}))");
        p.push(("container".into(), c.clone()));
    }
    if let Some(st) = &q.stack {
        sql.push_str(" AND stack = {stack:String}");
        p.push(("stack".into(), st.clone()));
    }
    if let Some(src) = q.source {
        sql.push_str(" AND source = {source:String}");
        p.push(("source".into(), src.as_str().to_owned()));
    }
    if !q.levels.is_empty() {
        sql.push_str(" AND level IN {levels:Array(String)}");
        let l: Vec<String> = q.levels.iter().map(|l| format!("'{}'", l.as_str())).collect();
        p.push(("levels".into(), format!("[{}]", l.join(","))));
    }
    if let Some(t) = &q.filter.text {
        sql.push_str(
            " AND (positionCaseInsensitiveUTF8(message, {text:String}) > 0
                OR positionCaseInsensitiveUTF8(exception, {text:String}) > 0)",
        );
        p.push(("text".into(), t.clone()));
    }
    for (i, (k, v)) in q.filter.properties.iter().enumerate() {
        // A value matches a JSON string or, when it reads as a JSON scalar
        // (number, true, false, null), that raw value.
        let quoted = serde_json::Value::String(v.clone()).to_string();
        let raw = match serde_json::from_str::<serde_json::Value>(v) {
            Ok(j @ (serde_json::Value::Number(_) | serde_json::Value::Bool(_) | serde_json::Value::Null)) => {
                j.to_string()
            }
            _ => quoted.clone(),
        };
        let _ =
            write!(sql, " AND JSONExtractRaw(properties, {{pk{i}:String}}) IN ({{pv{i}:String}}, {{pq{i}:String}})");
        p.push((format!("pk{i}"), k.clone()));
        p.push((format!("pv{i}"), raw));
        p.push((format!("pq{i}"), quoted));
    }
    if let Some(f) = q.from {
        sql.push_str(" AND ts >= fromUnixTimestamp64Milli({from:Int64})");
        p.push(("from".into(), f.0.to_string()));
    }
    if let Some(t) = q.to {
        sql.push_str(" AND ts < fromUnixTimestamp64Milli({to:Int64})");
        p.push(("to".into(), t.0.to_string()));
    }
    (sql, p)
}

#[async_trait]
impl AnalyticsStore for ClickHouse {
    fn engine(&self) -> &'static str {
        self.target.engine
    }

    async fn migrate(&self, retention: Retention) -> DomainResult<()> {
        let db = &self.target.database;
        // The name was validated in `from_url`, so it is a safe identifier.
        self.send(None, Query { sql: format!("CREATE DATABASE IF NOT EXISTS {db}"), params: Vec::new() }, None).await?;
        self.exec("CREATE TABLE IF NOT EXISTS forge_schema (version UInt32, applied_at DateTime('UTC') DEFAULT now()) ENGINE = MergeTree ORDER BY version".into()).await?;
        let have = self.applied().await?;
        for (version, statements) in MIGRATIONS {
            if *version <= have {
                continue;
            }
            for s in *statements {
                self.exec(fill(s, retention)).await?;
            }
            self.exec(format!("INSERT INTO forge_schema (version) VALUES ({version})")).await?;
            tracing::info!(version, "analytics schema migrated");
        }
        if let Some((latest, _)) = MIGRATIONS.last()
            && have > *latest
        {
            return Err(DomainError::Conflict(format!(
                "the analytics database has schema version {have}; this Forge knows up to {latest}"
            )));
        }
        self.apply_retention(retention).await
    }

    async fn apply_retention(&self, retention: Retention) -> DomainResult<()> {
        for (table, column, key) in TTLS {
            self.exec(fill(&format!("ALTER TABLE {table} MODIFY TTL {column} + INTERVAL {key} DAY"), retention))
                .await?;
        }
        Ok(())
    }

    async fn effective_retention(&self) -> DomainResult<Retention> {
        #[derive(Deserialize)]
        struct T {
            name: String,
            engine_full: String,
        }
        let rows: Vec<T> = self
            .rows(Query {
                sql: "SELECT name, engine_full FROM system.tables WHERE database = currentDatabase()".into(),
                params: Vec::new(),
            })
            .await?;
        let days = |table: &str| -> DomainResult<u32> {
            let t = rows.iter().find(|t| t.name == table).ok_or_else(|| db_err(format!("table {table} is missing")))?;
            ttl_days(&t.engine_full).ok_or_else(|| db_err(format!("table {table} has no TTL in days")))
        };
        Ok(Retention {
            node_metrics_days: days("node_metrics_1m")?,
            top_processes_days: days("node_top_processes_1m")?,
            node_rollup_days: days("node_metrics_1h")?,
            app_logs_days: days("app_logs")?,
        })
    }

    async fn engine_stats(&self) -> DomainResult<EngineStats> {
        #[derive(Deserialize)]
        struct M {
            v: Option<i64>,
        }
        #[derive(Deserialize)]
        struct S {
            v: Option<String>,
        }
        #[derive(Deserialize)]
        struct D {
            v: Option<u64>,
        }
        let q = |sql: &str| Query { sql: sql.into(), params: Vec::new() };
        // Each read is separate: an engine or user may allow some and not others.
        let mem: Option<Vec<M>> =
            self.rows(q("SELECT value AS v FROM system.metrics WHERE metric = 'MemoryTracking'")).await.ok();
        let limit: Option<Vec<S>> = self
            .rows(q("SELECT value AS v FROM system.server_settings WHERE name = 'max_server_memory_usage'"))
            .await
            .ok();
        let disk: Option<Vec<D>> = self.rows(q("SELECT min(free_space) AS v FROM system.disks")).await.ok();
        Ok(EngineStats {
            memory_used_bytes: mem.and_then(|r| r.first()?.v).and_then(|v| u64::try_from(v).ok()),
            server_memory_limit_bytes: limit.and_then(|r| r.first()?.v.as_deref()?.parse().ok()),
            free_disk_bytes: disk.and_then(|r| r.first()?.v),
        })
    }

    async fn health(&self) -> DomainResult<()> {
        self.send(None, Query { sql: "SELECT 1".into(), params: Vec::new() }, None).await.map(drop)
    }

    async fn write_node_minutes(&self, rows: &[NodeMinute]) -> DomainResult<()> {
        let rows: Vec<MinuteRow> = rows.iter().map(MinuteRow::from).collect();
        self.insert(NODE_MINUTES, &rows).await
    }

    async fn write_top_processes(&self, rows: &[NodeProcessMinute]) -> DomainResult<()> {
        let rows: Vec<ProcRow<'_>> = rows.iter().map(ProcRow::from).collect();
        self.insert(TOP_PROCESSES, &rows).await
    }

    async fn node_series(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        resolution: Resolution,
    ) -> DomainResult<Vec<NodePoint>> {
        let sql = match resolution {
            Resolution::Minute => {
                "SELECT toUnixTimestamp(minute) * 1000 AS t, avg(cpu_avg) AS cpu_avg, max(cpu_max) AS cpu_max,
                    toUInt64(avg(mem_used)) AS mem_used, max(mem_total) AS mem_total,
                    toUInt64(avg(disk_used)) AS disk_used, max(disk_total) AS disk_total,
                    toUInt64(avg(rx)) AS rx, toUInt64(avg(tx)) AS tx, avg(load1) AS load1
                FROM node_metrics_1m
                WHERE server_id = {server:UUID} AND minute >= {from:Int64} AND minute < {to:Int64}
                GROUP BY minute ORDER BY minute"
            }
            Resolution::Hour => {
                "SELECT toUnixTimestamp(hour) * 1000 AS t, avgMerge(cpu_avg) AS cpu_avg, maxMerge(cpu_max) AS cpu_max,
                    toUInt64(avgMerge(mem_used)) AS mem_used, maxMerge(mem_total) AS mem_total,
                    toUInt64(avgMerge(disk_used)) AS disk_used, maxMerge(disk_total) AS disk_total,
                    toUInt64(avgMerge(rx)) AS rx, toUInt64(avgMerge(tx)) AS tx, avgMerge(load1) AS load1
                FROM node_metrics_1h
                WHERE server_id = {server:UUID} AND hour >= {from:Int64} AND hour < {to:Int64}
                GROUP BY hour ORDER BY hour"
            }
        };
        self.rows(Query {
            sql: sql.to_string(),
            params: vec![
                ("server", server.to_string()),
                ("from", secs(from).to_string()),
                ("to", secs(to).to_string()),
            ],
        })
        .await
    }

    async fn top_processes(
        &self,
        server: ServerId,
        from: Timestamp,
        to: Timestamp,
        by: TopBy,
        limit: u32,
    ) -> DomainResult<Vec<ProcessUsage>> {
        let order = match by {
            TopBy::Cpu => "cpu_avg",
            TopBy::Mem => "mem_avg",
        };
        let mut sql = String::from(
            "SELECT name, user, avg(cpu) AS cpu_avg, max(cpu) AS cpu_max,
                toUInt64(avg(mem)) AS mem_avg, max(mem) AS mem_max,
                toUInt32(uniqExact(minute)) AS minutes, any(cmd) AS cmd
            FROM node_top_processes_1m
            WHERE server_id = {server:UUID} AND minute >= {from:Int64} AND minute < {to:Int64}
            GROUP BY name, user",
        );
        // `order` is one of two fixed column names, not input.
        let _ = write!(sql, " ORDER BY {order} DESC LIMIT {{limit:UInt32}}");
        self.rows(Query {
            sql,
            params: vec![
                ("server", server.to_string()),
                ("from", secs(from).to_string()),
                ("to", secs(to).to_string()),
                ("limit", limit.clamp(1, 100).to_string()),
            ],
        })
        .await
    }

    async fn write_container_minutes(&self, rows: &[ContainerMinute]) -> DomainResult<()> {
        let rows: Vec<ContainerRow<'_>> = rows.iter().map(ContainerRow::from).collect();
        self.insert(CONTAINER_MINUTES, &rows).await
    }

    async fn container_series(
        &self,
        server: ServerId,
        container: Option<&str>,
        window: (Timestamp, Timestamp),
        resolution: Resolution,
        limit: u32,
    ) -> DomainResult<Vec<ContainerSeries>> {
        let (table, col, select) = match resolution {
            Resolution::Minute => (
                "container_metrics_1m",
                "minute",
                "argMax(container_id, minute) AS container_id, argMax(image, minute) AS image,
                argMax(service_id, minute) AS service_id, argMax(stack, minute) AS stack,
                avg(cpu) AS cpu_avg, max(cpu) AS cpu_max, toUInt64(avg(mem_used)) AS mem_used,
                max(mem_limit) AS mem_limit, toUInt64(avg(rx)) AS rx, toUInt64(avg(tx)) AS tx,
                toUInt64(avg(block_read)) AS block_read, toUInt64(avg(block_write)) AS block_write,
                max(restarts) AS restarts",
            ),
            Resolution::Hour => (
                "container_metrics_1h",
                "hour",
                "argMaxMerge(container_id) AS container_id, argMaxMerge(image) AS image,
                argMaxMerge(service_id) AS service_id, argMaxMerge(stack) AS stack,
                avgMerge(cpu_avg) AS cpu_avg, maxMerge(cpu_max) AS cpu_max, toUInt64(avgMerge(mem_used)) AS mem_used,
                maxMerge(mem_limit) AS mem_limit, toUInt64(avgMerge(rx)) AS rx, toUInt64(avgMerge(tx)) AS tx,
                toUInt64(avgMerge(block_read)) AS block_read, toUInt64(avgMerge(block_write)) AS block_write,
                maxMerge(restarts) AS restarts",
            ),
        };
        // `table`, `col` and `select` are fixed text chosen above, not input.
        let mut sql = format!(
            "SELECT container_name, toUnixTimestamp({col}) * 1000 AS t, {select}
            FROM {table}
            WHERE server_id = {{server:UUID}} AND {col} >= {{from:Int64}} AND {col} < {{to:Int64}}"
        );
        let mut params = vec![
            ("server", server.to_string()),
            ("from", secs(window.0).to_string()),
            ("to", secs(window.1).to_string()),
            ("limit", limit.clamp(1, 500).to_string()),
        ];
        match container {
            Some(name) => {
                sql.push_str(" AND container_name = {name:String}");
                params.push(("name", name.to_owned()));
            }
            None => sql.push_str(&format!(
                " AND container_name IN (SELECT DISTINCT container_name FROM {table}
                    WHERE server_id = {{server:UUID}} AND {col} >= {{from:Int64}} AND {col} < {{to:Int64}}
                    ORDER BY container_name LIMIT {{limit:UInt32}})"
            )),
        }
        let _ = write!(sql, " GROUP BY container_name, {col} ORDER BY container_name, {col}");
        let rows: Vec<ContainerOut> = self.rows(Query { sql, params }).await?;
        Ok(group_series(rows))
    }

    async fn write_logs(&self, events: &[LogEvent]) -> DomainResult<()> {
        let rows: Vec<LogRow<'_>> = events.iter().map(LogRow::from).collect();
        self.insert(APP_LOGS, &rows).await
    }

    async fn query_logs(&self, q: &LogQuery) -> DomainResult<Vec<LogEvent>> {
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let (mut wh, mut params) = log_where(q);
        let (cursor, order) = match q.page {
            LogPage::Older(c) => (c.map(|c| (c, "<")), "DESC"),
            LogPage::Newer(c) => (Some((c, ">")), "ASC"),
        };
        if let Some((c, op)) = cursor {
            // `op` is one of two fixed comparison operators.
            let _ = write!(wh, " AND (ts, id) {op} (fromUnixTimestamp64Milli({{cts:Int64}}), {{cid:UUID}})");
            params.push(("cts".into(), c.ts.to_string()));
            params.push(("cid".into(), c.id.to_string()));
        }
        params.push(("limit".into(), q.limit.clamp(1, 1000).to_string()));
        // `order` is one of two fixed words.
        let sql = format!(
            "SELECT {LOG_COLUMNS} FROM app_logs WHERE {wh} ORDER BY ts {order}, id {order} LIMIT {{limit:UInt32}}"
        );
        let params: Vec<(&str, String)> = params.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let rows: Vec<LogOut> = self.rows_with(Query { sql, params }, &search_settings(q)).await?;
        let mut out: Vec<LogEvent> = rows.into_iter().map(LogOut::into_event).collect();
        if matches!(q.page, LogPage::Newer(_)) {
            out.reverse(); // newest first, like every page
        }
        Ok(out)
    }

    async fn log_histogram(&self, q: &LogQuery, step_ms: i64) -> DomainResult<Vec<LogBucket>> {
        #[derive(Deserialize)]
        struct B {
            t: i64,
            level: String,
            count: u64,
        }
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let (wh, mut params) = log_where(q);
        params.push(("step".into(), step_ms.max(1000).to_string()));
        let sql = format!(
            "SELECT intDiv(toUnixTimestamp64Milli(ts), {{step:Int64}}) * {{step:Int64}} AS t, level, count() AS count
             FROM app_logs WHERE {wh} GROUP BY t, level ORDER BY t"
        );
        let params: Vec<(&str, String)> = params.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let rows: Vec<B> = self.rows_with(Query { sql, params }, &search_settings(q)).await?;
        Ok(rows
            .into_iter()
            .filter_map(|b| LogLevel::parse_lenient(&b.level).map(|level| LogBucket { t: b.t, level, count: b.count }))
            .collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn parses_target() {
        let t = ClickHouseTarget::from_url("https://ana@ch.example.com/forge_op_analytics", Some("pw"), "clickhouse")
            .unwrap();
        assert_eq!(t.endpoint, "https://ch.example.com:443");
        assert_eq!(t.user, "ana");
        assert_eq!(t.database, "forge_op_analytics");
        let t = ClickHouseTarget::from_url("http://127.0.0.1:8123/x", None, "embedded").unwrap();
        assert_eq!(t.user, "default");
        assert!(format!("{t:?}").contains("finish") || !format!("{t:?}").contains("pw"));
        assert!(ClickHouseTarget::from_url("http://h/bad-name", None, "clickhouse").is_err());
        assert!(ClickHouseTarget::from_url("postgres://h/x", None, "clickhouse").is_err());
    }

    #[test]
    fn fills_retention() {
        let r = Retention { node_metrics_days: 7, top_processes_days: 7, node_rollup_days: 365, app_logs_days: 30 };
        let s = fill(MIGRATIONS[0].1[0], r);
        assert!(s.contains("INTERVAL 7 DAY"));
        assert!(!s.contains('{') || s.contains("{server"));
        let top = fill(TTLS[1].2, Retention { top_processes_days: 3, ..r });
        assert_eq!(top, "3");
    }

    #[test]
    fn parameters_travel_in_a_form_body_of_any_length() {
        // A scope of 5,000 nodes: far past the 64 KiB a URL may hold.
        let ids = format!("[{}]", vec!["'00000000-0000-0000-0000-000000000001'"; 5_000].join(","));
        let body = form("SELECT {open:Array(UUID)}", &[("open", ids.clone())]).unwrap();
        assert!(body.len() > 64 * 1024);
        assert!(body.starts_with(&format!(
            "--{FORM_BOUNDARY}\r\nContent-Disposition: form-data; name=\"query\"\r\n\r\nSELECT"
        )));
        assert!(body.contains(&format!("name=\"param_open\"\r\n\r\n{ids}\r\n")));
        assert!(body.ends_with(&format!("--{FORM_BOUNDARY}--\r\n")));
        assert!(FORM_CONTENT_TYPE.ends_with(FORM_BOUNDARY));
        assert!(form("SELECT 1", &[("q", format!("x{FORM_BOUNDARY}"))]).is_err(), "a value cannot end the form");
    }

    #[test]
    fn reads_ttl_days() {
        let full = "ReplacingMergeTree PARTITION BY toYYYYMMDD(minute) ORDER BY (server_id, minute) TTL minute + toIntervalDay(14) SETTINGS index_granularity = 8192";
        assert_eq!(ttl_days(full), Some(14));
        assert_eq!(ttl_days("MergeTree ORDER BY version SETTINGS index_granularity = 8192"), None);
    }
}
