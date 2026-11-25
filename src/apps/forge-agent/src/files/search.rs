//! Bounded filename and content search inside a root. Walks depth-first in
//! sorted order (so `after` can resume a truncated search), never follows
//! links, skips protected paths, binary files and files over the size limit,
//! and stops at the first limit reached: matches, result bytes, bytes
//! scanned or time.

use std::time::{Duration, Instant};

use forge_core::files::{
    FilePath, RootSpec, SEARCH_MAX_MATCHES, SEARCH_MAX_RESULT_BYTES, SEARCH_MAX_SCANNED_BYTES, SEARCH_MAX_SECONDS,
    SearchSpec, is_forge_internal,
};
use regex::bytes::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::{Value, json};

use super::{FileIo, Fs, Kind, protected, read_range, with};

#[derive(Debug, Serialize)]
struct LineMatch {
    line: u64,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    before: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    after: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FileHit {
    path: String,
    size: u64,
    modified_ms: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    matches: Vec<LineMatch>,
}

/// Why the walk stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Complete,
    Truncated,
    Timeout,
}

struct State {
    deadline: Instant,
    matches: usize,
    result_bytes: usize,
    scanned_bytes: u64,
    scanned_files: u64,
    hits: Vec<FileHit>,
    stop: Option<Status>,
    /// Last file fully handled (resume point).
    last: Option<String>,
}

/// Longest line kept in a result.
const MAX_LINE: usize = 400;

fn line_text(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    let s = s.trim_end_matches(['\r', '\n']);
    if s.len() > MAX_LINE {
        let mut cut = MAX_LINE;

        while !s.is_char_boundary(cut) {
            cut -= 1;
        }

        format!("{}…", &s[..cut])
    } else {
        s.to_owned()
    }
}

fn build_regex(spec: &SearchSpec) -> Result<Option<Regex>, String> {
    if spec.content.is_empty() {
        return Ok(None);
    }

    let pat = if spec.regex { spec.content.clone() } else { regex::escape(&spec.content) };
    let pat = if spec.whole_word { format!(r"\b(?:{pat})\b") } else { pat };

    RegexBuilder::new(&pat)
        .case_insensitive(!spec.case_sensitive)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map(Some)
        .map_err(|e| format!("invalid pattern: {e}"))
}

pub(crate) fn run<F: Fs>(fs: &F, root: &RootSpec, start: &FilePath, spec: &SearchSpec) -> Result<Value, String> {
    let re = build_regex(spec)?;
    let dir = fs.stat(start.components())?;
    if dir.kind != Kind::Dir {
        return Err(format!("`{start}` is not a directory"));
    }

    let mut st = State {
        deadline: Instant::now() + Duration::from_secs(SEARCH_MAX_SECONDS),
        matches: 0,
        result_bytes: 0,
        scanned_bytes: 0,
        scanned_files: 0,
        hits: Vec::new(),
        stop: None,
        last: None,
    };
    walk(fs, root, start, &[], 0, spec, re.as_ref(), &mut st);
    let status = st.stop.unwrap_or(Status::Complete);

    Ok(json!({
        "path": start.to_string(),
        "status": status,
        // Resume point for a truncated or timed-out search.
        "next": if status == Status::Complete { None } else { st.last.clone() },
        "files": st.hits,
        "matches": st.matches,
        "scanned_files": st.scanned_files,
        "scanned_bytes": st.scanned_bytes,
    }))
}

#[allow(clippy::too_many_arguments)]
fn walk<F: Fs>(
    fs: &F,
    root: &RootSpec,
    start: &FilePath,
    rel: &[String],
    depth: u32,
    spec: &SearchSpec,
    re: Option<&Regex>,
    st: &mut State,
) {
    let here = start.append(&FilePath::from_components(rel));
    let Ok((mut names, _, _)) = fs.read_dir(here.components()) else { return };
    names.sort_unstable();

    for name in names {
        if st.stop.is_some() {
            return;
        }

        if Instant::now() >= st.deadline {
            st.stop = Some(Status::Timeout);
            return;
        }

        if is_forge_internal(&name) || (!spec.hidden && name.starts_with('.')) {
            continue;
        }

        let rel_child = with(rel, name.clone());
        let rel_str = rel_child.join("/");

        // Resume: skip everything up to and including the previous stop.
        if let Some(after) = &spec.after
            && rel_str.as_str() <= after.as_str()
            && !after.starts_with(&format!("{rel_str}/"))
        {
            continue;
        }

        let full = here.append(&FilePath::from_components(std::slice::from_ref(&name)));
        if protected(root, &full).is_err() {
            continue;
        }

        let Ok(m) = fs.stat(full.components()) else { continue };
        if !spec.hidden && m.hidden {
            continue;
        }

        match m.kind {
            Kind::Dir => {
                if depth < spec.max_depth && !excluded(spec, &rel_str) {
                    walk(fs, root, start, &rel_child, depth + 1, spec, re, st);
                }
            }
            Kind::File => {
                if excluded(spec, &rel_str)
                    || !included(spec, &rel_str)
                    || (!spec.name.is_empty() && !super::glob::matches(&spec.name, &name, spec.case_sensitive))
                    || m.modified_ms < spec.modified_since
                {
                    continue;
                }

                st.scanned_files += 1;

                let hit = match re {
                    None => Some(Vec::new()),
                    Some(re) if m.size <= spec.max_file_size => content(fs, &full, m.size, re, spec, st),
                    Some(_) => None,
                };

                if let Some(matches) = hit {
                    let size = rel_str.len() + matches.iter().map(|l| l.text.len() + 16).sum::<usize>();
                    if st.result_bytes + size > SEARCH_MAX_RESULT_BYTES {
                        st.stop = Some(Status::Truncated);
                        return;
                    }

                    st.result_bytes += size;
                    st.matches += matches.len().max(1);
                    st.hits.push(FileHit { path: rel_str.clone(), size: m.size, modified_ms: m.modified_ms, matches });

                    if st.matches >= SEARCH_MAX_MATCHES {
                        st.last = Some(rel_str);
                        st.stop = Some(Status::Truncated);

                        return;
                    }
                }

                st.last = Some(rel_str);
            }
            _ => {}
        }
    }
}

fn excluded(spec: &SearchSpec, rel: &str) -> bool {
    spec.exclude.iter().any(|g| super::glob::matches(g, rel, spec.case_sensitive))
}

fn included(spec: &SearchSpec, rel: &str) -> bool {
    spec.include.is_empty() || spec.include.iter().any(|g| super::glob::matches(g, rel, spec.case_sensitive))
}

/// Bytes read per step of a content scan.
const SCAN_CHUNK: u64 = 64 * 1024;
/// A "line" longer than this is matched as is and dropped from memory.
const MAX_SCAN_LINE: usize = 1024 * 1024;

/// Streaming line matcher: keeps only `context` previous lines and the
/// matches still owed `after` context.
struct Scan<'r> {
    re: &'r Regex,
    ctx: usize,
    line: u64,
    before: std::collections::VecDeque<String>,
    out: Vec<LineMatch>,
    owed: Vec<(usize, usize)>,
}

impl Scan<'_> {
    fn feed(&mut self, l: &[u8]) {
        self.line += 1;

        if !self.owed.is_empty() {
            let t = line_text(l);

            for (i, n) in &mut self.owed {
                self.out[*i].after.push(t.clone());
                *n -= 1;
            }

            self.owed.retain(|(_, n)| *n > 0);
        }

        if self.re.is_match(l) {
            self.out.push(LineMatch {
                line: self.line,
                text: line_text(l),
                before: self.before.iter().cloned().collect(),
                after: Vec::new(),
            });

            if self.ctx > 0 {
                self.owed.push((self.out.len() - 1, self.ctx));
            }
        }

        if self.ctx > 0 {
            if self.before.len() == self.ctx {
                self.before.pop_front();
            }

            self.before.push_back(line_text(l));
        }
    }
}

/// Matching lines of one file, or `None` for binary files and no match.
/// Reads at most [`SCAN_CHUNK`] bytes at a time plus one partial line.
fn content<F: Fs>(
    fs: &F,
    path: &FilePath,
    size: u64,
    re: &Regex,
    spec: &SearchSpec,
    st: &mut State,
) -> Option<Vec<LineMatch>> {
    if st.scanned_bytes.saturating_add(size) > SEARCH_MAX_SCANNED_BYTES {
        st.stop = Some(Status::Truncated);
        return None;
    }

    let f = fs.open_read(path.components()).ok()?;

    // Re-check the handle: the entry may have been swapped for a link.
    let m = f.meta().ok()?;
    if m.kind != Kind::File || m.size > spec.max_file_size {
        return None;
    }

    // Binary: NUL in the first 8 KiB, unless the file starts with a UTF-16 BOM.
    let head = read_range(&f, 0, m.size.min(8192)).ok()?;
    let utf16 = head.starts_with(&[0xff, 0xfe]) || head.starts_with(&[0xfe, 0xff]);
    if !utf16 && head.contains(&0) {
        st.scanned_bytes += head.len() as u64;
        return None;
    }

    let mut scan = Scan {
        re,
        ctx: usize::from(spec.context),
        line: 0,
        before: std::collections::VecDeque::new(),
        out: Vec::new(),
        owed: Vec::new(),
    };
    let mut carry: Vec<u8> = Vec::new();
    let mut off = 0u64;
    let full = |scan: &Scan<'_>| st.matches + scan.out.len() >= SEARCH_MAX_MATCHES;

    'read: while off < m.size {
        let chunk = read_range(&f, off, (m.size - off).min(SCAN_CHUNK)).ok()?;
        if chunk.is_empty() {
            break;
        }

        off += chunk.len() as u64;
        st.scanned_bytes += chunk.len() as u64;
        carry.extend_from_slice(&chunk);
        let mut start = 0usize;

        while let Some(pos) = carry[start..].iter().position(|&b| b == b'\n') {
            scan.feed(&carry[start..start + pos]);
            start += pos + 1;

            if full(&scan) {
                break 'read;
            }
        }

        carry.drain(..start);

        if carry.len() > MAX_SCAN_LINE {
            scan.feed(&carry);
            carry.clear();
        }
    }

    if !carry.is_empty() && !full(&scan) {
        scan.feed(&carry);
    }

    (!scan.out.is_empty()).then_some(scan.out)
}
