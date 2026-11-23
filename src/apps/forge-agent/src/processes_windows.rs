//! Windows side of the process view and its actions (D-080), without
//! `unsafe` code: owners through sysinfo's account list plus the well-known
//! service SIDs, ending a process through the built-in `taskkill.exe /F`,
//! and priority classes through the built-in Windows PowerShell. Both tools
//! are started by absolute path with a fixed argument list; only integers
//! and names from a closed set go into them.
//!
//! "End task" is `taskkill` without `/F`, which asks the process to close
//! (`WM_CLOSE`). The agent runs as a service in session 0, so this reaches
//! only processes with a window there; for desktop apps and console
//! programs Windows answers that the process can only be ended forcefully,
//! and the console passes that on. "End process" is `taskkill /F`.

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::processes::PriorityClass;

/// `CREATE_NO_WINDOW`: no console window flashes on the node.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long the account list is reused.
const USERS_TTL: Duration = Duration::from_secs(60);

/// Names of the SIDs services run as; sysinfo lists only real accounts.
#[must_use]
pub fn well_known(sid: &str) -> Option<String> {
    let fixed = match sid {
        "S-1-5-18" => Some("NT AUTHORITY\\SYSTEM"),
        "S-1-5-19" => Some("NT AUTHORITY\\LOCAL SERVICE"),
        "S-1-5-20" => Some("NT AUTHORITY\\NETWORK SERVICE"),
        _ => None,
    };
    if let Some(f) = fixed {
        return Some(f.to_owned());
    }
    // Per-session system accounts: `S-1-5-90-0-<n>` and `S-1-5-96-0-<n>`.
    let session =
        |prefix: &str| sid.strip_prefix(prefix).filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if let Some(n) = session("S-1-5-90-0-") {
        return Some(format!("Window Manager\\DWM-{n}"));
    }
    session("S-1-5-96-0-").map(|n| format!("Font Driver Host\\UMFD-{n}"))
}

struct Accounts {
    users: sysinfo::Users,
    read: Option<Instant>,
    names: HashMap<String, String>,
}

fn accounts() -> &'static Mutex<Accounts> {
    static A: OnceLock<Mutex<Accounts>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(Accounts { users: sysinfo::Users::new(), read: None, names: HashMap::new() }))
}

/// The process owner's name: a well-known service account, a local account
/// from sysinfo, or the SID itself when neither knows it.
#[must_use]
pub fn account_name(uid: &sysinfo::Uid) -> String {
    let sid = uid.to_string();
    if let Some(n) = well_known(&sid) {
        return n;
    }
    let Ok(mut a) = accounts().lock() else { return sid };
    if a.read.is_none_or(|t| t.elapsed() > USERS_TTL) {
        a.users.refresh();
        a.read = Some(Instant::now());
        a.names.clear();
    }
    if let Some(n) = a.names.get(&sid) {
        return n.clone();
    }
    let name = a.users.get_user_by_id(uid).map_or_else(|| sid.clone(), |u| u.name().to_owned());
    a.names.insert(sid, name.clone());
    name
}

fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

/// The start time sysinfo reports for `pid` now, if it runs.
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

/// Ends the process: with `force` through `taskkill /F`
/// (`TerminateProcess`), otherwise by asking it to close. `Ok(false)` when
/// it is gone. The start time is checked again right before.
pub fn terminate(pid: u32, started: u64, force: bool) -> Result<bool, String> {
    match start_time_now(pid) {
        None => return Ok(false),
        Some(t) if t != started => return Err(super::processes::stale_message(pid)),
        Some(_) => {}
    }
    let mut args = vec!["/PID".to_owned(), pid.to_string()];
    if force {
        args.push("/F".to_owned());
    }
    let out = Command::new(system_root().join(r"System32\taskkill.exe"))
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run taskkill: {e}"))?;
    if out.status.success() {
        return Ok(true);
    }
    if start_time_now(pid).is_none() {
        return Ok(false);
    }
    Err(format!("could not end process {pid}: {}", tool_message(&out)))
}

/// The PowerShell name of a class.
#[must_use]
pub const fn ps_name(c: PriorityClass) -> &'static str {
    match c {
        PriorityClass::Idle => "Idle",
        PriorityClass::BelowNormal => "BelowNormal",
        PriorityClass::Normal => "Normal",
        PriorityClass::AboveNormal => "AboveNormal",
        PriorityClass::High => "High",
    }
}

/// The script that sets a class: it checks the start time on the same
/// process object it changes. Exit 3: another process has the PID now;
/// exit 4: no such process.
#[must_use]
pub fn priority_script(pid: u32, started: u64, class: PriorityClass) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'; \
         $p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; \
         if (-not $p) {{ exit 4 }}; \
         if ([DateTimeOffset]::new($p.StartTime).ToUnixTimeSeconds() -ne {started}) {{ exit 3 }}; \
         $p.PriorityClass = '{}'",
        ps_name(class)
    )
}

/// Sets the priority class. `Ok(false)` when the process is gone.
pub fn set_priority_class(pid: u32, started: u64, class: PriorityClass) -> Result<bool, String> {
    let out = Command::new(system_root().join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
        .env_remove("PSModulePath")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg(priority_script(pid, started, class))
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run PowerShell: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(4) => Ok(false),
        Some(3) => Err(super::processes::stale_message(pid)),
        _ => Err(format!("could not change the priority of process {pid}: {}", tool_message(&out))),
    }
}

/// The tool's message on one line: `taskkill` gives its reason on a second
/// line ("Reason: This process can only be terminated forcefully…").
fn tool_message(out: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr }).into_owned();
    one_line(&text).unwrap_or_else(|| format!("exit code {:?}", out.status.code()))
}

fn one_line(text: &str) -> Option<String> {
    let joined = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then(|| joined.chars().take(400).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taskkill_reasons_are_kept() {
        let out = "ERROR: The process with PID 32072 could not be terminated.\r\nReason: This process can only be terminated forcefully (with /F option).\r\n";
        assert_eq!(
            one_line(out).as_deref(),
            Some(
                "ERROR: The process with PID 32072 could not be terminated. Reason: This process can only be terminated forcefully (with /F option)."
            )
        );
        assert_eq!(one_line(" \r\n "), None);
    }

    #[test]
    fn well_known_service_sids_have_names_and_others_fall_back() {
        assert_eq!(well_known("S-1-5-18").as_deref(), Some("NT AUTHORITY\\SYSTEM"));
        assert_eq!(well_known("S-1-5-19").as_deref(), Some("NT AUTHORITY\\LOCAL SERVICE"));
        assert_eq!(well_known("S-1-5-20").as_deref(), Some("NT AUTHORITY\\NETWORK SERVICE"));
        assert_eq!(well_known("S-1-5-90-0-2").as_deref(), Some("Window Manager\\DWM-2"));
        assert_eq!(well_known("S-1-5-96-0-1").as_deref(), Some("Font Driver Host\\UMFD-1"));
        assert_eq!(well_known("S-1-5-90-0-x"), None);
        assert_eq!(well_known("S-1-5-21-1-2-3-1001"), None);
    }

    #[test]
    fn the_priority_script_carries_only_numbers_and_a_fixed_class_name() {
        let s = priority_script(4242, 1_700_000_000, PriorityClass::BelowNormal);
        assert!(s.contains("Get-Process -Id 4242 "));
        assert!(s.contains("-ne 1700000000)"));
        assert!(s.ends_with("$p.PriorityClass = 'BelowNormal'"));
        for c in PriorityClass::ALL {
            assert!(ps_name(c).bytes().all(|b| b.is_ascii_alphabetic()));
        }
    }

    #[test]
    fn reading_this_process_owner_gives_a_name() {
        let mut sys = sysinfo::System::new();
        let me = sysinfo::Pid::from_u32(std::process::id());
        sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[me]),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_user(sysinfo::UpdateKind::Always),
        );
        let uid = sys.process(me).and_then(sysinfo::Process::user_id).cloned();
        if let Some(uid) = uid {
            let n = account_name(&uid);
            assert!(!n.is_empty());
        }
    }

    #[test]
    fn a_stale_start_time_is_refused_without_running_anything() {
        let me = std::process::id();
        assert!(terminate(me, 1, true).is_err_and(|e| e.contains("no longer the one listed")));
        assert_eq!(terminate(u32::MAX - 7, 1, false), Ok(false));
    }
}
