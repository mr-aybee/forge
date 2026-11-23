//! Permissions, owners, ACLs and archives (issue #2, step 6).
//!
//! Everything goes through the confined [`Fs`] backend, so links are never
//! followed and protected paths are refused here exactly as for other
//! operations. Archives are checked entry by entry: names that are absolute
//! or climb with `..` are refused (zip-slip), links and device files are
//! skipped and reported, and the entry count, total size and compression
//! ratio are capped (zip bombs). Extraction writes into a hidden temporary
//! folder next to the destination and renames it into place at the end, so
//! a failed or refused archive leaves nothing behind.

use std::io::{Read, Seek, SeekFrom, Write};

use forge_core::files::{
    ARCHIVE_MAX_BYTES, ARCHIVE_MAX_ENTRIES, ARCHIVE_MAX_RATIO, ArchiveFormat, FilePath, RootSpec, UPLOAD_PREFIX,
    is_forge_internal,
};
use serde_json::{Value, json};

use super::{
    FileIo, Fs, FsError, Ids, Kind, Meta, Ops, b64, c, now_ms, protected, random_id, read_range, remove_tree, with,
};

/// Changes listed in a permission/owner result (the rest are counted).
const SAMPLE: usize = 50;
/// Temporary archives older than this are removed.
const TEMP_MAX_AGE_MS: i64 = 60 * 60 * 1000;

/// Describes an entry's current or target state (`"0644"`, `"www-data:www-data"`).
type Describe<'a> = dyn Fn(&Meta) -> String + 'a;
/// Applies a change to one entry.
type Apply<'a> = dyn Fn(&[String], &Meta) -> Result<(), String> + 'a;
/// Adds one directory or file to an archive being written.
type AddEntry<'a, T> = dyn FnMut(&str, &Meta, Option<&T>) -> Result<(), String> + 'a;

/// `std::io` reader over a [`FileIo`].
struct Reader<'a, T: FileIo> {
    f: &'a T,
    pos: u64,
    len: u64,
}

impl<T: FileIo> Read for Reader<'_, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.f.read_at(buf, self.pos).map_err(std::io::Error::other)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<T: FileIo> Seek for Reader<'_, T> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => i128::from(p),
            SeekFrom::End(d) => i128::from(self.len) + i128::from(d),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
        };
        self.pos = u64::try_from(p).map_err(|_| std::io::Error::other("seek before start"))?;
        Ok(self.pos)
    }
}

/// `std::io` writer over a [`FileIo`].
struct Writer<'a, T: FileIo> {
    f: &'a T,
    pos: u64,
    end: u64,
}

impl<T: FileIo> Write for Writer<'_, T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.f.write_all_at(buf, self.pos).map_err(std::io::Error::other)?;
        self.pos += buf.len() as u64;
        self.end = self.end.max(self.pos);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<T: FileIo> Seek for Writer<'_, T> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => i128::from(p),
            SeekFrom::End(d) => i128::from(self.end) + i128::from(d),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
        };
        self.pos = u64::try_from(p).map_err(|_| std::io::Error::other("seek before start"))?;
        Ok(self.pos)
    }
}

/// Checks an archive entry name and returns its components. Refuses
/// absolute paths, drive letters, `..`, NUL and names Forge reserves.
pub(crate) fn safe_entry(raw: &str) -> Result<Vec<String>, String> {
    let name = raw.replace('\\', "/");
    let bad = |why: &str| Err(format!("unsafe archive entry `{}`: {why}", raw.chars().take(200).collect::<String>()));
    if name.starts_with('/') {
        return bad("absolute path");
    }
    let b = name.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return bad("drive path");
    }
    if name.contains('\0') {
        return bad("NUL byte");
    }
    let mut out = Vec::new();
    for comp in name.split('/') {
        match comp {
            "" | "." => {}
            ".." => return bad("`..` climbs out of the destination"),
            c if c.len() > 255 => return bad("name too long"),
            c if is_forge_internal(c) => return bad("reserved name"),
            c => out.push(c.to_owned()),
        }
    }
    if out.len() > 64 {
        return bad("too deep");
    }
    #[cfg(windows)]
    for comp in &out {
        super::windows::valid_name(comp).map_err(|e| format!("unsafe archive entry `{raw}`: {e}"))?;
    }
    Ok(out)
}

/// Archive limits being consumed.
struct Caps {
    entries: usize,
    bytes: u64,
}

impl Caps {
    fn entry(&mut self) -> Result<(), String> {
        self.entries += 1;
        if self.entries > ARCHIVE_MAX_ENTRIES {
            return Err(format!("the archive has more than {ARCHIVE_MAX_ENTRIES} entries"));
        }
        Ok(())
    }

    fn bytes(&mut self, n: u64, archive_size: u64) -> Result<(), String> {
        self.bytes = self.bytes.saturating_add(n);
        if self.bytes > ARCHIVE_MAX_BYTES {
            return Err("the archive expands to more than 4 GiB".into());
        }
        if self.bytes > 1024 * 1024 && self.bytes > archive_size.saturating_mul(ARCHIVE_MAX_RATIO) {
            return Err("the archive expands more than 200 times its size (possible zip bomb)".into());
        }
        Ok(())
    }
}

impl<F: Fs> Ops<'_, F> {
    // ---- permissions and owners ----------------------------------------------

    /// Applies `f` to `path` (and below, with `recursive`), skipping links,
    /// protected paths and special files. Returns counts and a sample.
    fn walk_apply(
        &self,
        path: &FilePath,
        recursive: bool,
        dry_run: bool,
        describe: &Describe<'_>,
        target: &Describe<'_>,
        apply: &Apply<'_>,
    ) -> Result<Value, String> {
        let mut stack = vec![c(path).to_vec()];
        let (mut changed, mut unchanged, mut skipped, mut seen) = (0usize, 0usize, 0usize, 0usize);
        let mut sample = Vec::new();
        while let Some(p) = stack.pop() {
            seen += 1;
            if seen > super::MAX_DELETE_ENTRIES {
                return Err("too many entries to change at once".into());
            }
            let fp = FilePath::from_components(&p);
            if protected(self.spec, &fp).is_err() {
                skipped += 1;
                continue;
            }
            // The requested path must exist and be reachable (no links);
            // entries that vanish while walking below it are skipped.
            let m = match self.fs.stat(&p) {
                Ok(m) => m,
                Err(e) if p.len() == path.components().len() => return Err(e),
                Err(_) => continue,
            };
            if !matches!(m.kind, Kind::File | Kind::Dir) {
                skipped += 1;
                continue;
            }
            let (before, after) = (describe(&m), target(&m));
            if before == after {
                unchanged += 1;
            } else {
                if !dry_run {
                    apply(&p, &m)?;
                }
                changed += 1;
                if sample.len() < SAMPLE {
                    sample.push(json!({ "path": fp.to_string(), "before": before, "after": after }));
                }
            }
            if recursive && m.kind == Kind::Dir {
                let (names, _, _) = self.fs.read_dir(&p)?;
                stack.extend(names.into_iter().filter(|n| !is_forge_internal(n)).map(|n| with(&p, n)));
            }
        }
        Ok(
            json!({ "path": path.to_string(), "dry_run": dry_run, "changed": changed, "unchanged": unchanged, "skipped": skipped, "sample": sample }),
        )
    }

    pub(crate) fn set_permissions(
        &self,
        path: &FilePath,
        mode: u32,
        recursive: bool,
        dry_run: bool,
    ) -> Result<Value, String> {
        let describe = |m: &Meta| format!("{:04o}", m.mode.unwrap_or(0) & 0o7777);
        let target = |_: &Meta| format!("{:04o}", mode & 0o7777);
        let apply = |p: &[String], _: &Meta| self.fs.set_mode(p, mode & 0o7777);
        self.walk_apply(path, recursive, dry_run, &describe, &target, &apply)
    }

    pub(crate) fn set_owner(
        &self,
        path: &FilePath,
        owner: Option<&str>,
        group: Option<&str>,
        recursive: bool,
        dry_run: bool,
    ) -> Result<Value, String> {
        let ids = Ids::get();
        let uid = match owner {
            Some(n) => Some(
                ids.users
                    .iter()
                    .find(|(_, v)| v.as_str() == n)
                    .map(|(k, _)| *k)
                    .ok_or_else(|| format!("no user `{n}` on this node"))?,
            ),
            None => None,
        };
        let gid = match group {
            Some(n) => Some(
                ids.groups
                    .iter()
                    .find(|(_, v)| v.as_str() == n)
                    .map(|(k, _)| *k)
                    .ok_or_else(|| format!("no group `{n}` on this node"))?,
            ),
            None => None,
        };
        let name = |id: Option<u32>, map: &std::collections::HashMap<u32, String>| {
            id.map_or_else(|| "?".to_owned(), |i| map.get(&i).cloned().unwrap_or_else(|| i.to_string()))
        };
        let describe = |m: &Meta| format!("{}:{}", name(m.uid, &ids.users), name(m.gid, &ids.groups));
        let target = |m: &Meta| format!("{}:{}", name(uid.or(m.uid), &ids.users), name(gid.or(m.gid), &ids.groups));
        let apply = |p: &[String], _: &Meta| self.fs.set_owner(p, uid, gid);
        self.walk_apply(path, recursive, dry_run, &describe, &target, &apply)
    }

    pub(crate) fn acl(&self, path: &FilePath) -> Result<Value, String> {
        let m = self.fs.stat(c(path))?;
        let text = self.fs.acl(c(path))?;
        Ok(json!({ "path": path.to_string(), "acl": text, "mode": m.mode.map(|x| format!("{:04o}", x & 0o7777)) }))
    }

    // ---- archives ------------------------------------------------------------

    pub(crate) fn compress(
        &self,
        sources: &[FilePath],
        dest: Option<&FilePath>,
        format: ArchiveFormat,
    ) -> Result<Value, String> {
        if cfg!(windows) && format != ArchiveFormat::Zip {
            return Err("Windows nodes create .zip archives only".into());
        }
        let store;
        let (out_fs, tmp, temp_id): (&F, Vec<String>, Option<String>) = match dest {
            Some(d) => {
                if self.fs.stat(c(d)).is_ok() {
                    return Err("the archive already exists; choose another name".into());
                }
                (self.fs, with(c(&d.parent()), format!("{UPLOAD_PREFIX}{}.part", random_id())), None)
            }
            None => {
                store = self.fs.store("tmp")?;
                prune_temp(&store);
                let id = format!("{}-{}", random_id(), self.temp_tag());
                (&store, vec![format!("{id}.part")], Some(id))
            }
        };
        let f = out_fs.open_write(&tmp, true, true)?;
        let mut caps = Caps { entries: 0, bytes: 0 };
        let mut skipped: Vec<Value> = Vec::new();
        let result = (|| -> Result<u64, String> {
            let w = Writer { f: &f, pos: 0, end: 0 };
            match format {
                ArchiveFormat::Zip => {
                    let mut z = zip::ZipWriter::new(w);
                    for s in sources {
                        self.add_tree(s, &mut caps, &mut skipped, &mut |rel, m, f| {
                            let opts = zip::write::SimpleFileOptions::default()
                                .compression_method(zip::CompressionMethod::Deflated)
                                .unix_permissions(if m.kind == Kind::Dir { 0o755 } else { 0o644 });
                            if m.kind == Kind::Dir {
                                z.add_directory(format!("{rel}/"), opts).map_err(|e| e.to_string())
                            } else {
                                z.start_file(rel, opts).map_err(|e| e.to_string())?;
                                copy_into(f, m.size, &mut z)
                            }
                        })?;
                    }
                    let w = z.finish().map_err(|e| e.to_string())?;
                    Ok(w.end)
                }
                ArchiveFormat::TarGz | ArchiveFormat::Tar => {
                    let gz = flate2::write::GzEncoder::new(w, flate2::Compression::default());
                    let mut t = tar::Builder::new(gz);
                    for s in sources {
                        self.add_tree(s, &mut caps, &mut skipped, &mut |rel, m, f| {
                            let mut h = tar::Header::new_gnu();
                            h.set_mtime(u64::try_from(m.modified_ms / 1000).unwrap_or(0));
                            if m.kind == Kind::Dir {
                                h.set_entry_type(tar::EntryType::Directory);
                                h.set_mode(0o755);
                                h.set_size(0);
                                h.set_cksum();
                                t.append_data(&mut h, format!("{rel}/"), std::io::empty()).map_err(|e| e.to_string())
                            } else {
                                h.set_entry_type(tar::EntryType::Regular);
                                h.set_mode(0o644);
                                h.set_size(m.size);
                                h.set_cksum();
                                let file = f.ok_or("missing file")?;
                                let r = Reader { f: file, pos: 0, len: m.size };
                                t.append_data(&mut h, rel, r.take(m.size)).map_err(|e| e.to_string())
                            }
                        })?;
                    }
                    let gz = t.into_inner().map_err(|e| e.to_string())?;
                    let w = gz.finish().map_err(|e| e.to_string())?;
                    Ok(w.end)
                }
            }
        })();
        let size = match result {
            Ok(n) => n,
            Err(e) => {
                let _ = out_fs.remove_file(&tmp);
                return Err(e);
            }
        };
        f.sync()?;
        if dest.is_some() {
            // Written private (0600) while it fills; a finished archive in a
            // folder gets the usual 0644 like any new file (download-only
            // temporary archives stay private).
            f.chown_like(&Meta { mode: Some(0o644), ..Meta::default() })?;
        }
        drop(f);
        match (dest, temp_id) {
            (Some(d), _) => {
                out_fs.inherit_owner(&tmp)?;
                out_fs.rename(&tmp, out_fs, c(d), false).map_err(FsError::msg)?;
                Ok(
                    json!({ "dest": d.to_string(), "size": size, "entries": caps.entries, "bytes": caps.bytes, "skipped": skipped }),
                )
            }
            (None, Some(id)) => {
                let ext = if format == ArchiveFormat::Zip { "zip" } else { "tar.gz" };
                out_fs.rename(&tmp, out_fs, &[format!("{id}.{ext}")], false).map_err(FsError::msg)?;
                Ok(
                    json!({ "temp_id": id, "ext": ext, "size": size, "entries": caps.entries, "bytes": caps.bytes, "skipped": skipped }),
                )
            }
            (None, None) => Err("internal: no destination".into()),
        }
    }

    /// Walks `src` (file or tree), naming entries relative to its parent,
    /// and calls `add` for each directory and regular file.
    fn add_tree(
        &self,
        src: &FilePath,
        caps: &mut Caps,
        skipped: &mut Vec<Value>,
        add: &mut AddEntry<'_, F::File>,
    ) -> Result<(), String> {
        let parent_len = src.components().len().saturating_sub(1);
        let mut stack = vec![c(src).to_vec()];
        while let Some(p) = stack.pop() {
            let fp = FilePath::from_components(&p);
            let rel = p[parent_len..].join("/");
            if protected(self.spec, &fp).is_err() {
                skipped.push(json!({ "path": fp.to_string(), "reason": "protected" }));
                continue;
            }
            let m = self.fs.stat(&p)?;
            match m.kind {
                Kind::Dir => {
                    caps.entry()?;
                    add(&rel, &m, None)?;
                    let (mut names, _, _) = self.fs.read_dir(&p)?;
                    names.sort_unstable_by(|a, b| b.cmp(a));
                    stack.extend(names.into_iter().filter(|n| !is_forge_internal(n)).map(|n| with(&p, n)));
                }
                Kind::File => {
                    caps.entry()?;
                    caps.bytes(m.size, u64::MAX)?;
                    let (f, m) = self.open_regular(&p)?;
                    add(&rel, &m, Some(&f))?;
                }
                Kind::Symlink | Kind::Other => {
                    skipped.push(json!({ "path": fp.to_string(), "reason": if m.kind == Kind::Symlink { "link" } else { "special file" } }));
                }
            }
        }
        Ok(())
    }

    /// Unpacks `archive` into a hidden folder next to `dest`, then moves it
    /// into place. With `replace`, an existing folder `dest` is swapped for
    /// it: atomically on Linux (`RENAME_EXCHANGE`), so a site served from
    /// `dest` never answers 404; elsewhere with two quick renames. The old
    /// content is removed afterwards.
    pub(crate) fn extract(&self, archive: &FilePath, dest: &FilePath, replace: bool) -> Result<Value, String> {
        let format = ArchiveFormat::from_name(archive.name().unwrap_or_default()).ok_or("unknown archive type")?;
        let existing = self.fs.stat(c(dest)).ok();
        match &existing {
            Some(_) if !replace => {
                return Err("the destination already exists; choose a new folder or use `replace`".into());
            }
            Some(m) if m.kind != Kind::Dir => return Err(format!("`{dest}` exists and is not a folder")),
            _ => {}
        }
        let parent = c(&dest.parent()).to_vec();
        if self.fs.stat(&parent).map(|m| m.kind) != Ok(Kind::Dir) {
            return Err(self.missing_folder("the destination's folder", &dest.parent()));
        }
        let (src, am) = self.open_regular(c(archive))?;
        let tmp = with(&parent, format!("{UPLOAD_PREFIX}{}.part", random_id()));
        self.fs.mkdir(&tmp).map_err(FsError::msg)?;
        let mut caps = Caps { entries: 0, bytes: 0 };
        let mut skipped: Vec<Value> = Vec::new();
        let r = self.extract_into(format, &src, am.size, &tmp, dest, &mut caps, &mut skipped);
        if let Err(e) = r {
            let _ = remove_tree(self.fs, &tmp, &mut 0);
            return Err(e);
        }
        if existing.is_some() {
            // The new folder gets its parent folder's owner before it goes live.
            if let Err(e) = self.fs.inherit_owner(&tmp) {
                let _ = remove_tree(self.fs, &tmp, &mut 0);
                return Err(e);
            }
            let old = self.swap_in(&tmp, c(dest), &parent).inspect_err(|_| {
                let _ = remove_tree(self.fs, &tmp, &mut 0);
            })?;
            // Best effort: the new content is live; leftovers are hidden
            // (`.forge-upload-…`) and never listed.
            let _ = remove_tree(self.fs, &old, &mut 0);
        } else {
            if let Err(e) = self.fs.rename(&tmp, self.fs, c(dest), false) {
                let _ = remove_tree(self.fs, &tmp, &mut 0);
                return Err(e.msg());
            }
            self.fs.inherit_owner(c(dest))?;
        }
        Ok(json!({
            "dest": dest.to_string(),
            "entries": caps.entries,
            "bytes": caps.bytes,
            "skipped": skipped,
            "replaced": existing.is_some(),
        }))
    }

    /// Puts the folder `tmp` at `dest` and returns where the old `dest` now
    /// is (a hidden sibling, to be removed).
    fn swap_in(&self, tmp: &[String], dest: &[String], parent: &[String]) -> Result<Vec<String>, String> {
        if self.fs.exchange(tmp, dest)? {
            // `tmp` now holds the old content.
            return Ok(tmp.to_vec());
        }
        let old = with(parent, format!("{UPLOAD_PREFIX}{}.old", random_id()));
        self.fs.rename(dest, self.fs, &old, false).map_err(FsError::msg)?;
        if let Err(e) = self.fs.rename(tmp, self.fs, dest, false) {
            // Put the old content back.
            let _ = self.fs.rename(&old, self.fs, dest, false);
            return Err(e.msg());
        }
        Ok(old)
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_into(
        &self,
        format: ArchiveFormat,
        src: &F::File,
        size: u64,
        tmp: &[String],
        dest: &FilePath,
        caps: &mut Caps,
        skipped: &mut Vec<Value>,
    ) -> Result<(), String> {
        let reader = Reader { f: src, pos: 0, len: size };
        match format {
            ArchiveFormat::Zip => {
                let mut z = zip::ZipArchive::new(reader).map_err(|e| format!("not a valid zip archive: {e}"))?;
                if z.len() > ARCHIVE_MAX_ENTRIES {
                    return Err(format!("the archive has more than {ARCHIVE_MAX_ENTRIES} entries"));
                }
                let declared: u64 = (0..z.len()).filter_map(|i| z.by_index_raw(i).ok().map(|e| e.size())).sum();
                if let Some(free) = self.fs.free_bytes(tmp)
                    && free < declared
                {
                    return Err("not enough free space to extract".into());
                }
                for i in 0..z.len() {
                    let mut e = z.by_index(i).map_err(|e| format!("damaged zip archive: {e}"))?;
                    let name = e.name().map_err(|err| format!("damaged zip archive: {err}"))?.into_owned();
                    let comps = safe_entry(&name)?;
                    caps.entry()?;
                    if comps.is_empty() {
                        continue;
                    }
                    let mode = e.unix_mode().unwrap_or(0);
                    let kind = mode & 0o170_000;
                    if e.is_dir() {
                        self.mkdirs(tmp, &comps, dest)?;
                    } else if kind == 0o120_000 || (kind != 0 && kind != 0o100_000) {
                        skipped.push(
                            json!({ "entry": name, "reason": if kind == 0o120_000 { "link" } else { "special file" } }),
                        );
                    } else {
                        if e.size() > 1024 * 1024 && e.size() > e.compressed_size().saturating_mul(ARCHIVE_MAX_RATIO) {
                            return Err(format!("entry `{name}` expands more than 200 times (possible zip bomb)"));
                        }
                        self.write_entry(tmp, &comps, dest, &mut e, size, caps)?;
                    }
                }
                Ok(())
            }
            ArchiveFormat::TarGz | ArchiveFormat::Tar => {
                let inner: Box<dyn Read + '_> = if format == ArchiveFormat::Tar {
                    Box::new(reader)
                } else {
                    Box::new(flate2::read::GzDecoder::new(reader))
                };
                let mut t = tar::Archive::new(inner);
                for entry in t.entries().map_err(|e| format!("not a valid tar archive: {e}"))? {
                    let mut e = entry.map_err(|e| format!("damaged tar archive: {e}"))?;
                    let name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
                    let comps = safe_entry(&name)?;
                    caps.entry()?;
                    if comps.is_empty() {
                        continue;
                    }
                    match e.header().entry_type() {
                        tar::EntryType::Directory => self.mkdirs(tmp, &comps, dest)?,
                        tar::EntryType::Regular | tar::EntryType::Continuous => {
                            self.write_entry(tmp, &comps, dest, &mut e, size, caps)?;
                        }
                        // Long-name and PAX headers are handled by the reader itself.
                        tar::EntryType::XGlobalHeader
                        | tar::EntryType::XHeader
                        | tar::EntryType::GNULongName
                        | tar::EntryType::GNULongLink => {}
                        tar::EntryType::Symlink | tar::EntryType::Link => {
                            skipped.push(json!({ "entry": name, "reason": "link" }));
                        }
                        _ => skipped.push(json!({ "entry": name, "reason": "special file" })),
                    }
                }
                Ok(())
            }
        }
    }

    /// Creates the folders of `comps` inside `tmp` (no links followed).
    fn mkdirs(&self, tmp: &[String], comps: &[String], dest: &FilePath) -> Result<(), String> {
        let mut p = tmp.to_vec();
        for (i, comp) in comps.iter().enumerate() {
            p.push(comp.clone());
            protected(self.spec, &dest.append(&FilePath::from_components(&comps[..=i])))?;
            match self.fs.mkdir(&p) {
                Ok(()) | Err(FsError::Exists) => {}
                Err(e) => return Err(e.msg()),
            }
            if self.fs.stat(&p)?.kind != Kind::Dir {
                return Err(format!("archive entry `{}` collides with a file", comps[..=i].join("/")));
            }
        }
        Ok(())
    }

    fn write_entry(
        &self,
        tmp: &[String],
        comps: &[String],
        dest: &FilePath,
        r: &mut dyn Read,
        archive_size: u64,
        caps: &mut Caps,
    ) -> Result<(), String> {
        let (name, parents) = comps.split_last().ok_or("empty entry")?;
        self.mkdirs(tmp, parents, dest)?;
        protected(self.spec, &dest.append(&FilePath::from_components(comps)))?;
        let mut p = tmp.to_vec();
        p.extend(comps.iter().cloned());
        if self.fs.stat(&p).is_ok() {
            return Err(format!("archive entry `{}` appears twice", comps.join("/")));
        }
        let f = self.fs.open_write(&p, true, true)?;
        f.chown_like(&Meta { mode: Some(0o644), ..Meta::default() })?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut off = 0u64;
        loop {
            let n = r.read(&mut buf).map_err(|e| format!("damaged archive entry `{name}`: {e}"))?;
            if n == 0 {
                break;
            }
            caps.bytes(n as u64, archive_size)?;
            f.write_all_at(&buf[..n], off)?;
            off += n as u64;
        }
        Ok(())
    }

    /// Binds a temporary archive to the root it was made from: the store is
    /// shared by every root of the node, so an id only opens through the
    /// same root (the control plane also binds it to its creator).
    fn temp_tag(&self) -> String {
        let spec = serde_json::to_string(self.spec).unwrap_or_default();
        let d = ring::digest::digest(&ring::digest::SHA256, spec.as_bytes());
        d.as_ref()[..6].iter().map(|b| format!("{b:02x}")).collect()
    }

    fn own_temp(&self, id: &str) -> Result<(), String> {
        match id.rsplit_once('-') {
            Some((_, tag)) if tag == self.temp_tag() => Ok(()),
            _ => Err("that archive is gone".into()),
        }
    }

    pub(crate) fn temp_read(&self, id: &str, offset: u64, length: u32) -> Result<Value, String> {
        self.own_temp(id)?;
        let store = self.fs.store("tmp")?;
        let (names, _, _) = store.read_dir(&[])?;
        let name = names
            .into_iter()
            .find(|n| n.starts_with(&format!("{id}.")) && !n.ends_with(".part"))
            .ok_or("that archive is gone")?;
        let ext = name.split_once('.').map_or("zip", |(_, e)| e).to_owned();
        let f = store.open_read(&[name])?;
        let m = f.meta()?;
        let data = read_range(&f, offset, m.size.saturating_sub(offset).min(u64::from(length)))?;
        Ok(json!({
            "ext": ext,
            "offset": offset,
            "length": data.len(),
            "size": m.size,
            "eof": offset + data.len() as u64 >= m.size,
            "content_b64": base64::Engine::encode(&b64(), &data),
        }))
    }

    pub(crate) fn temp_delete(&self, id: &str) -> Result<Value, String> {
        self.own_temp(id)?;
        let store = self.fs.store("tmp")?;
        let (names, _, _) = store.read_dir(&[])?;
        for n in names.into_iter().filter(|n| n.starts_with(&format!("{id}."))) {
            let _ = store.remove_file(&[n]);
        }
        Ok(json!({ "deleted": true }))
    }
}

fn copy_into<T: FileIo>(f: Option<&T>, size: u64, w: &mut dyn Write) -> Result<(), String> {
    let f = f.ok_or("missing file")?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut off = 0u64;
    while off < size {
        let n = f.read_at(&mut buf, off)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        off += n as u64;
    }
    Ok(())
}

/// Removes temporary archives older than an hour.
fn prune_temp<F: Fs>(store: &F) {
    let Ok((names, _, _)) = store.read_dir(&[]) else { return };
    let cutoff = now_ms() - TEMP_MAX_AGE_MS;
    for n in names {
        if let Ok(m) = store.stat(std::slice::from_ref(&n))
            && m.modified_ms < cutoff
        {
            let _ = store.remove_file(&[n]);
        }
    }
}

/// Root kinds that hold permissions Forge may change.
#[must_use]
pub(crate) fn supports_permissions(spec: &RootSpec) -> bool {
    cfg!(unix) && !matches!(spec, RootSpec::Container { .. })
}
