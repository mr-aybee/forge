//! `processes`: an htop-like snapshot of the node, on demand (`ProcessList`,
//! read-only, run by the agent), and process actions (`ProcessAction`,
//! D-080): end a process or a process tree, or change its priority.
//!
//! One long-lived sampler keeps the previous reading, so per-process CPU %
//! is the usage since the last call (htop semantics: 100 % = one core).
//! The first call after start reports 0 % until the next one.
//!
//! Actions run with the node's highest privilege: on Linux in the root
//! helper, which derives them from the signed command only (D-068); on
//! Windows in the agent itself (LocalSystem). Every action names the
//! process's start time, and the node refuses when the PID now belongs to
//! another process. Protected processes (init, kernel threads, Windows
//! system processes, Forge's own) are refused before anything is sent.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
#[cfg(unix)]
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind,
};

/// Most processes returned.
pub const MAX_LIMIT: u32 = 500;
/// Longest command line returned (characters).
const MAX_CMD: usize = 300;

struct Sampler {
    sys: System,
    /// uid (as text) → user name (Unix; Windows resolves SIDs itself).
    #[cfg(unix)]
    users: HashMap<String, String>,
    #[cfg(unix)]
    users_read: Option<Instant>,
}

fn sampler() -> &'static Mutex<Sampler> {
    static S: OnceLock<Mutex<Sampler>> = OnceLock::new();
    S.get_or_init(|| {
        keep_no_files_open();
        Mutex::new(Sampler {
            sys: System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                    .with_memory(MemoryRefreshKind::everything()),
            ),
            #[cfg(unix)]
            users: HashMap::new(),
            #[cfg(unix)]
            users_read: None,
        })
    })
}

/// Stops sysinfo from keeping `/proc/<pid>/stat` open for every process
/// between refreshes. By default it keeps up to half the process's file
/// limit open (over 1,000 handles on a busy host) to save one `open` per
/// process per refresh; the sampler refreshes once a minute, so reopening
/// costs a few milliseconds a minute. Linux only; elsewhere a no-op.
fn keep_no_files_open() {
    sysinfo::set_open_files_limit(0);
}

/// uid → name from `/etc/passwd` (re-read at most once a minute).
#[cfg(unix)]
fn user_names(s: &mut Sampler) -> HashMap<String, String> {
    if s.users_read.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
        s.users = std::fs::read_to_string("/etc/passwd")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut f = l.split(':');
                let name = f.next()?;
                let uid = f.nth(1)?.to_owned();
                Some((uid, name.to_owned()))
            })
            .collect();
        s.users_read = Some(Instant::now());
    }

    s.users.clone()
}

/// The owner's name: a uid looked up in `/etc/passwd`, or on Windows a SID
/// resolved to `DOMAIN\user` (cached; the SID itself when it cannot be).
fn owner(users: &HashMap<String, String>, uid: &sysinfo::Uid) -> String {
    #[cfg(windows)]
    {
        let _ = users;
        crate::processes_windows::account_name(uid)
    }
    #[cfg(not(windows))]
    {
        let id = uid.to_string();
        users.get(&id).cloned().unwrap_or(id)
    }
}

/// htop's state letter. Windows has no such states (sysinfo reports every
/// process as running or unknown), so it gets none.
fn state_letter(st: sysinfo::ProcessStatus) -> Value {
    use sysinfo::ProcessStatus as P;

    if cfg!(windows) {
        return Value::Null;
    }

    let l = match st {
        P::Run => "R",
        P::Sleep | P::Idle => "S",
        P::UninterruptibleDiskSleep => "D",
        P::Zombie => "Z",
        P::Stop | P::Tracing => "T",
        P::Dead => "X",
        _ => "?",
    };

    Value::from(l)
}

fn cmd_line(p: &sysinfo::Process) -> String {
    let parts: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let mut c = if parts.is_empty() { p.name().to_string_lossy().into_owned() } else { parts.join(" ") };
    if c.chars().count() > MAX_CMD {
        c = c.chars().take(MAX_CMD).collect::<String>() + "…";
    }

    c
}

/// Linux: the nice value from `/proc/<pid>/stat` (field 19).
#[cfg(target_os = "linux")]
fn nice_of(pid: u32) -> Option<i64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_nice(&stat)
}

/// Field 19 of `/proc/<pid>/stat`; the command name (field 2) may contain
/// spaces and parentheses, so fields are counted after the last `)`.
#[cfg(any(target_os = "linux", test))]
fn parse_nice(stat: &str) -> Option<i64> {
    let rest = &stat[stat.rfind(')')? + 1..];

    // `rest` starts at field 3 (state); field 19 is 16 further.
    rest.split_whitespace().nth(16)?.parse().ok()
}

/// Per-row priority: Linux `nice`. Windows reports none: reading a class
/// needs a process handle, and D-080 keeps the agent free of `unsafe`.
fn add_priority(row: &mut Value, pid: u32) {
    #[cfg(target_os = "linux")]
    {
        row["nice"] = nice_of(pid).map_or(Value::Null, Value::from);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (row, pid);
    }
}

/// What this node can do to a process, so the console offers only that.
#[must_use]
pub fn capabilities() -> Value {
    if cfg!(windows) {
        json!({
            "signals": ["term", "kill"],
            "tree": true,
            "nice": false,
            "priority_classes": PriorityClass::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        })
    } else {
        json!({ "signals": ["term", "kill", "hup", "int"], "tree": true, "nice": true, "priority_classes": [] })
    }
}

/// The `n` processes using the most CPU and the `n` using the most memory
/// (their union), for the per-minute node history. Threads are skipped.
#[must_use]
pub fn top(n: usize) -> Vec<forge_proto::messages::TopProcess> {
    let Ok(mut s) = sampler().lock() else { return Vec::new() };
    s.sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
    #[cfg(unix)]
    let users = user_names(&mut s);
    #[cfg(not(unix))]
    let users: HashMap<String, String> = HashMap::new();
    let all: Vec<(&sysinfo::Pid, &sysinfo::Process)> =
        s.sys.processes().iter().filter(|(_, p)| p.thread_kind().is_none()).collect();
    let mut by_cpu: Vec<usize> = (0..all.len()).collect();
    by_cpu.sort_by(|a, b| all[*b].1.cpu_usage().total_cmp(&all[*a].1.cpu_usage()));
    let mut by_mem: Vec<usize> = (0..all.len()).collect();
    by_mem.sort_by_key(|i| std::cmp::Reverse(all[*i].1.memory()));
    let mut picked: Vec<usize> = by_cpu.into_iter().take(n).collect();

    for i in by_mem.into_iter().take(n) {
        if !picked.contains(&i) {
            picked.push(i);
        }
    }

    picked
        .into_iter()
        .map(|i| {
            let (pid, p) = all[i];
            let mut cmd = cmd_line(p);
            if cmd.len() > 256 {
                let mut cut = 256;

                while !cmd.is_char_boundary(cut) {
                    cut -= 1;
                }

                cmd.truncate(cut);
            }

            forge_proto::messages::TopProcess {
                pid: pid.as_u32(),
                name: p.name().to_string_lossy().into_owned(),
                user: p.user_id().map(|u| owner(&users, u)).unwrap_or_default(),
                cpu_percent: p.cpu_usage(),
                mem_bytes: p.memory(),
                cmd,
            }
        })
        .collect()
}

/// Snapshot: a summary header (per-core CPU, memory, swap, load, tasks,
/// uptime) and up to `limit` processes sorted by `sort`
/// (`cpu` | `mem` | `pid` | `time` | `name`), optionally filtered by a
/// case-insensitive substring of the name, command line or user.
#[must_use]
pub fn snapshot(sort: &str, limit: u32, filter: &str) -> Value {
    let Ok(mut s) = sampler().lock() else { return json!({ "error": "sampler unavailable" }) };
    s.sys.refresh_cpu_usage();
    s.sys.refresh_memory();
    s.sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let mem_total = s.sys.total_memory();
    let cpus: Vec<f32> = s.sys.cpus().iter().map(sysinfo::Cpu::cpu_usage).collect();
    let cpu_total = s.sys.global_cpu_usage();
    let (mem_used, swap_total, swap_used) = (s.sys.used_memory(), s.sys.total_swap(), s.sys.used_swap());

    // Windows has no load average (sysinfo reports zeros): none, not 0.00.
    let load = if cfg!(windows) {
        Value::Null
    } else {
        let l = System::load_average();
        json!([l.one, l.five, l.fifteen])
    };

    #[cfg(unix)]
    let users = user_names(&mut s);
    #[cfg(not(unix))]
    let users: HashMap<String, String> = HashMap::new();
    let needle = filter.trim().to_lowercase();

    let mut running = 0u32;
    let mut rows: Vec<(f32, u64, u32, u64, String, Value)> = Vec::new();
    let all = s.sys.processes();

    for (pid, p) in all {
        // Threads appear as processes on Linux; htop hides them by default.
        if p.thread_kind().is_some() {
            continue;
        }

        if matches!(p.status(), sysinfo::ProcessStatus::Run) {
            running += 1;
        }

        // A uid on Unix, a SID on Windows.
        let user = p.user_id().map(|u| owner(&users, u)).unwrap_or_default();
        let name = p.name().to_string_lossy().into_owned();
        let cmd = cmd_line(p);

        if !needle.is_empty()
            && !name.to_lowercase().contains(&needle)
            && !cmd.to_lowercase().contains(&needle)
            && !user.to_lowercase().contains(&needle)
        {
            continue;
        }

        #[allow(clippy::cast_precision_loss)]
        let mem_pct = if mem_total > 0 { p.memory() as f64 * 100.0 / mem_total as f64 } else { 0.0 };
        let row = json!({
            "pid": pid.as_u32(),
            "ppid": p.parent().map(sysinfo::Pid::as_u32),
            "user": user,
            "state": state_letter(p.status()),
            "cpu": (p.cpu_usage() * 10.0).round() / 10.0,
            "mem": p.memory(),
            "virt": p.virtual_memory(),
            "mem_pct": (mem_pct * 10.0).round() / 10.0,
            "time": p.run_time(),
            "started": p.start_time(),
            "name": name,
            "cmd": cmd,
        });
        rows.push((p.cpu_usage(), p.memory(), pid.as_u32(), p.run_time(), name, row));
    }

    let tasks = rows.len();

    match sort {
        "mem" => rows.sort_by_key(|r| std::cmp::Reverse(r.1)),
        "pid" => rows.sort_by_key(|r| r.2),
        "time" => rows.sort_by_key(|r| std::cmp::Reverse(r.3)),
        "name" => rows.sort_by_key(|r| r.4.to_lowercase()),
        _ => rows.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1))),
    }

    let limit = usize::try_from(limit.clamp(1, MAX_LIMIT)).unwrap_or(100);

    // Priority only for the rows returned: it costs a file read per process.
    let processes: Vec<Value> = rows
        .into_iter()
        .take(limit)
        .map(|(_, _, pid, _, _, mut row)| {
            add_priority(&mut row, pid);
            row
        })
        .collect();

    json!({
        "summary": {
            "os": std::env::consts::OS,
            "cpus": cpus.iter().map(|c| (c * 10.0).round() / 10.0).collect::<Vec<_>>(),
            "cpu_total": (cpu_total * 10.0).round() / 10.0,
            "load": load,
            "mem_total": mem_total,
            "mem_used": mem_used,
            "swap_total": swap_total,
            "swap_used": swap_used,
            "tasks": all.len(),
            "running": running,
            "uptime": System::uptime(),
        },
        "actions": capabilities(),
        "matched": tasks,
        "processes": processes,
    })
}

// ------------------------------------------------------------------ actions

/// Signals the process view can send. Windows: `term` (End task, `taskkill`)
/// and `kill` (End process, `taskkill /F`) only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// SIGTERM: ask the process to exit. Windows: `taskkill` asks it to close.
    Term,
    /// SIGKILL. Windows: `taskkill /F` (`TerminateProcess`).
    Kill,
    /// SIGHUP: many daemons reload their configuration.
    Hup,
    /// SIGINT: like Ctrl+C.
    Int,
}

impl Signal {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "term" => Self::Term,
            "kill" => Self::Kill,
            "hup" => Self::Hup,
            "int" => Self::Int,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Term => "term",
            Self::Kill => "kill",
            Self::Hup => "hup",
            Self::Int => "int",
        }
    }
}

/// Windows priority classes Forge sets. Realtime is never offered: it can
/// starve the system's own threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
}

impl PriorityClass {
    pub const ALL: [Self; 5] = [Self::Idle, Self::BelowNormal, Self::Normal, Self::AboveNormal, Self::High];

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::BelowNormal => "below_normal",
            Self::Normal => "normal",
            Self::AboveNormal => "above_normal",
            Self::High => "high",
        }
    }

    /// The Win32 `*_PRIORITY_CLASS` value.
    #[must_use]
    pub const fn win32(self) -> u32 {
        match self {
            Self::Idle => 0x40,
            Self::BelowNormal => 0x4000,
            Self::Normal => 0x20,
            Self::AboveNormal => 0x8000,
            Self::High => 0x80,
        }
    }

    /// The class name of a Win32 value (`realtime` is reported, never set).
    #[must_use]
    pub fn name_of_win32(v: u32) -> Option<&'static str> {
        if v == 0x100 {
            return Some("realtime");
        }

        Self::ALL.into_iter().find(|c| c.win32() == v).map(Self::as_str)
    }
}

/// Lowest and highest nice value.
pub const NICE_RANGE: std::ops::RangeInclusive<i32> = -20..=19;

/// One process action, as the helper (Linux) or the agent (Windows) runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessOp {
    /// Send `signal` to the process, and with `tree` then to its
    /// descendants, parents before children (as listed before acting).
    Signal {
        pid: u32,
        start_time: u64,
        signal: Signal,
        #[serde(default)]
        tree: bool,
    },
    /// Linux: set the nice value.
    Nice { pid: u32, start_time: u64, nice: i32 },
    /// Windows: set the priority class.
    PriorityClass { pid: u32, start_time: u64, class: PriorityClass },
}

impl ProcessOp {
    /// From the signed command's fields; validated.
    pub fn from_wire(a: &forge_proto::messages::ProcessAction) -> Result<Self, String> {
        let op = match a.op.as_str() {
            "signal" => Self::Signal {
                pid: a.pid,
                start_time: a.start_time,
                signal: Signal::parse(&a.signal).ok_or_else(|| format!("unknown signal `{}`", a.signal))?,
                tree: a.tree,
            },
            "nice" => Self::Nice { pid: a.pid, start_time: a.start_time, nice: a.nice },
            "priority_class" => Self::PriorityClass {
                pid: a.pid,
                start_time: a.start_time,
                class: PriorityClass::parse(&a.class).ok_or_else(|| format!("unknown priority class `{}`", a.class))?,
            },
            other => return Err(format!("unknown process action `{other}`")),
        };

        op.validate()?;

        Ok(op)
    }

    #[must_use]
    pub const fn pid(&self) -> u32 {
        match self {
            Self::Signal { pid, .. } | Self::Nice { pid, .. } | Self::PriorityClass { pid, .. } => *pid,
        }
    }

    #[must_use]
    pub const fn start_time(&self) -> u64 {
        match self {
            Self::Signal { start_time, .. }
            | Self::Nice { start_time, .. }
            | Self::PriorityClass { start_time, .. } => *start_time,
        }
    }

    /// Checks the arguments and that this OS supports the action.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_for(cfg!(windows))
    }

    fn validate_for(&self, windows: bool) -> Result<(), String> {
        if self.pid() == 0 {
            return Err("PID 0 is not a process Forge can act on".into());
        }

        match self {
            Self::Signal { signal: s @ (Signal::Hup | Signal::Int), .. } if windows => {
                Err(format!("Windows has no `{}` signal; use `term` (End task) or `kill` (End process)", s.as_str()))
            }
            Self::Nice { nice, .. } if !NICE_RANGE.contains(nice) => Err(format!("nice must be -20 to 19, not {nice}")),
            Self::Nice { .. } if windows => Err("Windows uses priority classes, not nice values".into()),
            Self::PriorityClass { .. } if !windows => Err("priority classes are for Windows; use nice".into()),
            _ => Ok(()),
        }
    }
}

/// Windows processes that keep the system running; ending one stops or
/// restarts Windows. Names compared case-insensitively, without `.exe`.
const WINDOWS_PROTECTED: [&str; 11] = [
    "system",
    "registry",
    "secure system",
    "memory compression",
    "smss",
    "csrss",
    "wininit",
    "services",
    "lsass",
    "lsaiso",
    "winlogon",
];

/// Why Forge refuses to act on this process, if it does: the kernel's and
/// init's processes, kernel threads, Windows system processes, and Forge's
/// own components (the agent, its helper, a co-located control plane),
/// which are managed through their services instead.
#[must_use]
pub fn protected_reason(pid: u32, ppid: Option<u32>, name: &str, windows: bool) -> Option<String> {
    let lower = name.trim().to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);

    if pid == 0 {
        return Some("PID 0 belongs to the kernel".into());
    }

    if base == "forge" || base.starts_with("forge-") || base.starts_with("forge_") {
        return Some(format!("`{name}` is part of Orbyn Forge; manage it through its service or the node installer"));
    }

    if windows {
        if pid == 4 || WINDOWS_PROTECTED.contains(&base) {
            return Some(format!("`{name}` is a Windows system process; ending or changing it stops Windows"));
        }
    } else {
        if pid == 1 {
            return Some("PID 1 is the init system".into());
        }

        if pid == 2 || ppid == Some(2) {
            return Some(format!("`{name}` is a kernel thread"));
        }
    }

    None
}

/// The processes an action touches: the target first and, for a tree, its
/// descendants, parents before children. Each with its start time.
///
/// The target goes first: a parent that waits for its children (a shell,
/// a supervisor) would otherwise exit or restart them when they end. The
/// descendants are listed before anything is sent, so ones reparented to
/// init once their parent ends are still reached, by PID and start time.
fn targets(sys: &System, pid: u32, tree: bool) -> Vec<(u32, u64)> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();

    for (p, proc_) in sys.processes() {
        if proc_.thread_kind().is_some() {
            continue;
        }

        if let Some(parent) = proc_.parent() {
            children.entry(parent.as_u32()).or_default().push(p.as_u32());
        }
    }

    let mut order = vec![pid];

    if tree {
        // Breadth-first: each process before its children.
        let mut i = 0;

        while i < order.len() && order.len() < 10_000 {
            if let Some(kids) = children.get(&order[i]) {
                order.extend(kids.iter().copied().filter(|k| *k != pid));
            }

            i += 1;
        }
    }

    order.into_iter().filter_map(|p| sys.process(sysinfo::Pid::from_u32(p)).map(|x| (p, x.start_time()))).collect()
}

/// Runs a process action on this node. Refuses before acting when the PID
/// was reused or any affected process is protected.
pub fn run(op: &ProcessOp) -> Result<Value, String> {
    op.validate()?;
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pid = op.pid();
    let target = sys
        .process(sysinfo::Pid::from_u32(pid))
        .ok_or_else(|| format!("process {pid} is not running (refresh the list)"))?;

    if target.start_time() != op.start_time() {
        return Err(stale_message(pid));
    }

    let tree = matches!(op, ProcessOp::Signal { tree: true, .. });
    let list = targets(&sys, pid, tree);

    for (p, _) in &list {
        if let Some(x) = sys.process(sysinfo::Pid::from_u32(*p))
            && let Some(why) =
                protected_reason(*p, x.parent().map(sysinfo::Pid::as_u32), &x.name().to_string_lossy(), cfg!(windows))
        {
            return Err(if *p == pid {
                format!("refused: {why}")
            } else {
                format!("refused: process {p} in the tree is protected: {why}")
            });
        }
    }

    let (mut done, mut gone) = (Vec::new(), Vec::new());

    for (p, started) in list {
        let r = match op {
            ProcessOp::Signal { signal, .. } => platform::signal(p, started, *signal),
            ProcessOp::Nice { nice, .. } => platform::nice(p, started, *nice),
            ProcessOp::PriorityClass { class, .. } => platform::priority_class(p, started, *class),
        };

        match r {
            Ok(true) => done.push(p),
            // Exited meanwhile: nothing left to do. Only an error for the target.
            Ok(false) if p == pid => return Err(format!("process {pid} exited before Forge acted")),
            Ok(false) => gone.push(p),
            // A descendant that exited and whose PID was taken since the
            // listing is not part of the tree any more.
            Err(e) if p != pid && e == stale_message(p) => gone.push(p),
            Err(e) if p == pid || done.is_empty() => return Err(e),
            Err(e) => return Err(format!("{e} (already done for {done:?})")),
        }
    }

    Ok(json!({ "pid": pid, "action": op, "affected": done, "exited": gone }))
}

pub(crate) fn stale_message(pid: u32) -> String {
    format!("process {pid} is no longer the one listed: it exited and the PID was reused (refresh the list)")
}

/// Linux: pidfd-based signals (a reused PID can never receive them) and
/// `setpriority`, through rustix.
#[cfg(target_os = "linux")]
mod platform {
    use rustix::io::Errno;
    use rustix::process::{Pid, PidfdFlags, Signal as Sig};

    use super::Signal;

    fn start_time_now(pid: u32) -> Option<u64> {
        let mut sys = sysinfo::System::new();
        let p = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[p]),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );

        sys.process(p).map(sysinfo::Process::start_time)
    }

    fn rpid(pid: u32) -> Result<Pid, String> {
        i32::try_from(pid).ok().and_then(Pid::from_raw).ok_or_else(|| format!("invalid PID {pid}"))
    }

    /// `Ok(false)` when the process is gone.
    pub(super) fn signal(pid: u32, started: u64, signal: Signal) -> Result<bool, String> {
        let p = rpid(pid)?;

        let sig = match signal {
            Signal::Term => Sig::TERM,
            Signal::Kill => Sig::KILL,
            Signal::Hup => Sig::HUP,
            Signal::Int => Sig::INT,
        };

        // Pin the process first, then check it is the one listed: a PID
        // reused after this point cannot receive the signal.
        let fd = match rustix::process::pidfd_open(p, PidfdFlags::empty()) {
            Ok(fd) => Some(fd),
            Err(Errno::SRCH) => return Ok(false),
            Err(_) => None, // kernel before 5.3: plain kill(2) below
        };

        match start_time_now(pid) {
            None => return Ok(false),
            Some(t) if t != started => return Err(super::stale_message(pid)),
            Some(_) => {}
        }

        let r = match &fd {
            Some(fd) => rustix::process::pidfd_send_signal(fd, sig),
            None => rustix::process::kill_process(p, sig),
        };

        match r {
            Ok(()) => Ok(true),
            Err(Errno::SRCH) => Ok(false),
            Err(e) => Err(format!("could not signal process {pid}: {e}")),
        }
    }

    pub(super) fn nice(pid: u32, started: u64, nice: i32) -> Result<bool, String> {
        let p = rpid(pid)?;

        match start_time_now(pid) {
            None => return Ok(false),
            Some(t) if t != started => return Err(super::stale_message(pid)),
            Some(_) => {}
        }

        match rustix::process::setpriority_process(Some(p), nice) {
            Ok(()) => Ok(true),
            Err(Errno::SRCH) => Ok(false),
            Err(e) => Err(format!("could not change the priority of process {pid}: {e}")),
        }
    }

    pub(super) fn priority_class(_: u32, _: u64, _: super::PriorityClass) -> Result<bool, String> {
        Err("priority classes are for Windows; use nice".into())
    }
}

#[cfg(windows)]
mod platform {
    use super::Signal;

    pub(super) fn signal(pid: u32, started: u64, signal: Signal) -> Result<bool, String> {
        match signal {
            Signal::Term => crate::processes_windows::terminate(pid, started, false),
            Signal::Kill => crate::processes_windows::terminate(pid, started, true),
            other => Err(format!("Windows has no `{}` signal; use End task or End process", other.as_str())),
        }
    }

    pub(super) fn nice(_: u32, _: u64, _: i32) -> Result<bool, String> {
        Err("Windows uses priority classes, not nice values".into())
    }

    pub(super) fn priority_class(pid: u32, started: u64, class: super::PriorityClass) -> Result<bool, String> {
        crate::processes_windows::set_priority_class(pid, started, class)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    const NO: &str = "process actions need a Linux or Windows node";

    pub(super) fn signal(_: u32, _: u64, _: super::Signal) -> Result<bool, String> {
        Err(NO.into())
    }

    pub(super) fn nice(_: u32, _: u64, _: i32) -> Result<bool, String> {
        Err(NO.into())
    }

    pub(super) fn priority_class(_: u32, _: u64, _: super::PriorityClass) -> Result<bool, String> {
        Err(NO.into())
    }
}

#[cfg(test)]
mod tests {
    use forge_proto::messages::ProcessAction;

    use super::*;

    /// `/proc/<pid>/stat` handles this process holds.
    #[cfg(target_os = "linux")]
    fn open_stat_files() -> usize {
        std::fs::read_dir("/proc/self/fd")
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter_map(|e| std::fs::read_link(e.path()).ok())
                    .filter(|t| t.starts_with("/proc") && t.ends_with("stat"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// The sampler reads every process each minute but keeps no
    /// `/proc/<pid>/stat` open between reads (sysinfo's default kept one per
    /// process: over 1,000 open files on a busy host).
    #[cfg(target_os = "linux")]
    #[test]
    fn the_sampler_keeps_no_process_files_open() {
        let mut kids: Vec<_> =
            (0..20).filter_map(|_| std::process::Command::new("sleep").arg("30").spawn().ok()).collect();
        let rows = top(5);
        let _ = snapshot("cpu", 50, "");
        let rows_again = top(5);
        let open = open_stat_files();

        for k in &mut kids {
            let _ = k.kill();
            let _ = k.wait();
        }

        assert!(!rows.is_empty() && !rows_again.is_empty(), "processes were read");
        assert!(open <= 2, "{open} /proc/<pid>/stat files held open after sampling");
    }

    #[test]
    fn snapshot_lists_this_process_and_respects_limit_and_filter() {
        let v = snapshot("cpu", 5, "");

        // `FORGE_PRINT_SNAPSHOT=1 cargo test … -- --nocapture` shows a real listing.
        if std::env::var_os("FORGE_PRINT_SNAPSHOT").is_some() {
            eprintln!("{}", serde_json::to_string_pretty(&snapshot("mem", 12, "")).unwrap_or_default());
        }

        assert!(v["processes"].as_array().is_some_and(|p| !p.is_empty() && p.len() <= 5));
        assert!(v["summary"]["mem_total"].as_u64().is_some_and(|m| m > 0));
        assert_eq!(v["summary"]["os"], std::env::consts::OS);
        assert!(v["actions"]["signals"].as_array().is_some_and(|s| s.iter().any(|x| x == "kill")));

        if cfg!(windows) {
            assert!(v["summary"]["load"].is_null(), "no load average on Windows");
        } else {
            assert_eq!(v["summary"]["load"].as_array().map(Vec::len), Some(3));
        }

        let none = snapshot("pid", 50, "no-such-process-name-xyz");
        assert_eq!(none["processes"].as_array().map(Vec::len), Some(0));
        let me = std::process::id();
        let all = snapshot("pid", MAX_LIMIT, "");
        let rows = all["processes"].as_array().cloned().unwrap_or_default();
        let mine = rows.iter().find(|r| r["pid"] == me);
        assert!(mine.is_some() || all["matched"].as_u64() > Some(u64::from(MAX_LIMIT)));

        if let Some(r) = mine {
            assert!(r["started"].as_u64().is_some_and(|t| t > 0));
            assert!(!r["user"].as_str().unwrap_or_default().is_empty(), "owner resolved: {r}");

            if cfg!(windows) {
                assert!(r["state"].is_null());
            }

            if cfg!(target_os = "linux") {
                assert!(r["nice"].is_i64(), "{r}");
            }
        }
    }

    #[test]
    fn nice_is_field_19_even_with_odd_command_names() {
        let stat = "4242 (a) b (c)) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 5 1 0 12345 0 0";
        assert_eq!(parse_nice(stat), Some(5));
        assert_eq!(parse_nice("garbage"), None);
    }

    fn wire(op: &str) -> ProcessAction {
        ProcessAction { op: op.into(), pid: 4242, start_time: 1_700_000_000, ..Default::default() }
    }

    #[test]
    fn actions_are_parsed_and_checked_per_os() {
        let mut w = wire("signal");
        w.signal = "term".into();
        w.tree = true;
        assert_eq!(
            ProcessOp::from_wire(&w),
            Ok(ProcessOp::Signal { pid: 4242, start_time: 1_700_000_000, signal: Signal::Term, tree: true })
        );
        w.signal = "stop".into();
        assert!(ProcessOp::from_wire(&w).is_err(), "only the listed signals");
        assert!(ProcessOp::from_wire(&wire("exec")).is_err());
        let zero = ProcessOp::Signal { pid: 0, start_time: 1, signal: Signal::Kill, tree: false };
        assert!(zero.validate_for(false).is_err() && zero.validate_for(true).is_err());

        let nice = |n| ProcessOp::Nice { pid: 9, start_time: 1, nice: n };
        assert!(nice(-20).validate_for(false).is_ok() && nice(19).validate_for(false).is_ok());
        assert!(nice(-21).validate_for(false).is_err() && nice(20).validate_for(false).is_err());
        assert!(nice(0).validate_for(true).is_err(), "Windows has no nice");
        let class = ProcessOp::PriorityClass { pid: 9, start_time: 1, class: PriorityClass::High };
        assert!(class.validate_for(true).is_ok() && class.validate_for(false).is_err());
        let sig = |signal| ProcessOp::Signal { pid: 9, start_time: 1, signal, tree: false };
        assert!(sig(Signal::Term).validate_for(true).is_ok() && sig(Signal::Kill).validate_for(true).is_ok());
        assert!(sig(Signal::Hup).validate_for(true).is_err() && sig(Signal::Int).validate_for(true).is_err());
        assert!(sig(Signal::Hup).validate_for(false).is_ok());
        let mut c = wire("priority_class");
        c.class = "realtime".into();
        assert!(ProcessOp::from_wire(&c).is_err(), "realtime is never set");
    }

    #[test]
    fn priority_classes_map_to_win32_values() {
        let expect =
            [(0x40, "idle"), (0x4000, "below_normal"), (0x20, "normal"), (0x8000, "above_normal"), (0x80, "high")];

        for (v, name) in expect {
            let c = PriorityClass::parse(name);
            assert_eq!(c.map(PriorityClass::win32), Some(v), "{name}");
            assert_eq!(PriorityClass::name_of_win32(v), Some(name));
        }

        assert_eq!(PriorityClass::name_of_win32(0x100), Some("realtime"));
        assert_eq!(PriorityClass::parse("realtime"), None);
        assert_eq!(PriorityClass::name_of_win32(7), None);
    }

    #[test]
    fn protected_processes_are_refused() {
        // Every Windows critical process, by name (any case, with or without
        // `.exe`) and PID 4 whatever its name. Ending one stops Windows, so
        // these are never tried live on a workstation.
        for (pid, name) in [
            (4, "System"),
            (612, "lsass.exe"),
            (700, "CSRSS.EXE"),
            (701, "csrss"),
            (800, "winlogon.exe"),
            (90, "smss.exe"),
            (550, "wininit.exe"),
            (640, "services.exe"),
            (130, "Registry"),
            (2000, "Memory Compression"),
            (140, "Secure System"),
            (900, "LsaIso.exe"),
            (4, "renamed"),
        ] {
            assert!(protected_reason(pid, Some(1), name, true).is_some(), "{name}");
        }

        assert!(protected_reason(5000, Some(1), "notepad.exe", true).is_none());

        // Linux: init, kernel threads, and nothing else by name.
        assert!(protected_reason(1, None, "systemd", false).is_some());
        assert!(protected_reason(2, Some(0), "kthreadd", false).is_some());
        assert!(protected_reason(35, Some(2), "kworker/0:1", false).is_some());
        assert!(protected_reason(900, Some(1), "nginx", false).is_none());
        assert!(protected_reason(900, Some(1), "lsass", false).is_none(), "Windows names only on Windows");

        // Forge's own components, on both.
        for name in ["forge-agent", "forge-agent-hel", "forge-server", "forge-agent.exe", "Forge-Plugin-Runner"] {
            assert!(protected_reason(1234, Some(1), name, false).is_some(), "{name}");
            assert!(protected_reason(1234, Some(1), name, true).is_some(), "{name}");
        }

        assert!(protected_reason(1234, Some(1), "forgejo", false).is_none());
        assert!(protected_reason(0, None, "idle", false).is_some());
    }

    #[test]
    fn a_reused_pid_or_a_protected_process_is_refused_before_acting() {
        let me = std::process::id();
        let wrong = ProcessOp::Signal { pid: me, start_time: 1, signal: Signal::Kill, tree: false };
        assert!(run(&wrong).is_err_and(|e| e.contains("no longer the one listed")), "stale start time");
        let missing = ProcessOp::Signal { pid: u32::MAX - 7, start_time: 1, signal: Signal::Kill, tree: false };
        assert!(run(&missing).is_err_and(|e| e.contains("not running")));
    }

    #[test]
    fn a_tree_lists_the_parent_before_its_children() {
        // A child of this test process, so the tree has two levels.
        let child = if cfg!(windows) {
            std::process::Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
        } else {
            std::process::Command::new("sleep").arg("30").spawn()
        };

        let Ok(mut child) = child else { panic!("could not start a child process: {child:?}") };
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
        let me = std::process::id();
        let list: Vec<u32> = targets(&sys, me, true).into_iter().map(|t| t.0).collect();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(list.first(), Some(&me), "the target comes first: {list:?}");
        assert!(list.contains(&child.id()), "the child is in the tree: {list:?}");
        assert_eq!(targets(&sys, me, false).len(), 1);
    }
}
