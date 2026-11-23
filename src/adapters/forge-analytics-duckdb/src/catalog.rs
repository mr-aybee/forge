//! The files the store has, which of them are attached, and who uses them.
//!
//! `ATTACH` and `DETACH` act on the whole DuckDB instance, not on one
//! connection, so every use of a file goes through a [`Lease`]: a file is
//! attached on its first lease and only detached (or, once expired,
//! deleted) when no lease holds it. Lock order: `state`, then `admin`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use duckdb::Connection;

use crate::Error;
use crate::parts::{Family, Part};
use crate::schema;

#[derive(Debug)]
struct Slot {
    leases: u32,
    last_used: Instant,
}

#[derive(Debug, Default)]
struct State {
    /// Files on disk.
    known: BTreeSet<Part>,
    attached: HashMap<Part, Slot>,
    /// Event-time range of each file, `(min, max)` inclusive.
    ranges: HashMap<Part, (i64, i64)>,
    /// Past their retention: no new leases; deleted at the last release.
    doomed: HashSet<Part>,
}

/// The store's files.
#[derive(Debug)]
pub struct Catalog {
    root: PathBuf,
    admin: Mutex<Connection>,
    state: Mutex<State>,
}

/// Files held attached for one reader or writer.
#[derive(Debug)]
pub struct Lease {
    catalog: Arc<Catalog>,
    parts: Vec<Part>,
}

impl Lease {
    #[must_use]
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.catalog.release(&self.parts);
    }
}

/// `'..'` with quotes doubled.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Resource limits of the DuckDB instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub memory_mb: u32,
    pub threads: u32,
}

impl Catalog {
    /// Opens the store under `root`: the instance, `meta.duckdb`, the list
    /// of files and their ranges.
    pub fn open(root: &Path, limits: Limits) -> Result<Arc<Self>, Error> {
        for f in Family::ALL {
            std::fs::create_dir_all(root.join(f.dir()))?;
        }
        let temp = root.join("tmp");
        std::fs::create_dir_all(&temp)?;
        let config = duckdb::Config::default()
            .max_memory(&format!("{}MB", limits.memory_mb.max(64)))?
            .threads(i64::from(limits.threads.max(1)))?
            .enable_autoload_extension(false)?
            .with("temp_directory", temp.to_string_lossy())?;
        let admin = Connection::open_in_memory_with_flags(config)?;
        admin.execute_batch(&format!("ATTACH {} AS meta", quote(&root.join("meta.duckdb").to_string_lossy())))?;
        for s in schema::meta() {
            admin.execute_batch(s)?;
        }
        let mut state = State::default();
        for f in Family::ALL {
            for e in std::fs::read_dir(root.join(f.dir()))? {
                let name = e?.file_name();
                if let Some(p) = name.to_str().and_then(|n| Part::parse(f, n)) {
                    state.known.insert(p);
                }
            }
        }
        {
            let mut st = admin.prepare("SELECT family, part, min_t, max_t FROM meta.ranges")?;
            let rows = st.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
            })?;
            for row in rows {
                let (f, index, lo, hi) = row?;
                if let Some(family) = Family::parse(&f) {
                    let p = Part { family, index };
                    let e = state.ranges.entry(p).or_insert((lo, hi));
                    *e = (e.0.min(lo), e.1.max(hi));
                }
            }
        }
        Ok(Arc::new(Self { root: root.to_path_buf(), admin: Mutex::new(admin), state: Mutex::new(state) }))
    }

    /// A new connection to the instance.
    pub fn connect(&self) -> Result<Connection, Error> {
        Ok(lock(&self.admin).try_clone()?)
    }

    /// Runs `f` on the administrative connection.
    pub fn with_admin<R>(&self, f: impl FnOnce(&Connection) -> Result<R, Error>) -> Result<R, Error> {
        f(&lock(&self.admin))
    }

    /// Changes the instance's memory limit and threads.
    pub fn set_limits(&self, l: Limits) -> Result<(), Error> {
        self.with_admin(|c| {
            c.execute_batch(&format!(
                "SET memory_limit = '{}MB'; SET threads = {}",
                l.memory_mb.max(64),
                l.threads.max(1)
            ))?;
            Ok(())
        })
    }

    /// Holds `parts` attached. With `create`, a missing file is created
    /// with its family's tables; without, missing and expired files are
    /// left out of the lease.
    pub fn lease(self: &Arc<Self>, parts: &[Part], create: bool) -> Result<Lease, Error> {
        let mut st = lock(&self.state);
        let mut held = Vec::with_capacity(parts.len());
        let mut result = Ok(());
        for p in parts {
            if st.doomed.contains(p) || (!create && !st.known.contains(p)) {
                continue;
            }
            if !st.attached.contains_key(p) {
                if let Err(e) = self.attach(*p) {
                    result = Err(e);
                    break;
                }
                st.known.insert(*p);
                st.attached.insert(*p, Slot { leases: 0, last_used: Instant::now() });
            }
            if let Some(s) = st.attached.get_mut(p) {
                s.leases += 1;
                s.last_used = Instant::now();
                held.push(*p);
            }
        }
        drop(st);
        let lease = Lease { catalog: self.clone(), parts: held };
        result.map(|()| lease)
    }

    fn attach(&self, p: Part) -> Result<(), Error> {
        let path = p.path(&self.root);
        let alias = p.alias();
        let admin = lock(&self.admin);
        admin.execute_batch(&format!("ATTACH {} AS {alias}", quote(&path.to_string_lossy())))?;
        for s in schema::family(p.family, &alias) {
            if let Err(e) = admin.execute_batch(&s) {
                let _ = admin.execute_batch(&format!("DETACH {alias}"));
                return Err(e.into());
            }
        }
        Ok(())
    }

    fn release(&self, parts: &[Part]) {
        let mut st = lock(&self.state);
        for p in parts {
            if let Some(s) = st.attached.get_mut(p) {
                s.leases = s.leases.saturating_sub(1);
                s.last_used = Instant::now();
                if s.leases == 0 && st.doomed.contains(p) {
                    self.remove(&mut st, *p);
                }
            }
        }
    }

    /// Detaches and deletes an unleased file (`.wal` too).
    fn remove(&self, st: &mut State, p: Part) {
        if st.attached.remove(&p).is_some()
            && let Err(e) = lock(&self.admin).execute_batch(&format!("DETACH {}", p.alias()))
        {
            tracing::warn!(file = %p.file_name(), error = %e, "detaching an expired analytics file failed");
        }
        let path = p.path(&self.root);
        let wal = path.with_extension("duckdb.wal");
        for f in [&path, &wal] {
            match std::fs::remove_file(f) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(file = %f.display(), error = %e, "deleting an expired analytics file failed"),
            }
        }
        st.known.remove(&p);
        st.ranges.remove(&p);
        st.doomed.remove(&p);
        let sql = "DELETE FROM meta.ranges WHERE family = ? AND part = ?";
        if let Err(e) = lock(&self.admin).execute(sql, duckdb::params![p.family.dir(), p.index]) {
            tracing::warn!(error = %e, "forgetting an expired file's range failed");
        }
    }

    /// The files of `family` that may hold rows with an event time in
    /// `[from, to)`. A file without a recorded range is always included.
    #[must_use]
    pub fn overlapping(&self, family: Family, from: i64, to: i64) -> Vec<Part> {
        let st = lock(&self.state);
        st.known
            .iter()
            .filter(|p| p.family == family && !st.doomed.contains(p))
            .filter(|p| st.ranges.get(p).is_none_or(|(lo, hi)| *lo < to && *hi >= from))
            .copied()
            .collect()
    }

    /// Every file of `family` (not expired).
    #[must_use]
    pub fn files(&self, family: Family) -> Vec<Part> {
        let st = lock(&self.state);
        st.known.iter().filter(|p| p.family == family && !st.doomed.contains(p)).copied().collect()
    }

    /// Widens the recorded ranges; durable before it returns. Called before
    /// the rows commit, so a crash leaves a range too wide, never too narrow.
    pub fn widen(&self, updates: &[(Part, i64, i64)]) -> Result<(), Error> {
        let mut st = lock(&self.state);
        let mut changed = Vec::new();
        for (p, lo, hi) in updates {
            let next = match st.ranges.get(p) {
                Some((a, b)) if *a <= *lo && *b >= *hi => continue,
                Some((a, b)) => ((*a).min(*lo), (*b).max(*hi)),
                None => (*lo, *hi),
            };
            changed.push((*p, next));
        }
        if changed.is_empty() {
            return Ok(());
        }
        let admin = lock(&self.admin);
        admin.execute_batch("BEGIN")?;
        let r = (|| -> Result<(), Error> {
            for (p, (lo, hi)) in &changed {
                admin.execute(
                    "DELETE FROM meta.ranges WHERE family = ? AND part = ?",
                    duckdb::params![p.family.dir(), p.index],
                )?;
                admin.execute(
                    "INSERT INTO meta.ranges VALUES (?, ?, ?, ?)",
                    duckdb::params![p.family.dir(), p.index, lo, hi],
                )?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => admin.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = admin.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        for (p, r) in changed {
            st.ranges.insert(p, r);
        }
        Ok(())
    }

    /// Marks the files of `family` before `keep_from` expired and deletes
    /// those no one holds. Returns the files dropped or doomed.
    pub fn expire(&self, family: Family, keep_from: i64) -> usize {
        let mut st = lock(&self.state);
        let old: Vec<Part> = st.known.iter().filter(|p| p.family == family && p.index < keep_from).copied().collect();
        for p in &old {
            st.doomed.insert(*p);
            if st.attached.get(p).is_none_or(|s| s.leases == 0) {
                self.remove(&mut st, *p);
            }
        }
        old.len()
    }

    /// Checkpoints `p` when attached.
    pub fn checkpoint(&self, p: Part) -> Result<(), Error> {
        let st = lock(&self.state);
        if st.attached.contains_key(&p) {
            lock(&self.admin).execute_batch(&format!("CHECKPOINT {}", p.alias()))?;
        }
        Ok(())
    }

    /// Detaches files no lease held for `idle`, except `keep`.
    pub fn detach_idle(&self, idle: Duration, keep: &HashSet<Part>) {
        let mut st = lock(&self.state);
        let now = Instant::now();
        let idle_parts: Vec<Part> = st
            .attached
            .iter()
            .filter(|(p, s)| s.leases == 0 && now.duration_since(s.last_used) >= idle && !keep.contains(p))
            .map(|(p, _)| *p)
            .collect();
        for p in idle_parts {
            let admin = lock(&self.admin);
            // A checkpoint first, so the file is complete without its WAL.
            let r = admin.execute_batch(&format!("CHECKPOINT {0}; DETACH {0}", p.alias()));
            drop(admin);
            match r {
                Ok(()) => {
                    st.attached.remove(&p);
                }
                Err(e) => tracing::warn!(file = %p.file_name(), error = %e, "detaching an idle analytics file failed"),
            }
        }
    }

    /// Files attached now (tests and status).
    #[must_use]
    pub fn attached(&self) -> Vec<Part> {
        let mut v: Vec<Part> = lock(&self.state).attached.keys().copied().collect();
        v.sort();
        v
    }

    /// Files on disk (tests and status).
    #[must_use]
    pub fn known(&self) -> Vec<Part> {
        lock(&self.state).known.iter().copied().collect()
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}
