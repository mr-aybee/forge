//! File explorer vocabulary: public paths, roots, the wire operation sent to a
//! node and the protected-path policy. Shared by the API, which validates
//! early, and the node agent, which enforces again where the operation takes
//! effect. No I/O here.
//!
//! Public paths are slash-separated and absolute *within a root*. On Windows
//! node roots the first component is the drive, so `C:\Users` is `/C:/Users`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Longest public path accepted, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// Longest single path component, in bytes.
pub const MAX_COMPONENT_BYTES: usize = 255;
/// Deepest path accepted.
pub const MAX_DEPTH: usize = 128;
/// Most entries returned by one directory listing page.
pub const MAX_LIST_LIMIT: u32 = 1000;
/// Default directory listing page size.
pub const DEFAULT_LIST_LIMIT: u32 = 500;
/// Most bytes returned by one read. Results travel base64-encoded in agent
/// heartbeats, whose bodies are capped at 1 MiB and may carry several results.
pub const MAX_READ_BYTES: u32 = 256 * 1024;

/// The built-in root covering the node's whole filesystem.
pub const NODE_ROOT: &str = "node";

/// Bytes up to which a file opens in the editor (and gets revisions).
pub const EDITABLE_BYTES: u64 = 2 * 1024 * 1024;
/// Saved revisions kept per file.
pub const MAX_REVISIONS: usize = 10;

/// A validated path inside a root: `/`, `/etc/nginx`, `/C:/Users`.
///
/// Rejects `.` and `..` components, NUL bytes, over-long components and
/// paths. Empty components (`//`, a trailing `/`) are dropped, so every
/// path has exactly one spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FilePath {
    components: Vec<String>,
}

impl FilePath {
    /// The root of a root (`/`).
    #[must_use]
    pub fn root() -> Self {
        Self { components: Vec::new() }
    }

    /// Parses a public path.
    pub fn parse(s: &str) -> Result<Self, String> {
        if s.len() > MAX_PATH_BYTES {
            return Err("path is too long".into());
        }

        if s.contains('\0') {
            return Err("path contains a NUL byte".into());
        }

        let s = if s.is_empty() { "/" } else { s };
        if !s.starts_with('/') {
            return Err("path must start with `/`".into());
        }

        let mut components = Vec::new();

        for c in s.split('/').filter(|c| !c.is_empty()) {
            if c == "." || c == ".." {
                return Err("path must not contain `.` or `..`".into());
            }

            if c.len() > MAX_COMPONENT_BYTES {
                return Err("a path component is too long".into());
            }

            components.push(c.to_owned());
        }

        if components.len() > MAX_DEPTH {
            return Err("path is too deep".into());
        }

        Ok(Self { components })
    }

    #[must_use]
    pub fn components(&self) -> &[String] {
        &self.components
    }

    #[must_use]
    pub fn is_root(&self) -> bool {
        self.components.is_empty()
    }

    /// The last component, if any.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.components.last().map(String::as_str)
    }

    /// This path with one more component (validated).
    pub fn join(&self, name: &str) -> Result<Self, String> {
        if name.is_empty() || name.contains('/') {
            return Err("invalid file name".into());
        }

        Self::parse(&format!("{self}/{name}"))
    }

    /// A path from components already known to be valid (they came from a
    /// validated path or a directory listing of names without `/`).
    #[must_use]
    pub fn from_components(components: &[String]) -> FilePath {
        FilePath { components: components.to_vec() }
    }

    /// `self` followed by every component of `rel`.
    #[must_use]
    pub fn append(&self, rel: &FilePath) -> FilePath {
        let mut components = self.components.clone();
        components.extend(rel.components.iter().cloned());

        FilePath { components }
    }

    /// The parent path (`/` for `/` and top-level entries).
    #[must_use]
    pub fn parent(&self) -> FilePath {
        let mut components = self.components.clone();
        components.pop();

        FilePath { components }
    }

    /// Whether `self` is `prefix` or lies below it.
    #[must_use]
    pub fn starts_with(&self, prefix: &[&str]) -> bool {
        self.components.len() >= prefix.len() && self.components.iter().zip(prefix).all(|(a, b)| a == b)
    }
}

impl fmt::Display for FilePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.components.is_empty() {
            return f.write_str("/");
        }

        for c in &self.components {
            write!(f, "/{c}")?;
        }

        Ok(())
    }
}

impl TryFrom<String> for FilePath {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<FilePath> for String {
    fn from(p: FilePath) -> Self {
        p.to_string()
    }
}

/// Where a root comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    /// The node's filesystem (Linux `/`, Windows drives).
    Node,
    /// An administrator-approved service directory.
    Service,
    /// An approved path inside a container.
    Container,
}

/// Operating-system family of a node, for path policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeOs {
    Linux,
    Windows,
}

impl NodeOs {
    /// From an agent's reported `os` (`linux`, `windows`, ...).
    #[must_use]
    pub fn from_reported(os: &str) -> Self {
        if os.eq_ignore_ascii_case("windows") { Self::Windows } else { Self::Linux }
    }
}

/// What a root covers, as the node needs it. Resolved by the control plane
/// from the root id; never chosen by the browser.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RootSpec {
    /// The whole node.
    Node,
    /// A directory on the node (public node path, such as `/srv/app` or
    /// `/D:/sites/app`). Operations stay below it.
    Dir { base: FilePath },
    /// A directory inside a running Docker container, by full container id.
    Container { container: String, base: FilePath },
    /// A native app's own folder, `/srv/forge/apps/<environment>/<app>` (Linux,
    /// D-083). The node runs every operation as the app's own account
    /// (D-081), never as root; it finds the account itself.
    App { environment: String, app: String },
}

/// Where native apps live on Linux nodes: `/srv/forge/apps/<ns>/<app>`.
pub const APP_HOMES: [&str; 3] = ["srv", "forge", "apps"];

/// Per-app trash and revision stores on Linux nodes, outside the app's
/// folder (which apps often serve) and owned by the app's account (D-083).
pub const APP_STORES: [&str; 3] = ["var", "lib", "forge-app-store"];

/// An environment or app slug: `a-z`, `0-9`, `-`, starting with a letter.
fn app_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.ends_with('-')
}

impl RootSpec {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Node => Ok(()),
            Self::Dir { base } if base.is_root() => Err("an approved directory cannot be `/`".into()),
            Self::Dir { .. } => Ok(()),
            Self::Container { container, .. } => {
                if container.len() == 64 && container.bytes().all(|b| b.is_ascii_hexdigit()) {
                    Ok(())
                } else {
                    Err("a container root needs the full 64-character container id".into())
                }
            }
            Self::App { environment, app } => {
                if app_slug(environment) && app_slug(app) {
                    Ok(())
                } else {
                    Err("an app root needs a valid environment and app name".into())
                }
            }
        }
    }

    /// The node-level (or container-level) path of `path` in this root,
    /// which the protected-path policy is matched against.
    #[must_use]
    pub fn full_path(&self, path: &FilePath) -> FilePath {
        match self {
            Self::Node => path.clone(),
            Self::Dir { base } | Self::Container { base, .. } => base.append(path),
            Self::App { environment, app } => Self::app_base(environment, app).append(path),
        }
    }

    /// The node folder of an app root.
    #[must_use]
    pub fn app_base(environment: &str, app: &str) -> FilePath {
        let mut c: Vec<String> = APP_HOMES.iter().map(|s| (*s).to_owned()).collect();
        c.push(environment.to_owned());
        c.push(app.to_owned());

        FilePath { components: c }
    }

    /// The node folder a root covers (`/` for the whole node).
    #[must_use]
    pub fn base(&self) -> FilePath {
        match self {
            Self::Node => FilePath::root(),
            Self::Dir { base } | Self::Container { base, .. } => base.clone(),
            Self::App { environment, app } => Self::app_base(environment, app),
        }
    }

    /// Linux nodes (D-083): an approved directory that is exactly a native
    /// app's folder becomes that app's root, so its operations run as the
    /// app's account. A directory inside an app's folder, or one that
    /// contains app folders (`/srv`, `/srv/forge`, `/srv/forge/apps`,
    /// `/srv/forge/apps/<ns>`), is refused: as a plain directory it would
    /// run as root over the app's files. Other roots are unchanged.
    pub fn for_linux_apps(self) -> Result<Self, String> {
        let Self::Dir { base } = &self else { return Ok(self) };
        let c = base.components();
        let n = APP_HOMES.len();
        let in_homes = c.len() >= n && c.iter().zip(APP_HOMES).all(|(a, b)| a == b);
        let above_homes = c.len() < n && c.iter().zip(APP_HOMES).all(|(a, b)| a == b);
        if above_homes || (in_homes && c.len() == n + 1) {
            return Err(format!(
                "`{base}` holds native apps' folders; approve one app's folder ({}/<environment>/<app>) instead",
                "/srv/forge/apps"
            ));
        }

        if in_homes && c.len() == n + 2 && app_slug(&c[n]) && app_slug(&c[n + 1]) {
            return Ok(Self::App { environment: c[n].clone(), app: c[n + 1].clone() });
        }

        if in_homes {
            return Err(format!(
                "`{base}` is inside a native app's folder; approve the app's folder itself ({}/{}/{})",
                "/srv/forge/apps",
                c.get(n).map_or("<environment>", String::as_str),
                c.get(n + 1).map_or("<app>", String::as_str)
            ));
        }

        Ok(self)
    }
}

/// What may happen when a destination already exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Overwrite {
    /// Fail if the destination exists.
    Never,
    /// Replace only if the destination is still this version.
    IfVersion { version: String },
    /// Replace whatever is there.
    Always,
}

/// Filters and limits for a node-side search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSpec {
    /// Glob matched against file names (`*.conf`), or empty for all.
    #[serde(default)]
    pub name: String,
    /// Content pattern; empty searches names only.
    #[serde(default)]
    pub content: String,
    /// Treat `content` as a regular expression rather than fixed text.
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    /// Lines of context around each match (0–5).
    #[serde(default)]
    pub context: u8,
    /// Globs a relative path must match (any), and must not match (all).
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "default_depth")]
    pub max_depth: u32,
    /// Skip files larger than this (content search).
    #[serde(default = "default_search_file_size")]
    pub max_file_size: u64,
    /// Only files modified at or after this time (epoch ms).
    #[serde(default)]
    pub modified_since: i64,
    #[serde(default)]
    pub hidden: bool,
    /// Resume after this relative path (from a previous `next`).
    #[serde(default)]
    pub after: Option<String>,
}

fn default_depth() -> u32 {
    32
}

fn default_search_file_size() -> u64 {
    1024 * 1024
}

/// Largest file a content search reads (nodes may have little memory).
pub const SEARCH_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Search limits (the node stops at the first one reached).
pub const SEARCH_MAX_MATCHES: usize = 1000;
pub const SEARCH_MAX_RESULT_BYTES: usize = 512 * 1024;
pub const SEARCH_MAX_SECONDS: u64 = 50;
pub const SEARCH_MAX_SCANNED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One file operation sent to a node (`FileAction.op_json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileOp {
    /// One page of a directory, sorted by name.
    List {
        root: RootSpec,
        path: FilePath,
        #[serde(default)]
        offset: u32,
        #[serde(default = "default_limit")]
        limit: u32,
        /// Include dotfiles (Linux) and hidden files (Windows).
        #[serde(default)]
        hidden: bool,
    },
    /// Metadata of one path (not followed if it is a link).
    Stat { root: RootSpec, path: FilePath },
    /// Up to [`MAX_READ_BYTES`] of a regular file. With `version`, fails
    /// with `source changed` if the file is no longer that version.
    Read {
        root: RootSpec,
        path: FilePath,
        #[serde(default)]
        offset: u64,
        length: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// SHA-256 of a regular file.
    Hash { root: RootSpec, path: FilePath },
    /// Creates a directory. The parent must exist unless `parents` is set,
    /// which creates every missing folder on the way (like `mkdir -p`; an
    /// existing directory is then not an error). `parents` is left out of
    /// the wire form when false, so older agents still accept the op.
    Mkdir {
        root: RootSpec,
        path: FilePath,
        #[serde(default, skip_serializing_if = "is_false")]
        parents: bool,
    },
    /// Writes bytes into the private partial file of an upload to `path`.
    /// The first chunk (offset 0) creates it; later chunks must continue at
    /// its current length.
    UploadChunk { root: RootSpec, path: FilePath, upload: String, offset: u64, data_b64: String },
    /// Length of an upload's partial file (0 if absent), for resuming.
    UploadStatus { root: RootSpec, path: FilePath, upload: String },
    /// Verifies and moves a finished upload into place.
    UploadCommit { root: RootSpec, path: FilePath, upload: String, size: u64, sha256: String, overwrite: Overwrite },
    /// Removes an upload's partial file.
    UploadAbort { root: RootSpec, path: FilePath, upload: String },
    /// Renames or moves within the root.
    Rename {
        root: RootSpec,
        from: FilePath,
        to: FilePath,
        #[serde(default)]
        overwrite: bool,
    },
    /// Copies a file or directory within the root.
    Copy {
        root: RootSpec,
        from: FilePath,
        to: FilePath,
        #[serde(default)]
        overwrite: bool,
    },
    /// Moves to the trash, or removes for good with `permanent`.
    Delete {
        root: RootSpec,
        path: FilePath,
        #[serde(default)]
        permanent: bool,
    },
    /// Trash entries that came from this root.
    TrashList { root: RootSpec },
    /// Puts a trash entry back where it was.
    TrashRestore {
        root: RootSpec,
        id: String,
        #[serde(default)]
        overwrite: bool,
    },
    /// Removes one trash entry, or every entry from this root.
    TrashPurge {
        root: RootSpec,
        #[serde(default)]
        id: Option<String>,
    },
    /// Saved revisions of a file, newest first.
    Revisions { root: RootSpec, path: FilePath },
    /// Reads a saved revision.
    RevisionRead {
        root: RootSpec,
        path: FilePath,
        revision: String,
        #[serde(default)]
        offset: u64,
        length: u32,
    },
    /// Filename and content search below `path`.
    Search { root: RootSpec, path: FilePath, spec: SearchSpec },
    /// Sets permission bits (Linux). With `recursive`, below a directory
    /// too; `dry_run` only counts what would change.
    SetPermissions {
        root: RootSpec,
        path: FilePath,
        /// `0o0000`–`0o7777`.
        mode: u32,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        dry_run: bool,
    },
    /// Sets owner and/or group by name (Linux).
    SetOwner {
        root: RootSpec,
        path: FilePath,
        #[serde(default)]
        owner: Option<String>,
        #[serde(default)]
        group: Option<String>,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        dry_run: bool,
    },
    /// The access control list as the OS reports it (Windows: `icacls`).
    Acl { root: RootSpec, path: FilePath },
    /// Packs `sources` into a new archive at `dest`, or into Forge's
    /// temporary store when `dest` is absent (download as archive).
    Compress {
        root: RootSpec,
        sources: Vec<FilePath>,
        #[serde(default)]
        dest: Option<FilePath>,
        format: ArchiveFormat,
    },
    /// Unpacks an archive into a new folder `dest`. Without `replace`, `dest`
    /// must not exist. With `replace`, an existing folder `dest` is swapped
    /// for the unpacked one in one step once extraction succeeded (no window
    /// in which `dest` is missing on Linux), and the old content is removed.
    Extract {
        root: RootSpec,
        archive: FilePath,
        dest: FilePath,
        #[serde(default, skip_serializing_if = "is_false")]
        replace: bool,
    },
    /// Reads a temporary archive made by `Compress` without `dest`.
    TempRead {
        root: RootSpec,
        id: String,
        #[serde(default)]
        offset: u64,
        length: u32,
    },
    /// Removes a temporary archive.
    TempDelete { root: RootSpec, id: String },
}

/// Archive formats Forge writes and reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    /// Read only.
    Tar,
}

impl ArchiveFormat {
    /// By file name (`.zip`, `.tar`, `.tar.gz`, `.tgz`).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let n = name.to_ascii_lowercase();
        if n.ends_with(".zip") {
            Some(Self::Zip)
        } else if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if n.ends_with(".tar") {
            Some(Self::Tar)
        } else {
            None
        }
    }
}

/// Limits for archives (zip bombs, huge trees).
pub const ARCHIVE_MAX_ENTRIES: usize = 20_000;
pub const ARCHIVE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Uncompressed size at most this many times the compressed size.
pub const ARCHIVE_MAX_RATIO: u64 = 200;
/// Items one bulk request may name.
pub const BULK_MAX_ITEMS: usize = 500;

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

fn default_limit() -> u32 {
    DEFAULT_LIST_LIMIT
}

/// Most bytes in one upload chunk (before base64).
pub const MAX_CHUNK_BYTES: usize = 256 * 1024;

/// Most bytes a client sends in one upload request or asks for in one read
/// through the REST API (issue #15). The control plane splits them into
/// [`MAX_CHUNK_BYTES`] / [`MAX_READ_BYTES`] node operations sent in one
/// round trip, so agents keep seeing the sizes they always accepted.
pub const API_TRANSFER_BYTES: usize = 2 * 1024 * 1024;

fn valid_token(s: &str, what: &str) -> Result<(), String> {
    if s.is_empty() || s.len() > 64 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(format!("invalid {what}"));
    }

    Ok(())
}

impl FileOp {
    #[must_use]
    pub fn root(&self) -> &RootSpec {
        match self {
            Self::List { root, .. }
            | Self::Stat { root, .. }
            | Self::Read { root, .. }
            | Self::Hash { root, .. }
            | Self::Mkdir { root, .. }
            | Self::UploadChunk { root, .. }
            | Self::UploadStatus { root, .. }
            | Self::UploadCommit { root, .. }
            | Self::UploadAbort { root, .. }
            | Self::Rename { root, .. }
            | Self::Copy { root, .. }
            | Self::Delete { root, .. }
            | Self::TrashList { root }
            | Self::TrashRestore { root, .. }
            | Self::TrashPurge { root, .. }
            | Self::Revisions { root, .. }
            | Self::RevisionRead { root, .. }
            | Self::Search { root, .. }
            | Self::SetPermissions { root, .. }
            | Self::SetOwner { root, .. }
            | Self::Acl { root, .. }
            | Self::Compress { root, .. }
            | Self::Extract { root, .. }
            | Self::TempRead { root, .. }
            | Self::TempDelete { root, .. } => root,
        }
    }

    /// Every path the operation touches (relative to its root).
    #[must_use]
    pub fn paths(&self) -> Vec<&FilePath> {
        match self {
            Self::List { path, .. }
            | Self::Stat { path, .. }
            | Self::Read { path, .. }
            | Self::Hash { path, .. }
            | Self::Mkdir { path, .. }
            | Self::UploadChunk { path, .. }
            | Self::UploadStatus { path, .. }
            | Self::UploadCommit { path, .. }
            | Self::UploadAbort { path, .. }
            | Self::Delete { path, .. }
            | Self::Revisions { path, .. }
            | Self::RevisionRead { path, .. }
            | Self::Search { path, .. }
            | Self::SetPermissions { path, .. }
            | Self::SetOwner { path, .. }
            | Self::Acl { path, .. } => vec![path],
            Self::Compress { sources, dest, .. } => sources.iter().chain(dest.iter()).collect(),
            Self::Extract { archive, dest, .. } => vec![archive, dest],
            Self::TempRead { .. } | Self::TempDelete { .. } => vec![],
            Self::Rename { from, to, .. } | Self::Copy { from, to, .. } => vec![from, to],
            Self::TrashList { .. } | Self::TrashRestore { .. } | Self::TrashPurge { .. } => vec![],
        }
    }

    /// Paths whose whole subtree the operation moves, replaces or removes:
    /// both ends of a rename (a moved folder could carry protected content
    /// in or out) and a delete. Checked with [`protected_within`].
    #[must_use]
    pub fn subtrees(&self) -> Vec<&FilePath> {
        match self {
            Self::Rename { from, to, .. } => vec![from, to],
            Self::Delete { path, .. } => vec![path],
            // Replacing a folder removes its old content.
            Self::Extract { dest, replace: true, .. } => vec![dest],
            _ => vec![],
        }
    }

    /// The first path, or `/` for trash operations.
    #[must_use]
    pub fn path(&self) -> FilePath {
        self.paths().first().map_or_else(FilePath::root, |p| (*p).clone())
    }

    /// Whether the operation changes the filesystem.
    #[must_use]
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            Self::List { .. }
                | Self::Stat { .. }
                | Self::Read { .. }
                | Self::Hash { .. }
                | Self::UploadStatus { .. }
                | Self::TrashList { .. }
                | Self::Revisions { .. }
                | Self::RevisionRead { .. }
                | Self::Search { .. }
                | Self::Acl { .. }
                | Self::TempRead { .. }
                | Self::SetPermissions { dry_run: true, .. }
                | Self::SetOwner { dry_run: true, .. }
                | Self::Compress { dest: None, .. }
        )
    }

    /// Bounds, names and root checks, run by the API and again by the node.
    pub fn validate(&self) -> Result<(), String> {
        self.root().validate()?;

        match self {
            Self::List { limit, .. } if *limit == 0 || *limit > MAX_LIST_LIMIT => {
                return Err(format!("limit must be between 1 and {MAX_LIST_LIMIT}"));
            }
            Self::Read { length, version, .. } => {
                if *length == 0 || *length > MAX_READ_BYTES {
                    return Err(format!("length must be between 1 and {MAX_READ_BYTES}"));
                }

                if version.as_ref().is_some_and(|v| v.len() > 128 || !v.bytes().all(|b| b.is_ascii_graphic())) {
                    return Err("invalid version".into());
                }
            }
            Self::RevisionRead { length, revision, .. } => {
                valid_token(revision, "revision")?;

                if *length == 0 || *length > MAX_READ_BYTES {
                    return Err(format!("length must be between 1 and {MAX_READ_BYTES}"));
                }
            }
            Self::UploadChunk { upload, data_b64, .. } => {
                valid_token(upload, "upload id")?;

                if data_b64.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4 {
                    return Err("chunk is too large".into());
                }
            }
            Self::UploadStatus { upload, .. } | Self::UploadAbort { upload, .. } => valid_token(upload, "upload id")?,
            Self::UploadCommit { upload, sha256, overwrite, .. } => {
                valid_token(upload, "upload id")?;

                if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("sha256 must be 64 hex digits".into());
                }

                if let Overwrite::IfVersion { version } = overwrite
                    && (version.is_empty() || version.len() > 128)
                {
                    return Err("invalid version".into());
                }
            }
            Self::TrashRestore { id, .. } | Self::TrashPurge { id: Some(id), .. } => valid_token(id, "trash id")?,
            Self::TempRead { id, length, .. } => {
                valid_token(id, "archive id")?;

                if *length == 0 || *length > MAX_READ_BYTES {
                    return Err(format!("length must be between 1 and {MAX_READ_BYTES}"));
                }
            }
            Self::TempDelete { id, .. } => valid_token(id, "archive id")?,
            Self::SetPermissions { mode, .. } if *mode > 0o7777 => {
                return Err("mode must be between 0000 and 7777 (octal)".into());
            }
            Self::SetOwner { owner, group, .. } => {
                if owner.is_none() && group.is_none() {
                    return Err("give an owner, a group or both".into());
                }

                let valid = |n: &String| {
                    !n.is_empty()
                        && n.len() <= 64
                        && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'$'))
                };

                if owner.iter().chain(group.iter()).any(|n| !valid(n)) {
                    return Err("invalid user or group name".into());
                }
            }
            Self::Compress { sources, dest, format, .. } => {
                if sources.is_empty() || sources.len() > BULK_MAX_ITEMS {
                    return Err(format!("choose between 1 and {BULK_MAX_ITEMS} items"));
                }

                if *format == ArchiveFormat::Tar {
                    return Err("Forge writes .zip or .tar.gz archives".into());
                }

                if let Some(d) = dest
                    && ArchiveFormat::from_name(d.name().unwrap_or_default()) != Some(*format)
                {
                    return Err("the archive name must end in .zip, or .tar.gz / .tgz".into());
                }
            }
            Self::Extract { archive, dest, .. } => {
                if ArchiveFormat::from_name(archive.name().unwrap_or_default()).is_none() {
                    return Err("Forge extracts .zip, .tar, .tar.gz and .tgz archives".into());
                }

                if dest.is_root() {
                    return Err("extract into a new folder".into());
                }
            }
            Self::Search { spec, .. } => {
                if spec.name.is_empty() && spec.content.is_empty() {
                    return Err("give a file name pattern, a content pattern or both".into());
                }

                if spec.name.len() > 256 || spec.content.len() > 1024 || spec.context > 5 {
                    return Err("search pattern or context too large".into());
                }

                if spec.include.len() > 32 || spec.exclude.len() > 32 {
                    return Err("too many include or exclude globs".into());
                }

                if spec.max_file_size == 0 || spec.max_file_size > SEARCH_MAX_FILE_BYTES {
                    return Err(format!("max_file_size must be between 1 and {SEARCH_MAX_FILE_BYTES} bytes"));
                }

                if spec.max_depth > 64 {
                    return Err("max_depth must be at most 64".into());
                }
            }
            _ => {}
        }

        let changes_entry = matches!(
            self,
            Self::Rename { .. } | Self::Copy { .. } | Self::Delete { .. } | Self::Mkdir { .. } | Self::Extract { .. }
        );

        for p in self.paths() {
            if p.is_root() && changes_entry {
                return Err("cannot change the root itself".into());
            }

            if p.name().is_some_and(is_forge_internal) {
                return Err(format!("names starting with `{UPLOAD_PREFIX}` are reserved"));
            }
        }

        if let Self::Rename { from, to, .. } | Self::Copy { from, to, .. } = self
            && to.starts_with(&from.components().iter().map(String::as_str).collect::<Vec<_>>())
        {
            return Err("cannot move or copy a directory into itself".into());
        }

        Ok(())
    }
}

/// Partial uploads live next to their destination under this prefix and
/// are hidden from listings.
pub const UPLOAD_PREFIX: &str = ".forge-upload-";

#[must_use]
pub fn is_forge_internal(name: &str) -> bool {
    name.starts_with(UPLOAD_PREFIX)
}

/// Type of a directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// Sockets, devices, FIFOs, Windows reparse points other than links.
    Other,
}

/// One directory entry or `stat` result, as the node reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Last modification, epoch ms.
    #[serde(default)]
    pub modified_ms: i64,
    /// Unix permission bits (`0o644`); absent on Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Target of a symbolic link (never followed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_target: Option<String>,
    /// The entry is a mount point (another filesystem starts here).
    #[serde(default)]
    pub mount: bool,
    /// Forge refuses to open this path (keys, credentials, virtual filesystems).
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub hidden: bool,
}

/// Why a path is protected, or `None`. Applies to listing, reading and,
/// later, every other operation. Matching is on the path as written: nodes
/// never follow links while resolving, so the written path is the real one.
#[must_use]
pub fn protected_reason(os: NodeOs, path: &FilePath) -> Option<&'static str> {
    match os {
        NodeOs::Linux => linux_protected(path),
        // An 8.3 short name (`PROGRA~3`) is another spelling of a long one;
        // refusing them keeps the literal comparison below complete.
        NodeOs::Windows => {
            if path.components().iter().any(|c| is_short_name(c)) {
                Some("8.3 short names are not accepted; use the long name")
            } else {
                windows_protected(path)
            }
        }
    }
}

/// Whether a Windows path component looks like an 8.3 short name
/// (`PROGRA~3`, `AGENT~1.TOM`): at most 8 characters, a `~` and digits at
/// the end of the stem, and an extension of at most 3.
#[must_use]
pub fn is_short_name(name: &str) -> bool {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    if stem.len() > 8 || ext.len() > 3 {
        return false;
    }

    stem.rfind('~').is_some_and(|i| i > 0 && i + 1 < stem.len() && stem[i + 1..].bytes().all(|b| b.is_ascii_digit()))
}

/// Why a path cannot be moved, replaced or removed as a whole: it is
/// protected itself, or a protected location may lie below it (moving
/// `/home/alice` would relocate `.ssh`; removing `/var` would remove the
/// agent's identity). `None` when the whole subtree is free.
#[must_use]
pub fn protected_within(os: NodeOs, path: &FilePath) -> Option<&'static str> {
    if let Some(why) = protected_reason(os, path) {
        return Some(why);
    }

    match os {
        NodeOs::Linux => linux_within(path),
        NodeOs::Windows => windows_within(path),
    }
}

/// `path` lies strictly above `protected`.
fn is_above(path: &FilePath, protected: &[&str]) -> bool {
    let c = path.components();
    c.len() < protected.len() && c.iter().zip(protected).all(|(a, b)| a == b)
}

fn linux_within(path: &FilePath) -> Option<&'static str> {
    if let Some((_, why)) = LINUX_PROTECTED_TREES.iter().find(|(p, _)| is_above(path, p)) {
        return Some(why);
    }

    if LINUX_PROTECTED_FILES.iter().any(|p| is_above(path, p)) {
        return Some("password hashes");
    }

    // /home and /home/<user> may hold /home/<user>/.ssh.
    let c = path.components();
    if c.len() <= 2 && c.first().is_none_or(|h| h == "home") {
        return Some("SSH credentials");
    }

    if is_above(path, &["etc", "ssh", "ssh_host_key"]) {
        return Some("SSH host key");
    }

    None
}

/// Every protected Windows location lies at a drive's top level or below
/// `ProgramData`, `Windows\System32` or `Users\<user>`; those folders (and
/// their parents) are never moved or removed as a whole. A new protected
/// location elsewhere must be added here too.
fn windows_within(path: &FilePath) -> Option<&'static str> {
    let c: Vec<String> = path.components().iter().map(|s| s.to_ascii_lowercase()).collect();
    let rest: Vec<&str> = c.iter().skip(1).map(String::as_str).collect();
    match rest.as_slice() {
        // A drive (`/C:`); the node root's first component is always one.
        [] if c.first().is_none_or(|d| d.len() == 2 && d.ends_with(':')) => Some("a drive holds system files"),
        ["programdata"] => Some("Forge configuration and agent identity"),
        ["windows"] | ["windows", "system32"] => Some("registry hives and credentials"),
        ["users"] | ["users", _] => Some("SSH credentials"),
        _ => None,
    }
}

/// Subtrees Forge never opens on Linux.
const LINUX_PROTECTED_TREES: &[(&[&str], &str)] = &[
    (&["proc"], "virtual filesystem"),
    (&["sys"], "virtual filesystem"),
    (&["dev"], "device files"),
    (&["run"], "runtime sockets"),
    (&["etc", "forge"], "Forge configuration and package credentials"),
    (&["var", "lib", "forge"], "Forge control-plane data and keys"),
    (&["var", "lib", "forge-agent"], "Forge agent identity"),
    (&["var", "lib", "forge-app-store"], "Forge trash and revisions of native apps"),
    (&["root", ".ssh"], "SSH credentials"),
];

const LINUX_PROTECTED_FILES: &[&[&str]] =
    &[&["etc", "shadow"], &["etc", "shadow-"], &["etc", "gshadow"], &["etc", "gshadow-"]];

fn linux_protected(path: &FilePath) -> Option<&'static str> {
    if let Some((_, why)) = LINUX_PROTECTED_TREES.iter().find(|(p, _)| path.starts_with(p)) {
        return Some(why);
    }

    if LINUX_PROTECTED_FILES.iter().any(|p| path.components().len() == p.len() && path.starts_with(p)) {
        return Some("password hashes");
    }

    let c = path.components();

    // /home/<user>/.ssh/...
    if c.len() >= 3 && c[0] == "home" && c[2] == ".ssh" {
        return Some("SSH credentials");
    }

    // SSH host private keys (the `.pub` halves stay readable).
    if c.len() == 3 && c[0] == "etc" && c[1] == "ssh" && c[2].starts_with("ssh_host_") && c[2].ends_with("_key") {
        return Some("SSH host key");
    }

    None
}

fn windows_protected(path: &FilePath) -> Option<&'static str> {
    let c: Vec<String> = path.components().iter().map(|s| s.to_ascii_lowercase()).collect();
    let rest: Vec<&str> = c.iter().skip(1).map(String::as_str).collect();
    let under = |p: &[&str]| rest.len() >= p.len() && rest.iter().zip(p).all(|(a, b)| a == b);

    // `OrbynForge\agent` is the agent's state (agent.key, identity, ca.crt).
    if under(&["programdata", "forge"]) || under(&["programdata", "orbynforge"]) {
        return Some("Forge configuration and agent identity");
    }

    if under(&["windows", "system32", "config"]) {
        return Some("registry hives and credentials");
    }

    if rest.len() == 1 && matches!(rest[0], "pagefile.sys" | "hiberfil.sys" | "swapfile.sys") {
        return Some("system memory file");
    }

    if rest.len() >= 3 && rest[0] == "users" && rest[2] == ".ssh" {
        return Some("SSH credentials");
    }

    None
}

// ---- control plane: roots, uploads and the node transport ---------------

/// An administrator-approved root on a server (service or container
/// directory). The whole-node root is built in and not stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedRoot {
    /// `fr_` followed by 24 hex digits.
    pub id: String,
    pub workspace_id: crate::WorkspaceId,
    pub server_id: crate::ServerId,
    pub kind: RootKind,
    pub label: String,
    pub spec: RootSpec,
    /// Read-only roots refuse every change, whatever the user's roles.
    pub read_only: bool,
    /// The service this directory belongs to, when known.
    pub service_id: Option<String>,
    pub created_by: String,
    pub created_at: crate::Timestamp,
}

/// A resumable upload session. The bytes live in a private partial file
/// on the node; the control plane keeps who may continue it and where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileUpload {
    /// 32 hex digits; also names the node's partial file.
    pub id: String,
    pub workspace_id: crate::WorkspaceId,
    pub server_id: crate::ServerId,
    pub root_id: String,
    pub path: FilePath,
    pub size: u64,
    pub received: u64,
    pub overwrite: Overwrite,
    /// The principal (or grant) that started it; only it may continue.
    pub created_by: String,
    pub created_at: crate::Timestamp,
    pub expires_at: crate::Timestamp,
}

/// Approved roots and upload sessions.
#[async_trait::async_trait]
pub trait FileRepository: Send + Sync + std::fmt::Debug {
    async fn put_root(&self, root: &ApprovedRoot) -> crate::DomainResult<()>;
    async fn get_root(&self, id: &str) -> crate::DomainResult<Option<ApprovedRoot>>;
    async fn list_roots(&self, server: crate::ServerId) -> crate::DomainResult<Vec<ApprovedRoot>>;
    async fn delete_root(&self, id: &str) -> crate::DomainResult<()>;
    async fn put_upload(&self, upload: &FileUpload) -> crate::DomainResult<()>;
    async fn get_upload(&self, id: &str) -> crate::DomainResult<Option<FileUpload>>;
    async fn delete_upload(&self, id: &str) -> crate::DomainResult<()>;
    /// Uploads whose `expires_at` is before `now`.
    async fn expired_uploads(&self, now: crate::Timestamp) -> crate::DomainResult<Vec<FileUpload>>;
}

/// Sends one file operation to a node and returns its answer (the agent
/// link). Errors are the node's message, or a transport problem prefixed
/// with `transport:`.
#[async_trait::async_trait]
pub trait FileTransport: Send + Sync + std::fmt::Debug {
    async fn call(&self, server: crate::ServerId, principal: &str, op: &FileOp) -> Result<serde_json::Value, String>;

    /// Several operations for one node, run in this order. A transport that
    /// can queue them together (the agent link) does so, which saves a round
    /// trip per operation in a transfer (issue #15); the default runs them
    /// one by one.
    async fn call_many(
        &self,
        server: crate::ServerId,
        principal: &str,
        ops: &[FileOp],
    ) -> Vec<Result<serde_json::Value, String>> {
        let mut out = Vec::with_capacity(ops.len());

        for op in ops {
            out.push(self.call(server, principal, op).await);
        }

        out
    }
}

impl FileOp {
    /// The same operation against another root (the control plane resolves
    /// the root id; callers never choose the spec).
    #[must_use]
    pub fn with_root(mut self, spec: RootSpec) -> Self {
        match &mut self {
            Self::List { root, .. }
            | Self::Stat { root, .. }
            | Self::Read { root, .. }
            | Self::Hash { root, .. }
            | Self::Mkdir { root, .. }
            | Self::UploadChunk { root, .. }
            | Self::UploadStatus { root, .. }
            | Self::UploadCommit { root, .. }
            | Self::UploadAbort { root, .. }
            | Self::Rename { root, .. }
            | Self::Copy { root, .. }
            | Self::Delete { root, .. }
            | Self::TrashList { root }
            | Self::TrashRestore { root, .. }
            | Self::TrashPurge { root, .. }
            | Self::Revisions { root, .. }
            | Self::RevisionRead { root, .. }
            | Self::Search { root, .. }
            | Self::SetPermissions { root, .. }
            | Self::SetOwner { root, .. }
            | Self::Acl { root, .. }
            | Self::Compress { root, .. }
            | Self::Extract { root, .. }
            | Self::TempRead { root, .. }
            | Self::TempDelete { root, .. } => *root = spec,
        }

        self
    }
}

// ---- temporary SFTP access grants ----------------------------------------

/// `expires_at` of a grant that never expires ([`crate::clock::NEVER_EXPIRES`]
/// as milliseconds). Only allowed when an administrator enables
/// `[sftp] allow_never_expiry`.
pub const NEVER_EXPIRES: i64 = crate::clock::NEVER_EXPIRES.0;

/// Temporary SFTP access to roots of one server, issued by a user. The
/// SFTP user name is `id`; the password is a random token shown once, of
/// which only the SHA-256 is stored. Effective access is the intersection of
/// the grant, the creator's current rights and each root's policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileGrant {
    /// `fg` followed by 16 hex digits.
    pub id: String,
    pub workspace_id: crate::WorkspaceId,
    pub server_id: crate::ServerId,
    /// What it is for, such as "Vendor attachment upload".
    pub name: String,
    /// Root ids (`node` or approved roots of the server).
    pub roots: Vec<String>,
    /// Create, upload and rename (`file.write`).
    pub write: bool,
    /// Delete and replace (`file.delete`).
    pub delete: bool,
    /// Hex SHA-256 of the token.
    #[serde(skip_serializing)]
    pub token_hash: String,
    /// Principal of the user who issued it.
    pub created_by: String,
    /// Principal whose current rights the grant acts with: the creator, or
    /// the last user who widened it (added folders, upload or delete).
    pub owner: String,
    pub created_at: crate::Timestamp,
    pub expires_at: crate::Timestamp,
    pub revoked_at: Option<crate::Timestamp>,
    /// Client addresses or CIDR ranges allowed to connect; empty for any.
    pub allowed_ips: Vec<String>,
    /// Concurrent SFTP sessions allowed.
    pub max_sessions: u32,
    /// Per-session bandwidth cap in KiB/s.
    pub bandwidth_kbps: Option<u32>,
    /// Total bytes the grant may transfer.
    pub transfer_limit: Option<u64>,
    pub bytes_transferred: u64,
    pub last_used_at: Option<crate::Timestamp>,
}

impl FileGrant {
    /// Not revoked and not expired at `now`.
    #[must_use]
    pub fn is_active(&self, now: crate::Timestamp) -> bool {
        self.revoked_at.is_none() && now.0 < self.expires_at.0
    }

    /// Issued without an expiry (see [`NEVER_EXPIRES`]).
    #[must_use]
    pub fn never_expires(&self) -> bool {
        self.expires_at.0 >= NEVER_EXPIRES
    }

    /// `active`, `expired` or `revoked` at `now`.
    #[must_use]
    pub fn status(&self, now: crate::Timestamp) -> &'static str {
        if self.revoked_at.is_some() {
            "revoked"
        } else if now.0 >= self.expires_at.0 {
            "expired"
        } else {
            "active"
        }
    }
}

/// Temporary access grants.
#[async_trait::async_trait]
pub trait FileGrantRepository: Send + Sync + std::fmt::Debug {
    async fn put_grant(&self, grant: &FileGrant) -> crate::DomainResult<()>;
    async fn get_grant(&self, id: &str) -> crate::DomainResult<Option<FileGrant>>;
    async fn list_grants(&self, server: crate::ServerId) -> crate::DomainResult<Vec<FileGrant>>;
    /// Every grant on every server, newest first.
    async fn list_all_grants(&self) -> crate::DomainResult<Vec<FileGrant>>;
    /// Adds transferred bytes and stamps the last use.
    async fn add_grant_usage(&self, id: &str, bytes: u64, at: crate::Timestamp) -> crate::DomainResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> FilePath {
        FilePath::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn new_flags_stay_off_the_wire_when_false_and_default_when_absent() {
        let plain = FileOp::Mkdir { root: RootSpec::Node, path: p("/a"), parents: false };
        let s = serde_json::to_string(&plain).unwrap();
        assert!(!s.contains("parents"), "{s}");
        let back: FileOp = serde_json::from_str(r#"{"op":"mkdir","root":{"kind":"node"},"path":"/a"}"#).unwrap();
        assert_eq!(back, plain);
        let deep = FileOp::Mkdir { root: RootSpec::Node, path: p("/a/b"), parents: true };
        assert!(serde_json::to_string(&deep).unwrap().contains(r#""parents":true"#));
        let ex = FileOp::Extract { root: RootSpec::Node, archive: p("/a.zip"), dest: p("/site"), replace: false };
        assert!(!serde_json::to_string(&ex).unwrap().contains("replace"));
        assert!(ex.subtrees().is_empty());
        let swap = FileOp::Extract { root: RootSpec::Node, archive: p("/a.zip"), dest: p("/site"), replace: true };
        assert_eq!(swap.subtrees(), vec![&p("/site")]);
    }

    #[test]
    fn paths_normalise_and_reject_traversal() {
        assert_eq!(p("").to_string(), "/");
        assert_eq!(p("/etc//nginx/").to_string(), "/etc/nginx");
        assert_eq!(p("/C:/Users").components(), ["C:", "Users"]);

        for bad in ["etc", "/etc/../root", "/./x", "/a\0b", &format!("/{}", "a".repeat(256))] {
            assert!(FilePath::parse(bad).is_err(), "{bad:?} accepted");
        }

        assert!(FilePath::parse(&"/a".repeat(MAX_DEPTH + 1)).is_err());
        assert_eq!(p("/etc").join("hosts").map(|x| x.to_string()), Ok("/etc/hosts".into()));
        assert!(p("/etc").join("../x").is_err());
        assert!(p("/etc").join("..").is_err());
    }

    #[test]
    fn serde_round_trip_validates() {
        let parse = |j: &str| serde_json::from_str::<FileOp>(j);
        let ok = |j: &str| parse(j).unwrap_or_else(|e| panic!("{j}: {e}")).validate();
        assert!(ok(r#"{"op":"list","root":{"kind":"node"},"path":"/var/log"}"#).is_ok());
        assert!(parse(r#"{"op":"list","root":{"kind":"node"},"path":"/var/../etc"}"#).is_err());
        assert!(parse(r#"{"op":"exec","root":{"kind":"node"},"path":"/x"}"#).is_err());
        assert!(parse(r#"{"op":"stat","root":"node","path":"/"}"#).is_err());
        assert!(ok(r#"{"op":"read","root":{"kind":"node"},"path":"/x","length":2000000}"#).is_err());

        // Roots.
        assert!(ok(r#"{"op":"stat","root":{"kind":"dir","base":"/srv/app"},"path":"/"}"#).is_ok());
        assert!(ok(r#"{"op":"stat","root":{"kind":"dir","base":"/"},"path":"/"}"#).is_err());
        assert!(ok(r#"{"op":"stat","root":{"kind":"container","container":"abc","base":"/app"},"path":"/"}"#).is_err());
        let cid = "a".repeat(64);
        assert!(
            ok(&format!(
                r#"{{"op":"stat","root":{{"kind":"container","container":"{cid}","base":"/app"}},"path":"/"}}"#
            ))
            .is_ok()
        );

        // Mutations.
        assert!(ok(r#"{"op":"delete","root":{"kind":"node"},"path":"/"}"#).is_err());
        assert!(ok(r#"{"op":"mkdir","root":{"kind":"node"},"path":"/tmp/.forge-upload-x"}"#).is_err());
        assert!(ok(r#"{"op":"rename","root":{"kind":"node"},"from":"/a","to":"/a/b"}"#).is_err());
        assert!(ok(r#"{"op":"copy","root":{"kind":"node"},"from":"/a","to":"/ab"}"#).is_ok());
        assert!(
            ok(r#"{"op":"upload_chunk","root":{"kind":"node"},"path":"/x","upload":"../x","offset":0,"data_b64":""}"#)
                .is_err()
        );
        let commit = r#"{"op":"upload_commit","root":{"kind":"node"},"path":"/x","upload":"u1","size":1,"sha256":"SHA","overwrite":{"mode":"never"}}"#;
        assert!(ok(commit).is_err());
        assert!(ok(&commit.replace("SHA", &"0".repeat(64))).is_ok());
        assert!(ok(r#"{"op":"search","root":{"kind":"node"},"path":"/","spec":{}}"#).is_err());
        assert!(ok(r#"{"op":"search","root":{"kind":"node"},"path":"/","spec":{"name":"*.conf"}}"#).is_ok());

        // Search reads are bounded: no unbounded max_file_size or depth.
        let big = r#"{"op":"search","root":{"kind":"node"},"path":"/","spec":{"content":"x","max_file_size":18446744073709551615}}"#;
        assert!(ok(big).is_err());
        assert!(
            ok(r#"{"op":"search","root":{"kind":"node"},"path":"/","spec":{"content":"x","max_file_size":8388608}}"#)
                .is_ok()
        );
        assert!(
            ok(r#"{"op":"search","root":{"kind":"node"},"path":"/","spec":{"content":"x","max_depth":1000}}"#).is_err()
        );
        assert!(ok(r#"{"op":"trash_restore","root":{"kind":"node"},"id":"a/b"}"#).is_err());

        // Step 6.
        assert!(ok(r#"{"op":"set_permissions","root":{"kind":"node"},"path":"/x","mode":493}"#).is_ok());
        assert!(ok(r#"{"op":"set_permissions","root":{"kind":"node"},"path":"/x","mode":99999}"#).is_err());
        assert!(ok(r#"{"op":"set_owner","root":{"kind":"node"},"path":"/x"}"#).is_err());
        assert!(ok(r#"{"op":"set_owner","root":{"kind":"node"},"path":"/x","owner":"www-data"}"#).is_ok());
        assert!(ok(r#"{"op":"set_owner","root":{"kind":"node"},"path":"/x","owner":"a;b"}"#).is_err());
        assert!(
            ok(r#"{"op":"compress","root":{"kind":"node"},"sources":["/a"],"dest":"/a.zip","format":"zip"}"#).is_ok()
        );
        assert!(
            ok(r#"{"op":"compress","root":{"kind":"node"},"sources":["/a"],"dest":"/a.tar","format":"zip"}"#).is_err()
        );
        assert!(ok(r#"{"op":"compress","root":{"kind":"node"},"sources":[],"format":"zip"}"#).is_err());
        assert!(ok(r#"{"op":"extract","root":{"kind":"node"},"archive":"/a.tgz","dest":"/a"}"#).is_ok());
        assert!(ok(r#"{"op":"extract","root":{"kind":"node"},"archive":"/a.rar","dest":"/a"}"#).is_err());
        assert!(ok(r#"{"op":"extract","root":{"kind":"node"},"archive":"/a.zip","dest":"/"}"#).is_err());
        assert_eq!(ArchiveFormat::from_name("x.TGZ"), Some(ArchiveFormat::TarGz));
    }

    #[test]
    fn root_full_paths() {
        // D-083: app folders become app roots; directories around or inside them are refused.
        let app = RootSpec::Dir { base: p("/srv/forge/apps/staging/api") }.for_linux_apps();
        assert_eq!(app, Ok(RootSpec::App { environment: "staging".into(), app: "api".into() }));

        for around in
            ["/srv", "/srv/forge", "/srv/forge/apps", "/srv/forge/apps/staging", "/srv/forge/apps/staging/api/public"]
        {
            assert!(RootSpec::Dir { base: p(around) }.for_linux_apps().is_err(), "{around}");
        }

        let other = RootSpec::Dir { base: p("/srv/www") };
        assert_eq!(other.clone().for_linux_apps(), Ok(other));
        let app = RootSpec::App { environment: "staging".into(), app: "api".into() };
        assert_eq!(
            app.full_path(&p("/public/index.html")).to_string(),
            "/srv/forge/apps/staging/api/public/index.html"
        );
        assert_eq!(app.base().to_string(), "/srv/forge/apps/staging/api");
        assert!(app.validate().is_ok());
        assert!(RootSpec::App { environment: "../x".into(), app: "api".into() }.validate().is_err());
        assert!(protected_reason(NodeOs::Linux, &p("/var/lib/forge-app-store/staging-api/trash")).is_some());
        let dir = RootSpec::Dir { base: p("/srv/app") };
        assert_eq!(dir.full_path(&p("/conf/x")).to_string(), "/srv/app/conf/x");
        assert_eq!(RootSpec::Node.full_path(&p("/etc")).to_string(), "/etc");
        assert_eq!(p("/a/b").parent().to_string(), "/a");
        assert_eq!(p("/a").parent().to_string(), "/");
    }

    #[test]
    fn protected_paths() {
        let lin = |s: &str| protected_reason(NodeOs::Linux, &p(s)).is_some();

        for s in [
            "/proc",
            "/proc/1/environ",
            "/etc/forge/pkgs/redis.env",
            "/var/lib/forge/forge.db",
            "/etc/shadow",
            "/home/alice/.ssh/id_ed25519",
            "/root/.ssh",
            "/etc/ssh/ssh_host_ed25519_key",
            "/run/docker.sock",
            "/dev/sda",
        ] {
            assert!(lin(s), "{s} not protected");
        }

        for s in [
            "/",
            "/etc",
            "/etc/hosts",
            "/etc/ssh/ssh_host_ed25519_key.pub",
            "/etc/ssh/sshd_config",
            "/home/alice",
            "/var/lib/forgery",
            "/etc/shadowsocks",
        ] {
            assert!(!lin(s), "{s} protected");
        }

        let win = |s: &str| protected_reason(NodeOs::Windows, &p(s)).is_some();
        assert!(win("/C:/ProgramData/Forge/agent.toml"));
        assert!(win("/c:/programdata/FORGE"));

        // The agent's state directory (forge-agent config.rs default).
        assert!(win("/C:/ProgramData/OrbynForge/agent/agent.key"));
        assert!(win("/c:/PROGRAMDATA/orbynforge/AGENT/identity.json"));
        assert!(win("/C:/ProgramData/OrbynForge"));
        assert!(!win("/C:/ProgramData/OrbynForgeBackup/x"));
        assert!(win("/C:/Windows/System32/config/SAM"));
        assert!(win("/D:/pagefile.sys"));
        assert!(!win("/C:/ProgramData"));
        assert!(!win("/C:/Users/bob/Documents"));
    }

    #[test]
    fn windows_short_names_are_refused() {
        let win = |s: &str| protected_reason(NodeOs::Windows, &p(s)).is_some();

        // 8.3 aliases of protected paths, and short names anywhere.
        assert!(win("/C:/PROGRA~3/Forge/agent.toml"));
        assert!(win("/C:/ProgramData/FORGE~1/agent.toml"));
        assert!(win("/C:/Users/bob/DOCUME~1"));
        assert!(win("/C:/x/AGENT~1.TOM"));

        for ok in ["/C:/Users/bob/Documents", "/C:/x/~1", "/C:/x/a~b", "/C:/x/notes~1.backup", "/C:/x/very-long~1"] {
            assert!(!win(ok), "{ok} refused");
        }

        // Linux names may contain `~1`.
        assert!(protected_reason(NodeOs::Linux, &p("/srv/PROGRA~3")).is_none());
    }

    #[test]
    fn subtrees_holding_protected_paths_are_protected() {
        let lin = |s: &str| protected_within(NodeOs::Linux, &p(s)).is_some();

        for s in [
            "/home",
            "/home/alice",
            "/home/alice/.ssh",
            "/root",
            "/var",
            "/var/lib",
            "/var/lib/forge-agent",
            "/etc",
            "/etc/ssh",
            "/etc/shadow",
        ] {
            assert!(lin(s), "{s} not protected as a subtree");
        }

        for s in ["/home/alice/projects", "/srv/app", "/var/log", "/var/lib/docker", "/etc/nginx", "/opt"] {
            assert!(!lin(s), "{s} protected as a subtree");
        }

        let win = |s: &str| protected_within(NodeOs::Windows, &p(s)).is_some();

        for s in ["/C:", "/C:/ProgramData", "/c:/windows", "/C:/Windows/System32", "/C:/Users", "/C:/Users/bob"] {
            assert!(win(s), "{s} not protected as a subtree");
        }

        for s in ["/C:/Users/bob/Documents", "/C:/Windows/Temp", "/D:/sites"] {
            assert!(!win(s), "{s} protected as a subtree");
        }

        // Every protected Windows sample lies below a guarded folder.
        for s in [
            "/C:/ProgramData/Forge/agent.toml",
            "/C:/ProgramData/OrbynForge/agent/agent.key",
            "/C:/Windows/System32/config/SAM",
            "/C:/Users/bob/.ssh/x",
        ] {
            let mut path = p(s);

            while !path.is_root() {
                path = path.parent();
                assert!(win(&path.to_string()), "{path} (above {s}) not guarded");
            }
        }

        let mv = FileOp::Rename { root: RootSpec::Node, from: p("/a"), to: p("/b"), overwrite: false };
        assert_eq!(mv.subtrees().len(), 2);
        assert!(FileOp::Mkdir { root: RootSpec::Node, path: p("/a"), parents: false }.subtrees().is_empty());
    }
}
