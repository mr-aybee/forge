//! Output: human views (tables, key/value), or a versioned machine envelope.
//!
//! Data goes to stdout; prompts, progress and human error text to stderr.
//! With `--output json|yaml|xml`, stdout always holds exactly one document:
//! `{"schema":"forge.cli/v1","result":…}` on success, or
//! `{"schema":"forge.cli/v1","error":{…}}` on failure (the exit code says
//! which). JSON is pretty-printed on a terminal and compact when piped.
//!
//! The default `table` view never prints raw JSON: lists are tables, single
//! objects a key/value view with nested sections and epoch-millisecond
//! timestamps shown as local date-times. Colour (status words, headers, the
//! `error:` prefix) is used only on a terminal, never with `NO_COLOR`, a
//! pipe or a machine format; `--color always|never` overrides. Long human
//! output opens in a pager on a terminal (`$PAGER`, else `less` on Unix and
//! a built-in pager on Windows); `--no-pager` turns it off.

use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};
use std::sync::OnceLock;

use clap::{Args, ValueEnum};
use serde_json::{Map, Value, json};

use crate::error::CliError;

pub const SCHEMA: &str = "forge.cli/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum Format {
    /// Tables and key/value views for people.
    #[default]
    Table,
    /// The JSON envelope (pretty on a terminal, compact when piped).
    Json,
    /// The same envelope as YAML.
    Yaml,
    /// The same envelope as XML.
    Xml,
}

impl Format {
    /// A machine-readable document format (one document on stdout).
    #[must_use]
    pub fn is_machine(self) -> bool {
        self != Self::Table
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ColorChoice {
    /// Colour on a terminal, unless `NO_COLOR` is set.
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy)]
struct Settings {
    color_out: bool,
    color_err: bool,
    pager: bool,
    offset: Option<time::UtcOffset>,
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();
static OFFSET: OnceLock<Option<time::UtcOffset>> = OnceLock::new();

/// Reads the local UTC offset. Call before any other thread starts (on Unix
/// the offset can only be read safely while the process is single-threaded).
pub fn capture_local_offset() {
    let _ = OFFSET.set(time::UtcOffset::current_local_offset().ok());
}

fn settings() -> Settings {
    *SETTINGS.get_or_init(|| Settings {
        color_out: false,
        color_err: false,
        pager: false,
        offset: OFFSET.get().copied().flatten(),
    })
}

/// Decides colour and paging once, from the global flags.
pub fn init(fmt: Format, color: ColorChoice, no_pager: bool) {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");

    let want = |tty: bool| match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => tty && !no_color && !dumb,
    };

    let human = !fmt.is_machine();

    let s = Settings {
        color_out: human && want(std::io::stdout().is_terminal()),
        color_err: want(std::io::stderr().is_terminal()),
        pager: human && !no_pager && std::io::stdout().is_terminal() && std::io::stdin().is_terminal(),
        offset: OFFSET.get().copied().flatten(),
    };

    #[cfg(windows)]
    if s.color_out || s.color_err {
        let _ = anstyle_query::windows::enable_ansi_colors();
    }
    let _ = SETTINGS.set(s);
}

// ------------------------------------------------------------------ styling

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

fn paint(on: bool, style: &str, s: &str) -> String {
    if on && !s.is_empty() { format!("{style}{s}{RESET}") } else { s.to_owned() }
}

/// Colour of a status word, if it is one.
#[must_use]
pub fn status_style(word: &str) -> Option<&'static str> {
    match word.to_ascii_lowercase().as_str() {
        "online" | "succeeded" | "success" | "ok" | "active" | "deployed" | "healthy" | "enabled" | "applied"
        | "ready" | "up" | "connected" | "passing" | "valid" | "issued" | "installed" | "live" => Some(GREEN),
        "pending" | "queued" | "running" | "deploying" | "starting" | "stopping" | "updating" | "in_progress"
        | "degraded" | "warning" | "warn" | "waiting" | "inactive" | "stale" | "paused" | "expiring" => Some(YELLOW),
        "offline" | "failed" | "failure" | "denied" | "error" | "cancelled" | "canceled" | "revoked" | "unhealthy"
        | "critical" | "down" | "expired" | "broken" | "unreachable" | "firing" => Some(RED),
        _ => None,
    }
}

/// Keys whose values are status words.
fn is_status_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    matches!(k.as_str(), "status" | "state" | "outcome" | "health" | "result" | "phase" | "severity" | "ok")
        || k.ends_with("_status")
        || k.ends_with("_state")
}

/// Red `error:` (on stderr's terminal).
#[must_use]
pub fn error_prefix() -> String {
    paint(settings().color_err, &format!("{BOLD}{RED}"), "error:")
}

// ------------------------------------------------------------------ values

/// Whether `key` holds an epoch-millisecond timestamp.
fn is_time_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k == "at"
        || k.ends_with("_at")
        || k.ends_with("_ms")
        || ["time", "since", "until", "expires", "last_seen", "created", "updated", "timestamp", "before"]
            .iter()
            .any(|w| k.contains(w))
}

/// Epoch milliseconds (years 2000 to 2100) as a local `YYYY-MM-DD HH:MM:SS`.
#[must_use]
pub fn datetime(ms: i64, offset: Option<time::UtcOffset>) -> Option<String> {
    if !(946_684_800_000..4_102_444_800_000).contains(&ms) {
        return None;
    }

    let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()?;
    let t = t.to_offset(offset.unwrap_or(time::UtcOffset::UTC));
    let zone = if offset.is_none_or(|o| o.is_utc()) { " UTC".to_owned() } else { String::new() };

    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}{zone}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    ))
}

/// A scalar as text; timestamps as date-times when `key` says so.
fn scalar(key: &str, v: &Value, offset: Option<time::UtcOffset>) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
        Value::Number(n) => match n.as_i64().filter(|_| is_time_key(key)).and_then(|ms| datetime(ms, offset)) {
            Some(d) => d,
            None => n.to_string(),
        },
        Value::Array(a) => a.iter().map(|x| scalar(key, x, offset)).collect::<Vec<_>>().join(", "),
        Value::Object(_) => v.to_string(),
    }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

// ------------------------------------------------------------------ tables

/// A table column: header and a JSON pointer into each row.
pub type Col = (&'static str, &'static str);

fn clip(s: &str, max: usize) -> String {
    let s = s.replace(['\n', '\r'], " ");
    if s.chars().count() <= max { s } else { format!("{}…", s.chars().take(max - 1).collect::<String>()) }
}

/// Renders rows as an aligned text table (plain text).
#[cfg(test)]
#[must_use]
pub fn table(rows: &[Value], cols: &[Col]) -> String {
    table_styled(rows, cols, false, None)
}

fn table_styled<H: AsRef<str>, P: AsRef<str>>(
    rows: &[Value],
    cols: &[(H, P)],
    color: bool,
    offset: Option<time::UtcOffset>,
) -> String {
    let key_of = |p: &str| p.rsplit('/').next().unwrap_or_default().to_owned();
    let data: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            cols.iter()
                .map(|(_, p)| {
                    let p = p.as_ref();
                    clip(&scalar(&key_of(p), r.pointer(p).unwrap_or(&Value::Null), offset), 60)
                })
                .collect()
        })
        .collect();
    let widths: Vec<usize> = cols
        .iter()
        .enumerate()
        .map(|(i, (h, _))| data.iter().map(|r| r[i].chars().count()).chain([h.as_ref().len()]).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    let last = cols.len().saturating_sub(1);
    let header: Vec<String> = cols
        .iter()
        .zip(&widths)
        .enumerate()
        .map(|(i, ((h, _), w))| {
            let h = h.as_ref();
            let cell = if i == last { h.to_owned() } else { format!("{h:<w$}") };
            paint(color, BOLD, &cell)
        })
        .collect();
    out.push_str(header.join("  ").trim_end());
    out.push('\n');

    for r in &data {
        let cells: Vec<String> = r
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(i, (c, w))| {
                let padded = if i == last { c.clone() } else { format!("{c:<w$}") };
                let status_col = is_status_key(&key_of(cols[i].1.as_ref())) || is_status_key(cols[i].0.as_ref());
                match status_style(c).filter(|_| color && status_col) {
                    Some(st) => padded.replacen(c.as_str(), &format!("{st}{c}{RESET}"), 1),
                    None => padded,
                }
            })
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }

    out
}

/// Columns for a list without defined ones: the first row's scalar fields.
fn auto_cols(rows: &[Value]) -> Vec<(String, String)> {
    let Some(Value::Object(first)) = rows.first() else { return Vec::new() };
    first
        .iter()
        .filter(|(_, v)| is_scalar(v) || v.as_array().is_some_and(|a| a.iter().all(is_scalar)))
        .take(8)
        .map(|(k, _)| (k.to_uppercase(), format!("/{k}")))
        .collect()
}

// ------------------------------------------------------------------ key/value view

/// Renders a value for people: objects as aligned `key  value` lines with
/// nested sections, lists of flat objects as indented tables.
fn human_with(v: &Value, color: bool, offset: Option<time::UtcOffset>) -> String {
    let mut out = String::new();

    match v {
        Value::Object(m) => kv(m, 0, color, offset, &mut out),
        Value::Array(rows) if rows.iter().all(Value::is_object) && !rows.is_empty() => {
            let cols = auto_cols(rows);
            if cols.is_empty() || rows.iter().any(|r| r.as_object().is_some_and(|o| o.values().any(|x| x.is_object())))
            {
                for (i, r) in rows.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }

                    if let Value::Object(m) = r {
                        kv(m, 0, color, offset, &mut out);
                    }
                }
            } else {
                out.push_str(&table_styled(rows, &cols, color, offset));
            }
        }
        Value::Array(a) => {
            for x in a {
                let _ = writeln!(out, "{}", scalar("", x, offset));
            }
        }
        Value::Null => {}
        other => {
            let _ = writeln!(out, "{}", scalar("", other, offset));
        }
    }

    out
}

fn kv(m: &Map<String, Value>, indent: usize, color: bool, offset: Option<time::UtcOffset>, out: &mut String) {
    let pad = " ".repeat(indent);
    let shown: Vec<(&String, &Value)> = m.iter().filter(|(_, v)| !is_empty(v)).collect();
    let width = shown
        .iter()
        .filter(|(_, v)| is_scalar(v) || v.as_array().is_some_and(|a| a.iter().all(is_scalar)))
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);

    for (k, v) in shown {
        match v {
            Value::Object(inner) => {
                let _ = writeln!(out, "{pad}{}", paint(color, BOLD, &format!("{k}:")));
                kv(inner, indent + 2, color, offset, out);
            }
            Value::Array(items) if !items.iter().all(is_scalar) => {
                let _ = writeln!(out, "{pad}{}", paint(color, BOLD, &format!("{k}:")));
                let flat = items.iter().all(|i| i.as_object().is_some_and(|o| o.values().all(is_scalar)));
                let cols = if flat { auto_cols(items) } else { Vec::new() };
                if !cols.is_empty() {
                    for line in table_styled(items, &cols, color, offset).lines() {
                        let _ = writeln!(out, "{pad}  {line}");
                    }
                } else {
                    for item in items {
                        match item {
                            Value::Object(o) => {
                                let mut sub = String::new();
                                kv(o, indent + 4, color, offset, &mut sub);
                                let mut lines = sub.lines();
                                if let Some(first) = lines.next() {
                                    let _ = writeln!(out, "{pad}  - {}", first.trim_start());
                                }

                                for l in lines {
                                    let _ = writeln!(out, "{l}");
                                }
                            }
                            other => {
                                let _ = writeln!(out, "{pad}  - {}", scalar(k, other, offset));
                            }
                        }
                    }
                }
            }
            Value::String(s) if s.contains('\n') => {
                let _ = writeln!(out, "{pad}{}", paint(color, BOLD, &format!("{k}:")));

                for l in s.lines() {
                    let _ = writeln!(out, "{pad}  {l}");
                }
            }
            _ => {
                let text = scalar(k, v, offset);
                let text = match status_style(&text).filter(|_| color && is_status_key(k)) {
                    Some(st) => format!("{st}{text}{RESET}"),
                    None => text,
                };

                let key = format!("{k:<width$}");
                let _ = writeln!(out, "{pad}{}  {text}", paint(color, CYAN, &key));
            }
        }
    }
}

// ------------------------------------------------------------------ YAML / XML

fn yaml_str(s: &str) -> String {
    let plain = !s.is_empty()
        && !s.starts_with([' ', '-', '?', ':', '!', '&', '*', '#', '|', '>', '%', '@', '`', '"', '\'', '[', '{'])
        && !s.ends_with(' ')
        && !s.contains([':', '#', '\n', '\r', '\t', '{', '}', '[', ']', ',', '"', '\\'])
        && !matches!(s.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~")
        && s.parse::<f64>().is_err();
    if plain { s.to_owned() } else { serde_json::to_string(s).unwrap_or_default() }
}

fn yaml_scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => yaml_str(s),
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Object(o) if o.is_empty() => "{}".into(),
        _ => String::new(),
    }
}

fn yaml_inline(v: &Value) -> bool {
    match v {
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => true,
    }
}

fn yaml(v: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);

    match v {
        Value::Object(m) if !m.is_empty() => {
            for (k, x) in m {
                if yaml_inline(x) {
                    let _ = writeln!(out, "{pad}{}: {}", yaml_str(k), yaml_scalar(x));
                } else {
                    let _ = writeln!(out, "{pad}{}:", yaml_str(k));
                    yaml(x, indent + 2, out);
                }
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for x in a {
                if yaml_inline(x) {
                    let _ = writeln!(out, "{pad}- {}", yaml_scalar(x));
                } else {
                    let mut sub = String::new();
                    yaml(x, indent + 2, &mut sub);
                    let _ =
                        writeln!(out, "{pad}- {}", sub.get(indent + 2..).unwrap_or_default().trim_end_matches('\n'));
                }
            }
        }
        other => {
            let _ = writeln!(out, "{pad}{}", yaml_scalar(other));
        }
    }
}

/// A value as a YAML document.
#[must_use]
pub fn to_yaml(v: &Value) -> String {
    let mut out = String::from("---\n");
    yaml(v, 0, &mut out);

    out
}

fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());

    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\r' | '\t') => {}
            c => o.push(c),
        }
    }

    o
}

fn xml_name_ok(k: &str) -> bool {
    let mut cs = k.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !k.to_ascii_lowercase().starts_with("xml")
}

fn xml(name: &str, v: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);

    let (open, close) = if xml_name_ok(name) {
        (name.to_owned(), name.to_owned())
    } else {
        (format!("entry key=\"{}\"", xml_escape(name)), "entry".to_owned())
    };

    match v {
        Value::Null => {
            let _ = writeln!(out, "{pad}<{open} nil=\"true\"/>");
        }
        Value::Object(m) => {
            let _ = writeln!(out, "{pad}<{open}>");

            for (k, x) in m {
                xml(k, x, indent + 1, out);
            }

            let _ = writeln!(out, "{pad}</{close}>");
        }
        Value::Array(a) => {
            let _ = writeln!(out, "{pad}<{open}>");

            for x in a {
                xml("item", x, indent + 1, out);
            }

            let _ = writeln!(out, "{pad}</{close}>");
        }
        Value::String(s) => {
            let _ = writeln!(out, "{pad}<{open}>{}</{close}>", xml_escape(s));
        }
        other => {
            let _ = writeln!(out, "{pad}<{open}>{other}</{close}>");
        }
    }
}

/// The envelope as an XML document: `<forge schema="…"><result>…</result></forge>`.
#[must_use]
pub fn to_xml(envelope: &Value) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(out, "<forge schema=\"{SCHEMA}\">");

    for (k, x) in envelope.as_object().into_iter().flatten().filter(|(k, _)| *k != "schema") {
        xml(k, x, 1, &mut out);
    }

    out.push_str("</forge>\n");

    out
}

/// A machine document for `fmt`.
fn document(fmt: Format, envelope: &Value) -> String {
    match fmt {
        Format::Json | Format::Table => {
            if std::io::stdout().is_terminal() {
                format!("{}\n", serde_json::to_string_pretty(envelope).unwrap_or_default())
            } else {
                format!("{envelope}\n")
            }
        }
        Format::Yaml => to_yaml(envelope),
        Format::Xml => to_xml(envelope),
    }
}

// ------------------------------------------------------------------ printing

/// Prints human text, through the pager when it is longer than the screen.
pub fn emit(text: &str) {
    let s = settings();
    let rows = crossterm::terminal::size().map(|(_, r)| usize::from(r)).unwrap_or(0);
    if s.pager && rows > 2 && text.lines().count() >= rows && page(text, rows) {
        return;
    }

    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// Shows `text` in `$PAGER`, `less` (Unix) or the built-in pager. `false`
/// when no pager could run (the caller prints plainly).
fn page(text: &str, rows: usize) -> bool {
    let pager = std::env::var("FORGE_PAGER").or_else(|_| std::env::var("PAGER")).ok().filter(|p| !p.trim().is_empty());
    let cmd = match pager.as_deref() {
        Some("cat") => return false,
        Some(p) => Some(p.to_owned()),
        None if cfg!(windows) => None,
        None => Some("less".to_owned()),
    };

    if let Some(cmd) = cmd {
        let mut parts = cmd.split_whitespace();
        let Some(prog) = parts.next() else { return false };
        let mut c = std::process::Command::new(prog);
        c.args(parts).stdin(std::process::Stdio::piped());

        if std::env::var_os("LESS").is_none() {
            // Quit when it fits, keep colours, leave the text on screen.
            c.env("LESS", "FRX");
        }

        if let Ok(mut child) = c.spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                // A user quitting early closes the pipe: not an error.
                let _ = stdin.write_all(text.as_bytes());
            }

            let _ = child.wait();

            return true;
        }
    }

    builtin_pager(text, rows)
}

/// A minimal pager: a screen at a time; Space/PgDn next page, Enter/Down
/// one line, `q`/Esc quits.
fn builtin_pager(text: &str, rows: usize) -> bool {
    use crossterm::event::{Event, KeyCode, KeyEventKind, read};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    let lines: Vec<&str> = text.lines().collect();
    let mut stdout = std::io::stdout().lock();
    let mut shown = 0usize;
    let mut step = rows.saturating_sub(1).max(1);

    while shown < lines.len() {
        let end = (shown + step).min(lines.len());

        for l in &lines[shown..end] {
            let _ = writeln!(stdout, "{l}");
        }

        shown = end;

        if shown >= lines.len() {
            break;
        }

        let pct = shown * 100 / lines.len();
        let _ = write!(stdout, "{DIM}-- more ({pct}%) · space: page · enter: line · q: quit --{RESET}");
        let _ = stdout.flush();

        if enable_raw_mode().is_err() {
            let _ = writeln!(stdout);

            for l in &lines[shown..] {
                let _ = writeln!(stdout, "{l}");
            }

            return true;
        }

        let action = loop {
            match read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                    KeyCode::Char('q' | 'Q') | KeyCode::Esc => break None,
                    KeyCode::Char('c') if k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => break None,
                    KeyCode::Enter | KeyCode::Down | KeyCode::Char('j') => break Some(1),
                    KeyCode::Char(' ') | KeyCode::PageDown | KeyCode::Char('f') => break Some(rows - 1),
                    _ => {}
                },
                Ok(_) => {}
                Err(_) => break None,
            }
        };

        let _ = disable_raw_mode();
        let _ = write!(stdout, "\r\x1b[2K");

        match action {
            Some(n) => step = n.max(1),
            None => {
                let _ = stdout.flush();
                return true;
            }
        }
    }

    let _ = stdout.flush();

    true
}

/// Prints a successful result.
pub fn result(fmt: Format, value: &Value, cols: Option<&[Col]>) {
    if fmt.is_machine() {
        let doc = document(fmt, &json!({ "schema": SCHEMA, "result": value }));
        let _ = std::io::stdout().lock().write_all(doc.as_bytes());

        return;
    }

    emit(&render_human(value, cols));
}

/// The human view of `value` (table with `cols`, else key/value).
#[must_use]
pub fn render_human(value: &Value, cols: Option<&[Col]>) -> String {
    let s = settings();

    match (value, cols) {
        (Value::Array(rows), Some(cols)) => table_styled(rows, cols, s.color_out, s.offset),
        // A single object with columns is a one-row table, like a list.
        (Value::Object(_), Some(cols)) => table_styled(std::slice::from_ref(value), cols, s.color_out, s.offset),
        (Value::String(st), _) => format!("{st}\n"),
        _ => human_with(value, s.color_out, s.offset),
    }
}

/// Prints an error (a document on stdout in machine formats, text on stderr otherwise).
pub fn error(fmt: Format, e: &CliError) {
    if fmt.is_machine() {
        let body = json!({
            "schema": SCHEMA,
            "error": {
                "code": e.code,
                "class": e.exit.code(),
                "message": e.message,
                "retryable": e.retryable,
                "request_id": e.request_id,
                "exit_code": e.exit as i32,
            }
        });
        let _ = std::io::stdout().lock().write_all(document(fmt, &body).as_bytes());

        return;
    }

    let _ = writeln!(std::io::stderr().lock(), "{} {} [{}]", error_prefix(), e.message, e.code);
}

// ------------------------------------------------------------------ paging

/// Paging flags of list commands that have no `--all` of their own.
#[derive(Args, Debug, Clone, Copy)]
pub struct PageArgs {
    /// Rows per page (`0`: every row).
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    /// Page to show (1 is the first).
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..))]
    pub page: u64,
}

/// Paging flags of list commands: `--limit`, `--page`, `--all`.
#[derive(Args, Debug, Clone, Copy)]
pub struct Paging {
    #[command(flatten)]
    pub p: PageArgs,
    /// Every row, on one page (same as `--limit 0`).
    #[arg(long, conflicts_with = "page")]
    pub all: bool,
}

/// A page request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// `None`: everything.
    pub limit: Option<usize>,
    /// 1-based.
    pub page: usize,
}

impl From<PageArgs> for Page {
    fn from(a: PageArgs) -> Self {
        Self { limit: (a.limit > 0).then_some(a.limit), page: usize::try_from(a.page).unwrap_or(1).max(1) }
    }
}

impl From<Paging> for Page {
    fn from(a: Paging) -> Self {
        if a.all { Self { limit: None, page: 1 } } else { a.p.into() }
    }
}

impl Page {
    /// Rows a server-side `limit` must return to cover this page.
    #[must_use]
    pub fn fetch(&self) -> Option<usize> {
        self.limit.map(|l| l * self.page)
    }

    /// Cuts `rows` (every row from the first up to at least the end of this
    /// page) to the page; returns the footer text for people.
    pub fn cut(&self, rows: &mut Vec<Value>, total: Total) -> Option<String> {
        let have = rows.len();

        let (count, more) = match total {
            Total::Rows => (have, false),
            Total::AtLeast => (have, true),
            Total::Known(n) => (n.max(have), false),
        };

        let Some(limit) = self.limit else {
            return (have > 0).then(|| format!("{have} rows"));
        };

        let start = limit * (self.page - 1);
        let end = (start + limit).min(have);
        let of = if more { format!("{count}+") } else { count.to_string() };

        if start >= have {
            rows.clear();
            return Some(format!("page {} is empty ({of} rows)", self.page));
        }

        rows.truncate(end);
        rows.drain(..start);
        let further = end < count || more;
        let next = if further { format!(" · next: --page {}", self.page + 1) } else { String::new() };
        let all = if further || self.page > 1 { " · every row: --all" } else { "" };

        Some(format!("showing {}–{end} of {of}{next}{all}", start + 1))
    }
}

/// How many rows a list has in all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Total {
    /// The rows given are all there are.
    Rows,
    /// The server stopped at the requested count: more may exist.
    AtLeast,
    /// The server said how many match.
    Known(usize),
}

/// Prints a list page: the table and, for people, a footer such as
/// `showing 1–50 of 312 · next: --page 2`. Machine formats get the page rows.
pub fn list(fmt: Format, rows: Vec<Value>, cols: Option<&[Col]>, page: Page, total: Total) {
    let mut rows = rows;
    let footer = page.cut(&mut rows, total);
    let value = Value::Array(rows);

    if fmt.is_machine() {
        result(fmt, &value, cols);
        return;
    }

    let mut text = render_human(&value, cols);

    if let Some(f) = footer.filter(|_| page.limit.is_some()) {
        let _ = writeln!(text, "{}", paint(settings().color_out, DIM, &f));
    }

    emit(&text);
}

/// [`list`] for an API answer that holds every row: the array itself, or
/// the array at `pointer` (`""`: the answer is the array).
pub fn list_value(fmt: Format, v: Value, pointer: &str, cols: Option<&[Col]>, page: impl Into<Page>) {
    let rows = if pointer.is_empty() { v } else { v.pointer(pointer).cloned().unwrap_or(Value::Null) };
    let rows = match rows {
        Value::Array(a) => a,
        _ => Vec::new(),
    };

    list(fmt, rows, cols, page.into(), Total::Rows);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_align_and_clip() {
        let rows = vec![json!({"id": "a1", "name": "lab-b", "n": 3}), json!({"id": "b22", "name": null})];
        let t = table(&rows, &[("ID", "/id"), ("NAME", "/name"), ("N", "/n")]);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "ID   NAME   N");
        assert_eq!(lines[1], "a1   lab-b  3");
        assert_eq!(lines[2], "b22  -      -");
        assert_eq!(clip("abcdef", 4), "abc…");
    }

    #[test]
    fn single_object_with_columns_is_a_one_row_table() {
        let v = json!({"suffix": "local", "source": "default"});
        let t = table(std::slice::from_ref(&v), &[("SUFFIX", "/suffix"), ("SOURCE", "/source")]);
        assert_eq!(t, "SUFFIX  SOURCE\nlocal   default\n");
    }

    #[test]
    fn status_columns_are_coloured_only_when_asked() {
        let rows = vec![json!({"name": "failed", "status": "online"})];
        let cols: &[Col] = &[("NAME", "/name"), ("STATUS", "/status")];
        let plain = table_styled(&rows, cols, false, None);
        assert!(!plain.contains('\x1b'));
        let c = table_styled(&rows, cols, true, None);
        assert!(c.contains(&format!("{GREEN}online{RESET}")));

        // A name that happens to be a status word stays plain.
        assert!(c.contains("failed  "));
        assert!(!c.contains(&format!("{RED}failed")));
    }

    #[test]
    fn objects_render_as_key_value_with_sections_and_dates() {
        let v = json!({
            "hostname": "lab-a",
            "status": "online",
            "last_seen_at": 1_700_000_000_000_i64,
            "facts": {"cpus": 2, "os": "debian"},
            "tags": ["a", "b"],
            "empty": "",
            "instances": [{"node": "lab-b", "status": "deployed"}],
        });
        let t = human_with(&v, false, None);
        assert!(t.contains("hostname      lab-a"), "{t}");
        assert!(t.contains("last_seen_at  2023-11-14 22:13:20 UTC"), "{t}");
        assert!(t.contains("facts:\n  cpus  2\n  os    debian"), "{t}");
        assert!(t.contains("tags          a, b"), "{t}");
        assert!(!t.contains("empty"), "empty fields are hidden: {t}");
        assert!(t.contains("instances:\n  NODE   STATUS\n  lab-b  deployed"), "{t}");
        assert!(!t.contains('{'), "no raw JSON: {t}");
    }

    #[test]
    fn yaml_and_xml_documents() {
        // Keys in sorted order: serde_json's `preserve_order` feature is on when
        // the workspace builds together and off for this crate alone, so the
        // expected text must hold either way.
        let v = json!({"result": {"e": [], "list": [{"x": "1"}, "y"], "n": 1, "name": "a: b", "ok": true}, "schema": SCHEMA});
        let y = to_yaml(&v);
        assert_eq!(
            y,
            "---\nresult:\n  e: []\n  list:\n    - x: \"1\"\n    - y\n  n: 1\n  name: \"a: b\"\n  ok: true\nschema: forge.cli/v1\n"
        );
        let x = to_xml(&v);
        assert!(x.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<forge schema=\"forge.cli/v1\">\n"));
        assert!(x.contains("    <name>a: b</name>\n"), "{x}");
        assert!(x.contains("      <item>\n        <x>1</x>\n      </item>\n      <item>y</item>\n"), "{x}");
        assert!(!x.contains("<schema>"));
        assert_eq!(xml_escape("<a&'\">"), "&lt;a&amp;&apos;&quot;&gt;");
        let mut o = String::new();
        xml("1bad key", &json!(null), 0, &mut o);
        assert_eq!(o, "<entry key=\"1bad key\" nil=\"true\"/>\n");
    }

    #[test]
    fn pages_cut_rows_and_say_what_is_next() {
        let rows: Vec<Value> = (1..=120).map(|i| json!({ "n": i })).collect();
        let p = Page { limit: Some(50), page: 1 };
        let mut r = rows.clone();
        assert_eq!(
            p.cut(&mut r, Total::Rows).as_deref(),
            Some("showing 1–50 of 120 · next: --page 2 · every row: --all")
        );
        assert_eq!(r.len(), 50);
        let mut r = rows.clone();
        let f = Page { limit: Some(50), page: 3 }.cut(&mut r, Total::Rows);
        assert_eq!(f.as_deref(), Some("showing 101–120 of 120 · every row: --all"));
        assert_eq!(r[0]["n"], 101);
        let mut r = rows.clone();
        assert_eq!(Page { limit: None, page: 1 }.cut(&mut r, Total::Rows).as_deref(), Some("120 rows"));
        assert_eq!(r.len(), 120);
        let mut r: Vec<Value> = rows[..100].to_vec();
        let f = Page { limit: Some(50), page: 2 }.cut(&mut r, Total::AtLeast);
        assert_eq!(f.as_deref(), Some("showing 51–100 of 100+ · next: --page 3 · every row: --all"));
        let mut r: Vec<Value> = rows[..50].to_vec();
        let f = Page { limit: Some(50), page: 1 }.cut(&mut r, Total::Known(312));
        assert_eq!(f.as_deref(), Some("showing 1–50 of 312 · next: --page 2 · every row: --all"));
        assert_eq!(Page { limit: Some(50), page: 2 }.fetch(), Some(100));
    }

    #[test]
    fn epoch_ms_only_in_range() {
        assert_eq!(datetime(0, None), None);
        assert_eq!(datetime(1_700_000_000_000, None).as_deref(), Some("2023-11-14 22:13:20 UTC"));
        let cet = time::UtcOffset::from_hms(1, 0, 0).ok();
        assert_eq!(datetime(1_700_000_000_000, cet).as_deref(), Some("2023-11-14 23:13:20"));
        assert_eq!(scalar("count", &json!(1_700_000_000_000_i64), None), "1700000000000");
    }
}
