//! The spool file ids the worker stored (D-142), kept on disk so a file
//! sent again after a lost ack is acknowledged without a second write, even
//! across a worker restart.
//!
//! An agent uploads its files oldest first, one at a time, and sends a file
//! again only while it has no ack for it, so the file sent again is always
//! one of the last few the worker stored for that node. The log therefore
//! keeps the last [`PER_NODE`] ids per node: memory is bounded by the number
//! of nodes. Ids are appended to `seen.log` as `<node> <id>` lines; when the
//! file holds more than [`COMPACT_AT`] lines it is rewritten with only the
//! kept ids, and nodes that left the fleet are dropped then.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use forge_core::ServerId;

/// Ids remembered per node.
pub const PER_NODE: usize = 64;
/// Lines in the log before it is rewritten.
const COMPACT_AT: usize = 200_000;
/// Most nodes remembered.
const MAX_NODES: usize = 100_000;
const FILE: &str = "seen.log";

/// The persistent id log.
#[derive(Debug)]
pub struct SeenLog {
    path: PathBuf,
    file: File,
    nodes: HashMap<ServerId, VecDeque<String>>,
    lines: usize,
}

impl SeenLog {
    /// Opens (or creates) the log in `dir` (mode 0700).
    pub fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let path = dir.join(FILE);
        let mut nodes: HashMap<ServerId, VecDeque<String>> = HashMap::new();
        let mut lines = 0;
        if let Ok(f) = File::open(&path) {
            for line in BufReader::new(f).lines() {
                // A torn last line (a crash while appending) is skipped.
                let Ok(line) = line else { break };
                lines += 1;
                if let Some((node, id)) = parse(&line) {
                    remember(&mut nodes, node, id);
                }
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut log = Self { path, file, nodes, lines };
        if log.lines > COMPACT_AT {
            log.compact(None)?;
        }
        Ok(log)
    }

    #[must_use]
    pub fn contains(&self, node: ServerId, id: &str) -> bool {
        self.nodes.get(&node).is_some_and(|ids| ids.iter().any(|i| i == id))
    }

    /// Records a stored file; durable before it returns.
    pub fn insert(&mut self, node: ServerId, id: &str) -> io::Result<()> {
        writeln!(self.file, "{node} {id}")?;
        self.file.sync_data()?;
        self.lines += 1;
        remember(&mut self.nodes, node, id.to_owned());
        if self.lines > COMPACT_AT {
            self.compact(None)?;
        }
        Ok(())
    }

    /// Rewrites the log with the kept ids; with `active`, drops other nodes.
    pub fn compact(&mut self, active: Option<&HashSet<ServerId>>) -> io::Result<()> {
        if let Some(active) = active {
            self.nodes.retain(|n, _| active.contains(n));
        }
        let tmp = self.path.with_extension("tmp");
        {
            let mut w = io::BufWriter::new(File::create(&tmp)?);
            for (node, ids) in &self.nodes {
                for id in ids {
                    writeln!(w, "{node} {id}")?;
                }
            }
            w.into_inner().map_err(io::IntoInnerError::into_error)?.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        self.file = OpenOptions::new().append(true).open(&self.path)?;
        self.lines = self.nodes.values().map(VecDeque::len).sum();
        Ok(())
    }

    /// Nodes remembered.
    #[must_use]
    pub fn nodes(&self) -> usize {
        self.nodes.len()
    }
}

fn parse(line: &str) -> Option<(ServerId, String)> {
    let (node, id) = line.split_once(' ')?;
    let node = node.parse().ok()?;
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (node, id.to_owned()))
}

fn remember(nodes: &mut HashMap<ServerId, VecDeque<String>>, node: ServerId, id: String) {
    if !nodes.contains_key(&node) && nodes.len() >= MAX_NODES {
        return;
    }
    let ids = nodes.entry(node).or_default();
    if ids.iter().any(|i| *i == id) {
        return;
    }
    if ids.len() >= PER_NODE {
        ids.pop_front();
    }
    ids.push_back(id);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn id(i: usize) -> String {
        format!("{i:032x}")
    }

    #[test]
    fn ids_survive_a_restart_and_stay_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (ServerId::new(), ServerId::new());
        {
            let mut log = SeenLog::open(dir.path()).unwrap();
            for i in 0..PER_NODE + 10 {
                log.insert(a, &id(i)).unwrap();
            }
            log.insert(b, &id(1)).unwrap();
        }
        // A torn line from a crash mid-append is ignored.
        let mut f = OpenOptions::new().append(true).open(dir.path().join(FILE)).unwrap();
        write!(f, "{a} 00ab").unwrap();
        drop(f);
        let mut log = SeenLog::open(dir.path()).unwrap();
        assert!(log.contains(a, &id(PER_NODE + 9)), "remembered across a restart");
        assert!(!log.contains(a, &id(0)), "only the last ids per node");
        assert!(log.contains(b, &id(1)));
        assert!(!log.contains(a, &id(1_000)));
        log.compact(Some(&[a].into_iter().collect())).unwrap();
        assert_eq!(log.nodes(), 1, "nodes that left are dropped");
        let log = SeenLog::open(dir.path()).unwrap();
        assert!(log.contains(a, &id(PER_NODE + 9)) && !log.contains(b, &id(1)));
        assert_eq!(log.lines, PER_NODE);
    }
}
