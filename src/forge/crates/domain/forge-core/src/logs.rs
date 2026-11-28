//! Service logs (Seq-compatible, D-128): event model, CLEF parsing,
//! message-template rendering and the console's query syntax. Pure: no I/O.
//!
//! Apps send Compact Log Event Format (CLEF) — newline-delimited JSON
//! objects with `@t`, `@mt`/`@m`, `@l`, `@x`, `@i`, `@tr`, `@sp` and their
//! own properties — or the older Seq raw format `{"Events":[...]}`. Every
//! event belongs to one service, taken from the ingest key.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{LogKeyId, ServerId, ServiceId, Timestamp, UserId};

/// Severity, Serilog's names. Ordered from least to most severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LogLevel {
    Verbose,
    Debug,
    Information,
    Warning,
    Error,
    Fatal,
}

impl LogLevel {
    pub const ALL: [LogLevel; 6] = [
        LogLevel::Verbose,
        LogLevel::Debug,
        LogLevel::Information,
        LogLevel::Warning,
        LogLevel::Error,
        LogLevel::Fatal,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LogLevel::Verbose => "Verbose",
            LogLevel::Debug => "Debug",
            LogLevel::Information => "Information",
            LogLevel::Warning => "Warning",
            LogLevel::Error => "Error",
            LogLevel::Fatal => "Fatal",
        }
    }

    /// Accepts the names used by Serilog, NLog, MEL, log4net, Python and
    /// syslog, in any case. Unknown names are `None`.
    #[must_use]
    pub fn parse_lenient(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "verbose" | "trace" | "vrb" | "trc" => LogLevel::Verbose,
            "debug" | "dbg" => LogLevel::Debug,
            "information" | "info" | "inf" | "notice" => LogLevel::Information,
            "warning" | "warn" | "wrn" => LogLevel::Warning,
            "error" | "err" | "eror" | "fail" => LogLevel::Error,
            "fatal" | "critical" | "crit" | "ftl" | "alert" | "emergency" | "emerg" => LogLevel::Fatal,
            _ => return None,
        })
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LogLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_lenient(s).ok_or_else(|| format!("unknown log level `{s}`"))
    }
}

/// An event's id (UUIDv7).
pub type EventId = uuid::Uuid;

/// Where an event came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSource {
    /// Sent by the app to the Seq-compatible ingest endpoint.
    #[default]
    Ingest,
    /// Collected by the node's agent from a Docker container's output.
    Container,
}

impl LogSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LogSource::Ingest => "ingest",
            LogSource::Container => "container",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ingest" => Some(LogSource::Ingest),
            "container" => Some(LogSource::Container),
            _ => None,
        }
    }
}

/// The output stream of a container line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
}

impl LogStream {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LogStream::Stdout => "stdout",
            LogStream::Stderr => "stderr",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "stdout" => Some(LogStream::Stdout),
            "stderr" => Some(LogStream::Stderr),
            _ => None,
        }
    }
}

/// One stored event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    /// UUIDv7 assigned at ingest; with `ts` it orders and pages events.
    pub id: uuid::Uuid,
    /// The service the event belongs to; `None` for container lines Forge
    /// could not attribute to a service (they belong to their node).
    pub service_id: Option<ServiceId>,
    /// When the app says the event happened (`@t`).
    pub ts: Timestamp,
    /// When the node's agent received it (agent ingest only).
    pub node_ts: Option<Timestamp>,
    /// When Forge stored it; retention counts from here.
    pub ingest_ts: Timestamp,
    pub level: LogLevel,
    /// The rendered message.
    pub message: String,
    /// The template (`@mt`), or the message when the app sent none.
    pub message_template: String,
    pub exception: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    /// `@i`, or a hash of the template: groups events of one kind.
    pub event_type: u32,
    pub server_id: Option<ServerId>,
    /// Every other property, as sent.
    pub properties: Map<String, Value>,
    pub source: LogSource,
    /// Short (12 hex digits) id of the container the line came from.
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    /// Stack (`forge.stack` label, else the compose project), when any.
    pub stack: Option<String>,
    /// stdout or stderr, for container lines.
    pub stream: Option<LogStream>,
}

/// Position in the stream: events are ordered by `(ts, id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogCursor {
    pub ts: i64,
    pub id: uuid::Uuid,
}

impl fmt::Display for LogCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", self.ts, self.id.simple())
    }
}

impl FromStr for LogCursor {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (ts, id) = s.split_once('_').ok_or("invalid cursor")?;
        Ok(Self {
            ts: ts.parse().map_err(|_| "invalid cursor")?,
            id: uuid::Uuid::parse_str(id).map_err(|_| "invalid cursor")?,
        })
    }
}

impl LogEvent {
    #[must_use]
    pub fn cursor(&self) -> LogCursor {
        LogCursor { ts: self.ts.0, id: self.id }
    }
}

/// Which way to page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogPage {
    /// Newest first, optionally older than a cursor.
    Older(Option<LogCursor>),
    /// Events newer than a cursor (live tail), oldest of them first.
    Newer(LogCursor),
}

/// A filtered read of the stream. `services` is always set by the
/// server from what the caller may read; never empty when queried.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogQuery {
    /// Whole services: every event of these, whichever node sent it.
    pub services: Vec<ServiceId>,
    /// Single nodes of a service: only events attributed to that node.
    /// OR'ed with `services`; both empty matches nothing.
    pub instances: Vec<(ServiceId, ServerId)>,
    /// Nodes whose unattributed container lines (no service) the caller
    /// may read (`view` on the node). OR'ed with `services`/`instances`.
    pub open_nodes: Vec<ServerId>,
    /// Filter: only events from these nodes. Empty: every node.
    pub nodes: Vec<ServerId>,
    /// Filter: container name, or a container id prefix.
    pub container: Option<String>,
    /// Filter: stack name.
    pub stack: Option<String>,
    /// Filter: where events came from.
    pub source: Option<LogSource>,
    /// Empty: every level.
    pub levels: Vec<LogLevel>,
    pub filter: LogFilter,
    pub from: Option<Timestamp>,
    pub to: Option<Timestamp>,
    pub page: LogPage,
    pub limit: u32,
    /// Resource limits for this search (D-136).
    pub limits: SearchLimits,
}

impl LogQuery {
    /// Whether the query's scope names nothing the caller may read: such a
    /// query matches no event.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.services.is_empty() && self.instances.is_empty() && self.open_nodes.is_empty()
    }
}

/// Per-search resource limits passed to the analytics engine. `0` leaves
/// the engine's own default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchLimits {
    /// Seconds a search may run before the engine stops it.
    pub timeout_secs: u32,
    /// Threads one search may use.
    pub threads: u32,
    /// Memory one search may use, bytes.
    pub memory_bytes: u64,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self { timeout_secs: 20, threads: 0, memory_bytes: 0 }
    }
}

/// Counts of one level in one time bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogBucket {
    pub t: i64,
    pub level: LogLevel,
    pub count: u64,
}

/// The console's search box: `Name="value"` / `Name=value` tokens filter on
/// a property; the rest is free text matched against message and exception,
/// case-insensitively.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFilter {
    pub text: Option<String>,
    pub properties: Vec<(String, String)>,
}

/// Longest free text and number of property filters accepted.
pub const MAX_FILTER_TEXT: usize = 500;
pub const MAX_PROPERTY_FILTERS: usize = 8;

fn is_prop_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

impl LogFilter {
    /// Parses the search box. Never fails: what is not a property filter is text.
    #[must_use]
    pub fn parse(input: &str) -> Self {
        let mut props = Vec::new();
        let mut text: Vec<String> = Vec::new();
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }

            // Read one token, honouring a double-quoted value after `=`.
            let start = i;
            let mut name_end = None;
            let mut value = String::new();
            let mut quoted = false;

            while i < chars.len() && !chars[i].is_whitespace() {
                if chars[i] == '=' && name_end.is_none() {
                    name_end = Some(i);
                    i += 1;

                    if i < chars.len() && chars[i] == '"' {
                        quoted = true;
                        i += 1;

                        while i < chars.len() && chars[i] != '"' {
                            if chars[i] == '\\' && i + 1 < chars.len() {
                                i += 1;
                            }

                            value.push(chars[i]);
                            i += 1;
                        }

                        i += 1; // closing quote

                        break;
                    }

                    continue;
                }

                if name_end.is_some() {
                    value.push(chars[i]);
                }

                i += 1;
            }

            let token: String = chars[start..i.min(chars.len())].iter().collect();

            match name_end {
                Some(e) if props.len() < MAX_PROPERTY_FILTERS => {
                    let name: String = chars[start..e].iter().collect();
                    if is_prop_name(&name) && (quoted || !value.is_empty()) {
                        props.push((name, value));
                    } else {
                        text.push(token);
                    }
                }
                _ => text.push(token),
            }
        }

        let mut text = text.join(" ");
        if text.len() > MAX_FILTER_TEXT {
            let mut cut = MAX_FILTER_TEXT;

            while !text.is_char_boundary(cut) {
                cut -= 1;
            }

            text.truncate(cut);
        }

        Self { text: (!text.is_empty()).then_some(text), properties: props }
    }
}

/// An ingest key: lets an app write events for one service. Only a
/// hash of the token is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogKey {
    pub id: LogKeyId,
    pub service_id: ServiceId,
    pub name: String,
    /// First characters of the token, to tell keys apart.
    pub prefix: String,
    pub created_at: Timestamp,
    pub created_by: Option<UserId>,
    pub last_used_at: Option<Timestamp>,
}

/// Why a batch was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClefError {
    /// A line is not a JSON object; carries the 1-based line number.
    NotAnObject(usize),
    TooManyEvents(usize),
}

impl fmt::Display for ClefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClefError::NotAnObject(n) => write!(f, "line {n} is not a JSON object"),
            ClefError::TooManyEvents(n) => write!(f, "a batch holds at most {n} events"),
        }
    }
}

/// Events accepted in one request.
pub const MAX_BATCH_EVENTS: usize = 10_000;
/// Longest message, template, exception or property bag kept (bytes).
pub const MAX_FIELD_BYTES: usize = 32 * 1024;

/// What the parser needs besides the body.
#[derive(Debug, Clone, Copy)]
pub struct IngestContext {
    pub service_id: ServiceId,
    pub server_id: Option<ServerId>,
    pub node_ts: Option<Timestamp>,
    pub now: Timestamp,
}

/// Parses a request body: CLEF lines, or `{"Events":[...]}` (Seq raw format).
pub fn parse_batch(body: &str, cx: IngestContext) -> Result<Vec<LogEvent>, ClefError> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{')
        && let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(trimmed)
        && let Some(Value::Array(events)) = o.remove("Events")
    {
        if events.len() > MAX_BATCH_EVENTS {
            return Err(ClefError::TooManyEvents(MAX_BATCH_EVENTS));
        }

        return events
            .into_iter()
            .enumerate()
            .map(|(i, v)| match v {
                Value::Object(o) => Ok(from_raw(o, cx)),
                _ => Err(ClefError::NotAnObject(i + 1)),
            })
            .collect();
    }

    let mut out = Vec::new();

    for (n, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if out.len() >= MAX_BATCH_EVENTS {
            return Err(ClefError::TooManyEvents(MAX_BATCH_EVENTS));
        }

        match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(o)) => out.push(from_clef(o, cx.into())),
            _ => return Err(ClefError::NotAnObject(n + 1)),
        }
    }

    Ok(out)
}

fn clip(mut s: String) -> String {
    if s.len() > MAX_FIELD_BYTES {
        let mut cut = MAX_FIELD_BYTES;

        while !s.is_char_boundary(cut) {
            cut -= 1;
        }

        s.truncate(cut);
        s.push('…');
    }

    s
}

fn take_str(o: &mut Map<String, Value>, k: &str) -> Option<String> {
    match o.remove(k)? {
        Value::String(s) => Some(s),
        Value::Null => None,
        v => Some(v.to_string()),
    }
}

/// Epoch ms from an RFC 3339 / ISO 8601 timestamp (`2024-01-02T03:04:05.678Z`,
/// any fraction length, `Z` or `±hh:mm`). `None` when it cannot be read.
#[must_use]
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.trim().as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let mut v = 0i64;

        for &c in b.get(r)? {
            if !c.is_ascii_digit() {
                return None;
            }

            v = v * 10 + i64::from(c - b'0');
        }

        Some(v)
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);

    if !matches!(b.get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }

    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    let mut ms = 0i64;

    if b.get(i) == Some(&b'.') {
        i += 1;
        let mut digits = 0;

        while let Some(c) = b.get(i).filter(|c| c.is_ascii_digit()) {
            if digits < 3 {
                ms = ms * 10 + i64::from(c - b'0');
            }

            digits += 1;
            i += 1;
        }

        for _ in digits..3 {
            ms *= 10;
        }
    }

    let offset_min = match b.get(i) {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = num(i + 1..i + 3)?;
            let om = if b.get(i + 3) == Some(&b':') { num(i + 4..i + 6)? } else { num(i + 3..i + 5).unwrap_or(0) };
            let m = oh * 60 + om;
            if *sign == b'+' { m } else { -m }
        }
        _ => return None,
    };

    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }

    // Days from civil (Howard Hinnant's algorithm).
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    Some(((days * 86_400 + h * 3_600 + mi * 60 + sec) - offset_min * 60) * 1_000 + ms)
}

/// 32-bit FNV-1a of the template: Seq's "event type" when `@i` is absent.
#[must_use]
pub fn template_hash(template: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;

    for b in template.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }

    h
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        v => v.to_string(),
    }
}

/// Renders a message template: `{Name}`, `{@Name}`, `{$Name}`, `{Name:fmt}`
/// and `{Name,align}` take the property's value; `{{`/`}}` are braces; an
/// unknown name is left as written. Numeric holes (`{0}`) read `@p`-less
/// positional properties named `"0"`, as Serilog sends them.
#[must_use]
pub fn render_template(template: &str, props: &Map<String, Value>) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;

    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push(tail.as_bytes()[0] as char);
            rest = &tail[2..];

            continue;
        }

        if let Some(after) = tail.strip_prefix('}') {
            out.push('}');
            rest = after;

            continue;
        }

        match tail.find('}') {
            Some(end) => {
                let hole = &tail[1..end];
                let name = hole.trim_start_matches(['@', '$']);
                let name = name.split([':', ',']).next().unwrap_or(name);
                match props.get(name) {
                    Some(v) => out.push_str(&display(v)),
                    None => out.push_str(&tail[..=end]),
                }

                rest = &tail[end + 1..];
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }

    out.push_str(rest);

    out
}

fn event_id_from(v: Option<Value>) -> Option<u32> {
    match v? {
        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Value::String(s) => u32::from_str_radix(s.trim_start_matches("0x"), 16).ok(),
        _ => None,
    }
}

/// The context [`finish`] stamps on an event.
#[derive(Debug, Clone, Copy)]
struct Base {
    service_id: Option<ServiceId>,
    server_id: Option<ServerId>,
    node_ts: Option<Timestamp>,
    now: Timestamp,
}

impl From<IngestContext> for Base {
    fn from(cx: IngestContext) -> Self {
        Self { service_id: Some(cx.service_id), server_id: cx.server_id, node_ts: cx.node_ts, now: cx.now }
    }
}

/// Fields read from one incoming event.
struct Parts {
    ts: Option<i64>,
    level: Option<String>,
    template: Option<String>,
    message: Option<String>,
    exception: Option<String>,
    event_id: Option<u32>,
    trace_id: Option<String>,
    span_id: Option<String>,
    props: Map<String, Value>,
}

fn finish(cx: Base, p: Parts) -> LogEvent {
    let Parts { ts, level, template, message, exception, event_id, trace_id, span_id, props } = p;
    let level = level.as_deref().and_then(LogLevel::parse_lenient).unwrap_or(LogLevel::Information);
    let template = template.or_else(|| message.clone()).unwrap_or_default();
    let message = message.unwrap_or_else(|| render_template(&template, &props));
    let mut props = props;

    // Keep the bag bounded: drop the largest values until it fits.
    if Value::Object(props.clone()).to_string().len() > MAX_FIELD_BYTES {
        props = props
            .into_iter()
            .map(|(k, v)| {
                let s = v.to_string();
                if s.len() > 1024 { (k, Value::String(clip(display(&v)).chars().take(1024).collect())) } else { (k, v) }
            })
            .collect();
    }

    LogEvent {
        id: uuid::Uuid::now_v7(),
        service_id: cx.service_id,
        ts: Timestamp(ts.unwrap_or(cx.now.0)),
        node_ts: cx.node_ts,
        ingest_ts: cx.now,
        level,
        event_type: event_id.unwrap_or_else(|| template_hash(&template)),
        message: clip(message),
        message_template: clip(template),
        exception: exception.filter(|s| !s.is_empty()).map(clip),
        trace_id: trace_id.filter(|s| !s.is_empty()),
        span_id: span_id.filter(|s| !s.is_empty()),
        server_id: cx.server_id,
        properties: props,
        source: LogSource::Ingest,
        container_id: None,
        container_name: None,
        stack: None,
        stream: None,
    }
}

fn from_clef(mut o: Map<String, Value>, cx: Base) -> LogEvent {
    let ts = take_str(&mut o, "@t").and_then(|s| parse_rfc3339_ms(&s));
    let level = take_str(&mut o, "@l");
    let template = take_str(&mut o, "@mt");
    let message = take_str(&mut o, "@m");
    let exception = take_str(&mut o, "@x");
    let event_id = event_id_from(o.remove("@i"));
    let trace = take_str(&mut o, "@tr");
    let span = take_str(&mut o, "@sp");
    o.remove("@r"); // renderings: the rendered message is computed here

    // `@@name` escapes a property that starts with `@`.
    let props: Map<String, Value> = o
        .into_iter()
        .filter_map(|(k, v)| {
            if let Some(rest) = k.strip_prefix("@@") {
                Some((format!("@{rest}"), v))
            } else if k.starts_with('@') {
                None
            } else {
                Some((k, v))
            }
        })
        .collect();

    finish(cx, Parts { ts, level, template, message, exception, event_id, trace_id: trace, span_id: span, props })
}

fn from_raw(mut o: Map<String, Value>, cx: IngestContext) -> LogEvent {
    let ts = take_str(&mut o, "Timestamp").and_then(|s| parse_rfc3339_ms(&s));
    let level = take_str(&mut o, "Level");
    let template = take_str(&mut o, "MessageTemplate");
    let message = take_str(&mut o, "RenderedMessage");
    let exception = take_str(&mut o, "Exception");

    let props = match o.remove("Properties") {
        Some(Value::Object(p)) => p,
        _ => Map::new(),
    };

    finish(
        cx.into(),
        Parts { ts, level, template, message, exception, event_id: None, trace_id: None, span_id: None, props },
    )
}

/// What the server knows about one line collected from a container.
#[derive(Debug, Clone)]
pub struct ContainerContext {
    /// The service the container belongs to, when Forge could tell.
    pub service_id: Option<ServiceId>,
    /// The node that sent the line (from its authenticated link).
    pub server_id: ServerId,
    pub container_id: String,
    pub container_name: String,
    pub stack: Option<String>,
    pub now: Timestamp,
}

/// Keys that carry a level in JSON lines, in order of preference.
const JSON_LEVEL_KEYS: [&str; 5] = ["@l", "level", "Level", "lvl", "severity"];
/// Keys that carry the message in JSON lines that are not CLEF.
const JSON_MESSAGE_KEYS: [&str; 4] = ["msg", "message", "Message", "MESSAGE"];

/// Splits the prefix most plain-text loggers write before the message: a
/// leading timestamp (Docker already gave the event its time, so it is
/// dropped) and a level word such as `INFO`, `[WARN]` or `error:`. Returns
/// the level, if any, and the rest of the line. A line that does not start
/// this way is returned whole.
fn plain_prefix(line: &str) -> (Option<LogLevel>, &str) {
    let mut rest = line.trim_start();
    let next = |s: &str| -> (String, usize) {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        (s[..end].to_owned(), end)
    };
    let looks_like_date = |t: &str| {
        let t = t.trim_matches(|c| c == '[' || c == ']');
        let b = t.as_bytes();
        b.len() >= 10
            && b[..4].iter().all(u8::is_ascii_digit)
            && matches!(b[4], b'-' | b'/')
            && b[5..7].iter().all(u8::is_ascii_digit)
    };
    let looks_like_time = |t: &str| {
        let t = t.trim_matches(|c| c == '[' || c == ']' || c == ',');
        let b = t.as_bytes();
        b.len() >= 8 && b[..2].iter().all(u8::is_ascii_digit) && b[2] == b':'
    };
    let (tok, end) = next(rest);
    if looks_like_date(&tok) {
        rest = rest[end..].trim_start();
        let (tok, end) = next(rest);
        if looks_like_time(&tok) {
            rest = rest[end..].trim_start();
        }
    }

    let (tok, end) = next(rest);
    let word = tok.trim_matches(|c: char| matches!(c, '[' | ']' | '(' | ')' | ':' | '|'));
    match LogLevel::parse_lenient(word) {
        // Only the upper-case form or a bracketed/colon form counts, so a
        // sentence that starts with "error" or "notice" keeps its first word.
        Some(level) if word.chars().all(|c| c.is_ascii_uppercase()) || tok != word => {
            (Some(level), rest[end..].trim_start())
        }
        _ => (None, rest),
    }
}

/// One container output line as an event. A CLEF line (`@t`, `@mt` or `@m`)
/// is read like ingest; another JSON object gives its level (`level`, …)
/// and message (`msg`, …), the rest becomes properties; plain text is the
/// message. Without a level, stderr is `Warning` and stdout `Information`.
#[must_use]
pub fn container_event(cx: &ContainerContext, stream: LogStream, ts_ms: i64, line: &str) -> LogEvent {
    let base =
        Base { service_id: cx.service_id, server_id: Some(cx.server_id), node_ts: Some(Timestamp(ts_ms)), now: cx.now };

    let fallback = match stream {
        LogStream::Stdout => LogLevel::Information,
        LogStream::Stderr => LogLevel::Warning,
    };

    let trimmed = line.trim();
    let object = if trimmed.starts_with('{') {
        match serde_json::from_str::<Value>(trimmed) {
            Ok(Value::Object(o)) => Some(o),
            _ => None,
        }
    } else {
        None
    };

    let mut e = match object {
        Some(o) if o.contains_key("@t") || o.contains_key("@mt") || o.contains_key("@m") => {
            let (has_level, has_ts) = (o.contains_key("@l"), o.contains_key("@t"));
            let mut e = from_clef(o, base);

            if !has_level {
                e.level = fallback;
            }

            // A CLEF line without `@t` happened when Docker read it.
            if !has_ts {
                e.ts = Timestamp(ts_ms);
            }

            e
        }
        Some(mut o) => {
            let level = JSON_LEVEL_KEYS.iter().find_map(|k| match o.get(*k) {
                Some(Value::String(s)) => LogLevel::parse_lenient(s),
                _ => None,
            });

            for k in JSON_LEVEL_KEYS {
                if level.is_some() {
                    o.remove(k);
                }
            }

            let message =
                JSON_MESSAGE_KEYS.iter().find_map(|k| take_str(&mut o, k)).unwrap_or_else(|| trimmed.to_owned());
            let mut e = finish(
                base,
                Parts {
                    ts: Some(ts_ms),
                    level: None,
                    template: None,
                    message: Some(message),
                    exception: None,
                    event_id: None,
                    trace_id: None,
                    span_id: None,
                    props: o,
                },
            );
            e.level = level.unwrap_or(fallback);
            e
        }
        None => {
            let (level, rest) = plain_prefix(line);
            let mut e = finish(
                base,
                Parts {
                    ts: Some(ts_ms),
                    level: None,
                    template: None,
                    message: Some(rest.trim_end().to_owned()),
                    exception: None,
                    event_id: None,
                    trace_id: None,
                    span_id: None,
                    props: Map::new(),
                },
            );
            e.level = level.unwrap_or(fallback);
            e
        }
    };

    e.source = LogSource::Container;
    e.container_id = Some(cx.container_id.clone());
    e.container_name = Some(cx.container_name.clone());
    e.stack = cx.stack.clone();
    e.stream = Some(stream);

    e
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn cx() -> IngestContext {
        IngestContext { service_id: ServiceId::new(), server_id: None, node_ts: None, now: Timestamp(1_000) }
    }

    fn ccx(service_id: Option<ServiceId>) -> ContainerContext {
        ContainerContext {
            service_id,
            server_id: ServerId::new(),
            container_id: "0123456789ab".into(),
            container_name: "web".into(),
            stack: None,
            now: Timestamp(9_000),
        }
    }

    #[test]
    fn plain_container_lines_lose_their_timestamp_and_give_a_level() {
        let (l, m) = plain_prefix("2026-11-21T12:00:48.121081Z  INFO forge_agent::agent: executing command");
        assert_eq!((l, m), (Some(LogLevel::Information), "forge_agent::agent: executing command"));
        let (l, m) = plain_prefix("2026/11/21 12:00:48 [error] 29#29: connect() failed");
        assert_eq!((l, m), (Some(LogLevel::Error), "29#29: connect() failed"));
        assert_eq!(plain_prefix("WARN: disk almost full"), (Some(LogLevel::Warning), "disk almost full"));
        assert_eq!(plain_prefix("error opening file"), (None, "error opening file"));
        assert_eq!(plain_prefix("listening on :8080"), (None, "listening on :8080"));
        let e = container_event(&ccx(None), LogStream::Stdout, 5, "2026-11-21T12:00:48Z ERROR boom");
        assert_eq!((e.level, e.message.as_str()), (LogLevel::Error, "boom"));
    }

    #[test]
    fn container_lines_get_a_level_from_json_or_their_stream() {
        let cx = ccx(None);
        let plain = container_event(&cx, LogStream::Stdout, 5_000, "GET / 200\n");
        assert_eq!(
            (plain.level, plain.message.as_str(), plain.ts),
            (LogLevel::Information, "GET / 200", Timestamp(5_000))
        );
        assert_eq!(plain.source, LogSource::Container);
        assert_eq!(plain.container_name.as_deref(), Some("web"));
        assert_eq!(plain.node_ts, Some(Timestamp(5_000)));
        assert_eq!(container_event(&cx, LogStream::Stderr, 5_000, "oops").level, LogLevel::Warning);
        let json = container_event(&cx, LogStream::Stdout, 5_000, r#"{"level":"error","msg":"db down","code":7}"#);
        assert_eq!((json.level, json.message.as_str()), (LogLevel::Error, "db down"));
        assert_eq!(json.properties["code"], 7);
        assert!(!json.properties.contains_key("level"));
        let clef = container_event(
            &cx,
            LogStream::Stderr,
            5_000,
            r#"{"@t":"1970-01-01T00:00:01Z","@mt":"Hi {Who}","@l":"Debug","Who":"x"}"#,
        );
        assert_eq!((clef.level, clef.message.as_str(), clef.ts), (LogLevel::Debug, "Hi x", Timestamp(1_000)));
        let clef = container_event(&cx, LogStream::Stderr, 5_000, r#"{"@m":"no level"}"#);
        assert_eq!((clef.level, clef.ts), (LogLevel::Warning, Timestamp(5_000)), "stream level and Docker's time");
        let not_object = container_event(&cx, LogStream::Stdout, 5_000, "{broken");
        assert_eq!(not_object.message, "{broken");
        let svc = ServiceId::new();
        assert_eq!(container_event(&ccx(Some(svc)), LogStream::Stdout, 1, "x").service_id, Some(svc));
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01Z"), Some(1_000));
        assert_eq!(parse_rfc3339_ms("2024-02-29T12:00:00.1234567Z"), Some(1_709_208_000_123));
        assert_eq!(parse_rfc3339_ms("2024-02-29T14:00:00.5+02:00"), Some(1_709_208_000_500));
        assert_eq!(parse_rfc3339_ms("2024-02-29T10:30:00-01:30"), Some(1_709_208_000_000));
        assert_eq!(parse_rfc3339_ms("nope"), None);
    }

    #[test]
    fn renders_templates() {
        let mut p = Map::new();
        p.insert("User".into(), Value::String("ann".into()));
        p.insert("Elapsed".into(), serde_json::json!(12.5));
        p.insert("0".into(), serde_json::json!(7));
        assert_eq!(
            render_template("{@User} took {Elapsed:0.0} ms {{x}} {Missing} {0}", &p),
            "ann took 12.5 ms {x} {Missing} 7"
        );
        assert_eq!(render_template("open {brace", &p), "open {brace");
    }

    #[test]
    fn parses_clef_lines() {
        let body = r#"{"@t":"2024-01-01T00:00:00Z","@mt":"Hello {Name}","Name":"world","@l":"Warning","@x":"boom","@tr":"abc","@@odd":1}

{"@m":"plain"}"#;
        let e = parse_batch(body, cx()).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].message, "Hello world");
        assert_eq!(e[0].level, LogLevel::Warning);
        assert_eq!(e[0].exception.as_deref(), Some("boom"));
        assert_eq!(e[0].trace_id.as_deref(), Some("abc"));
        assert_eq!(e[0].ts, Timestamp(1_704_067_200_000));
        assert!(e[0].properties.contains_key("@odd"));
        assert!(!e[0].properties.contains_key("@tr"));
        assert_eq!(e[1].level, LogLevel::Information);
        assert_eq!(e[1].ts, Timestamp(1_000));
        assert_eq!(e[1].event_type, template_hash("plain"));
        assert_eq!(parse_batch("{\"a\":1}\nnot json", cx()), Err(ClefError::NotAnObject(2)));
    }

    #[test]
    fn parses_raw_format() {
        let body = r#"{"Events":[{"Timestamp":"2024-01-01T00:00:00Z","Level":"Error","MessageTemplate":"Failed {Id}","Properties":{"Id":5}}]}"#;
        let e = parse_batch(body, cx()).unwrap();
        assert_eq!(e[0].message, "Failed 5");
        assert_eq!(e[0].level, LogLevel::Error);
    }

    #[test]
    fn parses_filters() {
        let f = LogFilter::parse(r#"timeout RequestPath="/api/x y" Status=500 a=  more"#);
        assert_eq!(f.text.as_deref(), Some("timeout a= more"));
        assert_eq!(f.properties, vec![("RequestPath".into(), "/api/x y".into()), ("Status".into(), "500".into())]);
        assert_eq!(LogFilter::parse("   "), LogFilter::default());
    }

    #[test]
    fn levels_and_cursors() {
        assert_eq!(LogLevel::parse_lenient("WARN"), Some(LogLevel::Warning));
        assert_eq!(LogLevel::parse_lenient("critical"), Some(LogLevel::Fatal));
        let c = LogCursor { ts: 5, id: uuid::Uuid::now_v7() };
        assert_eq!(c.to_string().parse::<LogCursor>().unwrap(), c);
        assert!("x".parse::<LogCursor>().is_err());
    }
}
