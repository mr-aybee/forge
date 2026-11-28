//! Windows service host for the agent (D-079).
//!
//! `forge-agent service install` registers the `ForgeAgent` service
//! (automatic start, LocalSystem, restarted by the Service Control Manager
//! after a failure), `service uninstall` stops and deletes it, and the SCM
//! starts `forge-agent service run`, which runs the agent until the service
//! is stopped or Windows shuts down. Built on the `windows-service` crate,
//! so this module has no `unsafe` of its own.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceErrorControl,
    ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::StatePaths;

/// The service's name in the Service Control Manager.
pub const SERVICE_NAME: &str = "ForgeAgent";
const DISPLAY_NAME: &str = "Orbyn Forge agent";
const DESCRIPTION: &str = "Connects this machine to Orbyn Forge and carries out the actions Forge authorizes.";

/// The state directory the SCM-started process uses (set before the dispatcher starts).
static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Runs the agent until the receiver turns `true` (provided by the binary).
pub type Runner = fn(StatePaths, tokio::sync::watch::Receiver<bool>) -> anyhow::Result<()>;
static RUNNER: OnceLock<Runner> = OnceLock::new();

/// The command line the service runs: `<exe> service run [--state-dir <dir>]`.
#[must_use]
pub fn launch_arguments(state_dir: Option<&Path>) -> Vec<OsString> {
    let mut args = vec![OsString::from("service"), OsString::from("run")];

    if let Some(d) = state_dir {
        args.push(OsString::from("--state-dir"));
        args.push(d.as_os_str().to_owned());
    }

    args
}

fn info(exe: &Path, state_dir: Option<&Path>) -> ServiceInfo {
    ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments: launch_arguments(state_dir),
        dependencies: Vec::new(),
        // LocalSystem (D-079): the agent manages services, files and accounts.
        account_name: None,
        account_password: None,
    }
}

/// Restart after 5 s, 10 s, then every 60 s; the count resets after a day.
fn failure_actions() -> ServiceFailureActions {
    ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(5) },
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(10) },
            ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(60) },
        ]),
    }
}

/// Refuses to register a binary outside `%ProgramFiles%`: the service runs it
/// as LocalSystem, so a copy in a folder other accounts can write (Downloads,
/// `%TEMP%`, a share) would let any program that account runs replace it and
/// take over the machine. Only Administrators can write under Program Files.
pub fn require_protected_location(exe: &Path) -> anyhow::Result<()> {
    let program_files =
        std::env::var_os("ProgramFiles").map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
    let exe_real = std::fs::canonicalize(exe)?;
    let base_real = std::fs::canonicalize(&program_files)?;
    if is_inside(&exe_real, &base_real) {
        return Ok(());
    }

    anyhow::bail!(
        "{} is not under {}: the service runs it as LocalSystem, and a folder other accounts can write would let \
         them replace it. Copy it to {}\\OrbynForge, or use install.ps1",
        exe.display(),
        program_files.display(),
        program_files.display()
    )
}

/// Whether `path` lies inside `base` (both canonical), compared without
/// regard to case like Windows paths.
fn is_inside(path: &Path, base: &Path) -> bool {
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    let (p, b) = (lower(path), lower(base));
    let b = b.trim_end_matches('\\');

    p.len() > b.len() && p.starts_with(b) && p.as_bytes().get(b.len()) == Some(&b'\\')
}

/// Creates (or updates) and starts the service. Needs Administrator.
pub fn install(exe: &Path, state_dir: Option<&Path>) -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;
    let access =
        ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP | ServiceAccess::CHANGE_CONFIG;
    let info = info(exe, state_dir);

    let service = match manager.open_service(SERVICE_NAME, access) {
        Ok(s) => {
            // Re-install (upgrade): point the existing service at this binary.
            s.change_config(&info)?;
            s
        }
        Err(_) => manager.create_service(&info, access)?,
    };

    service.set_description(DESCRIPTION)?;
    service.update_failure_actions(failure_actions())?;
    service.set_failure_actions_on_non_crash_failures(true)?;

    if service.query_status()?.current_state != ServiceState::Running {
        service.start::<&str>(&[])?;
    }

    Ok(())
}

/// Stops (waiting up to 30 s) and deletes the service. Needs Administrator.
pub fn uninstall() -> anyhow::Result<bool> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;

    let Ok(service) =
        manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
    else {
        return Ok(false);
    };

    if service.query_status()?.current_state != ServiceState::Stopped {
        let _ = service.stop();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);

        while service.query_status()?.current_state != ServiceState::Stopped && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    service.delete()?;

    Ok(true)
}

define_windows_service!(ffi_service_main, service_main);

/// Entry point of `forge-agent service run` (only the SCM starts it).
pub fn run(state_dir: PathBuf, runner: Runner) -> anyhow::Result<()> {
    let _ = STATE_DIR.set(state_dir);
    let _ = RUNNER.set(runner);
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;

    Ok(())
}

fn status(state: ServiceState, exit: u32, wait_hint: Duration) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(exit),
        checkpoint: 0,
        wait_hint,
        process_id: None,
    }
}

/// How long the agent gets to finish after a stop request before the service
/// reports Stopped anyway (the process then exits, ending any work in flight).
const STOP_GRACE: Duration = Duration::from_secs(15);

fn service_main(_arguments: Vec<OsString>) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

    // The handler reports StopPending itself, at once: callers such as
    // PowerShell's Stop-Service give up after about 2 s if the service still
    // reads Running.
    let handle_cell: Arc<OnceLock<ServiceStatusHandle>> = Arc::new(OnceLock::new());
    let handler_cell = Arc::clone(&handle_cell);

    let handler = move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            if let Some(h) = handler_cell.get() {
                let _ = h.set_service_status(status(ServiceState::StopPending, 0, STOP_GRACE + Duration::from_secs(5)));
            }

            let _ = tx.send(true);
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };

    let Ok(handle) = service_control_handler::register(SERVICE_NAME, handler) else {
        tracing::error!("could not register the service control handler");
        return;
    };

    let _ = handle_cell.set(handle);
    let _ = handle.set_service_status(status(ServiceState::Running, 0, Duration::ZERO));
    let paths = StatePaths::new(STATE_DIR.get().cloned().unwrap_or_else(StatePaths::default_dir));

    // The agent runs on its own thread so a stop is bounded by STOP_GRACE
    // even if the agent is in the middle of slow work.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let runner = RUNNER.get().copied();
    let spawned = std::thread::Builder::new().name("forge-agent".into()).spawn(move || {
        let result = runner.map_or_else(|| Err(anyhow::anyhow!("no agent runner")), |run| run(paths, rx));
        let _ = done_tx.send(result);
    });

    let result = match spawned {
        Ok(_) => wait_for_agent(&done_rx, &stop_rx, STOP_GRACE),
        Err(e) => Err(anyhow::anyhow!("could not start the agent thread: {e}")),
    };

    let exit = match result {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!(error = %e, "agent stopped with an error");

            // ERROR_SERVICE_SPECIFIC_ERROR: the SCM's failure actions restart it.
            1066
        }
    };

    let _ = handle.set_service_status(status(ServiceState::Stopped, exit, Duration::ZERO));
}

/// Waits for the agent thread; after a stop request, at most `grace`.
fn wait_for_agent(
    done: &std::sync::mpsc::Receiver<anyhow::Result<()>>,
    stop: &std::sync::mpsc::Receiver<()>,
    grace: Duration,
) -> anyhow::Result<()> {
    let mut stop_at: Option<std::time::Instant> = None;

    loop {
        match done.recv_timeout(Duration::from_millis(250)) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("the agent thread ended without a result"));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }

        if stop_at.is_none() && stop.try_recv().is_ok() {
            stop_at = Some(std::time::Instant::now());
        }

        if stop_at.is_some_and(|t| t.elapsed() >= grace) {
            tracing::warn!(?grace, "agent did not finish in time; stopping the service anyway");
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_runs_the_service_subcommand() {
        assert_eq!(launch_arguments(None), vec![OsString::from("service"), OsString::from("run")]);
        let a = launch_arguments(Some(Path::new(r"D:\forge state")));
        assert_eq!(a.len(), 4);
        assert_eq!(a[2], OsString::from("--state-dir"));
        assert_eq!(a[3], OsString::from(r"D:\forge state"));
        let i = info(Path::new(r"C:\Program Files\OrbynForge\forge-agent.exe"), None);
        assert_eq!(i.name, OsString::from(SERVICE_NAME));
        assert!(i.account_name.is_none(), "LocalSystem");
        assert_eq!(failure_actions().actions.map(|a| a.len()), Some(3));
    }

    #[test]
    fn only_a_binary_under_program_files_becomes_the_service() {
        let base = Path::new(r"\\?\C:\Program Files");
        assert!(is_inside(Path::new(r"\\?\C:\Program Files\OrbynForge\forge-agent.exe"), base));
        assert!(is_inside(Path::new(r"\\?\C:\PROGRAM FILES\OrbynForge\forge-agent.exe"), base), "case-insensitive");
        assert!(!is_inside(Path::new(r"\\?\C:\Program Files Evil\forge-agent.exe"), base), "a sibling, not inside");
        assert!(!is_inside(Path::new(r"\\?\C:\Users\bob\Downloads\forge-agent.exe"), base));
        assert!(!is_inside(base, base));

        // The test binary itself lives in target\, outside Program Files.
        let me = std::env::current_exe().unwrap_or_else(|e| panic!("{e}"));
        let err = require_protected_location(&me).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("LocalSystem"), "{err}");
    }

    #[test]
    fn a_stop_is_bounded_even_if_the_agent_hangs() {
        let (_done_tx, done_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        assert!(stop_tx.send(()).is_ok());
        let started = std::time::Instant::now();
        assert!(wait_for_agent(&done_rx, &stop_rx, Duration::from_millis(300)).is_ok());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_agent_result_is_passed_through() {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (_stop_tx, stop_rx) = std::sync::mpsc::channel();
        assert!(done_tx.send(Err(anyhow::anyhow!("boom"))).is_ok());
        assert!(wait_for_agent(&done_rx, &stop_rx, STOP_GRACE).is_err());
    }

    #[test]
    fn stop_pending_accepts_no_further_controls() {
        let s = status(ServiceState::StopPending, 0, STOP_GRACE);
        assert!(s.controls_accepted.is_empty());
        assert_eq!(
            status(ServiceState::Running, 0, Duration::ZERO).controls_accepted,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        );
    }
}
