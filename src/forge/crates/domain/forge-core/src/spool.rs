//! The agent spool file (D-142): one unit of analytics data from a node.
//!
//! A file is zstd-compressed NDJSON named `<node-id>-<file-id>.ndjson.zst`,
//! where the file id is a time-ordered UUIDv7 in 32 hex digits. Its first
//! line is a [`FileHeader`]; every later line is one [`Row`], tagged by its
//! kind in `"k"`. The agent writes rows without parsing application
//! events: a request body to the local listener is one [`AppLog`] row
//! holding the body as a string. The worker reads, checks and stores the
//! rows; the node id and the services come from the worker's trust rules,
//! never from the file.

use serde::{Deserialize, Serialize};

/// Version of the file format in [`FileHeader::v`].
pub const VERSION: u32 = 1;
/// Suffix of a sealed spool file.
pub const SUFFIX: &str = ".ndjson.zst";
/// Header carrying the file id of an upload.
pub const FILE_ID_HEADER: &str = "x-forge-spool-file";
/// Most bytes of one uploaded (compressed) file the worker accepts.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Most bytes one file holds once decompressed; the worker stops reading
/// past this.
pub const MAX_RAW_BYTES: u64 = 64 * 1024 * 1024;

/// First line of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader {
    pub v: u32,
    /// The node that wrote it, as the node believes; the worker uses the
    /// node of the client certificate instead.
    pub node: String,
    /// When the file was started, epoch ms (node clock).
    pub created: i64,
    pub counts: Counts,
}

/// Rows of each kind in a file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub log: u64,
    #[serde(default)]
    pub container_log: u64,
    #[serde(default)]
    pub node_minute: u64,
    #[serde(default)]
    pub top_process: u64,
    #[serde(default)]
    pub container_minute: u64,
}

impl Counts {
    pub fn add(&mut self, kind: Kind) {
        let c = match kind {
            Kind::Log => &mut self.log,
            Kind::ContainerLog => &mut self.container_log,
            Kind::NodeMinute => &mut self.node_minute,
            Kind::TopProcess => &mut self.top_process,
            Kind::ContainerMinute => &mut self.container_minute,
        };

        *c = c.saturating_add(1);
    }

    #[must_use]
    pub fn total(&self) -> u64 {
        self.log + self.container_log + self.node_minute + self.top_process + self.container_minute
    }
}

/// The kind of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Log,
    ContainerLog,
    NodeMinute,
    TopProcess,
    ContainerMinute,
}

/// One line after the header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Row {
    Log(AppLog),
    ContainerLog(ContainerLine),
    NodeMinute(NodeMinuteRow),
    TopProcess(TopProcessRow),
    ContainerMinute(ContainerMinuteRow),
}

impl Row {
    #[must_use]
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Log(_) => Kind::Log,
            Self::ContainerLog(_) => Kind::ContainerLog,
            Self::NodeMinute(_) => Kind::NodeMinute,
            Self::TopProcess(_) => Kind::TopProcess,
            Self::ContainerMinute(_) => Kind::ContainerMinute,
        }
    }
}

/// Format of an application's request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyFormat {
    /// The body as the app sent it to `/api/events/raw` (CLEF lines or a
    /// Seq `{"Events":[…]}` document); the worker tells them apart.
    Raw,
}

/// One request an application sent to the node's listener.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppLog {
    /// SHA-256 (hex) of the ingest key the request carried.
    pub key: String,
    /// When the node received it, epoch ms.
    pub ts: i64,
    pub fmt: BodyFormat,
    /// The request body, unparsed.
    pub body: String,
}

/// Output stream of a container line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// The labels Forge attributes a container by (empty when absent).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Labels {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub forge_app: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub forge_stack: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compose_project: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compose_service: String,
}

/// One line a container wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerLine {
    /// Docker's timestamp, epoch ms.
    pub ts: i64,
    pub stream: Stream,
    /// Short container id.
    pub cid: String,
    pub name: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub labels: Labels,
    pub line: String,
}

/// A node's resources over one minute (the averages of its samples).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeMinuteRow {
    /// Start of the minute, epoch ms.
    pub minute: i64,
    pub cpu_avg: f32,
    pub cpu_max: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub rx: u64,
    pub tx: u64,
    pub load1: f32,
    pub samples: u32,
}

/// One process of a node's top list for one minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopProcessRow {
    pub minute: i64,
    pub pid: u32,
    pub name: String,
    pub user: String,
    pub cpu: f32,
    pub mem: u64,
    pub cmd: String,
}

/// One container's resources for one minute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerMinuteRow {
    pub minute: i64,
    pub cid: String,
    pub name: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub labels: Labels,
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub rx: u64,
    pub tx: u64,
    pub block_read: u64,
    pub block_write: u64,
    pub restarts: u32,
}

/// A spool file's name, split into node and file id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileName {
    pub node: uuid::Uuid,
    /// 32 lowercase hex digits (a UUIDv7).
    pub id: String,
}

impl FileName {
    /// A new file of `node`, ordered by `now_ms` among the node's files.
    #[must_use]
    pub fn new(node: uuid::Uuid) -> Self {
        Self { node, id: uuid::Uuid::now_v7().simple().to_string() }
    }

    /// `<node>-<id>` (no suffix).
    #[must_use]
    pub fn stem(&self) -> String {
        format!("{}-{}", self.node, self.id)
    }

    /// Parses a stem (`<node uuid>-<32 hex>`). Anything else is refused,
    /// so a name can never carry a path.
    #[must_use]
    pub fn parse(stem: &str) -> Option<Self> {
        let (node, id) = stem.split_at_checked(36)?;
        let id = id.strip_prefix('-')?;
        let node = uuid::Uuid::try_parse(node).ok()?;

        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return None;
        }

        Some(Self { node, id: id.to_owned() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn rows_are_tagged_by_kind_and_round_trip() {
        let r = Row::Log(AppLog { key: "ab".into(), ts: 5, fmt: BodyFormat::Raw, body: "{\"@mt\":\"x\"}\n".into() });
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.starts_with("{\"k\":\"log\""), "{s}");
        assert!(!s.contains('\n'), "one line");
        assert_eq!(serde_json::from_str::<Row>(&s).unwrap(), r);
        let mut c = Counts::default();
        c.add(r.kind());
        c.add(Kind::ContainerMinute);
        assert_eq!((c.log, c.container_minute, c.total()), (1, 1, 2));
    }

    #[test]
    fn names_parse_strictly() {
        let n = FileName::new(uuid::Uuid::new_v4());
        assert_eq!(FileName::parse(&n.stem()), Some(n.clone()));
        let later = FileName::new(n.node);
        assert!(later.id > n.id || later.id[..12] == n.id[..12], "time ordered");

        for bad in ["", "../x", &format!("{}-{}", n.node, "Z".repeat(32)), &format!("{}/{}", n.node, n.id)] {
            assert_eq!(FileName::parse(bad), None, "{bad}");
        }
    }
}
