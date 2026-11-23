//! Windows Service Control Manager via `windows-sys`.
#![allow(unsafe_code)]

use async_trait::async_trait;
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_HANDLE,
    SC_MANAGER_CONNECT, SERVICE_CONTINUE_PENDING, SERVICE_CONTROL_STOP, SERVICE_PAUSED, SERVICE_QUERY_STATUS,
    SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOP_PENDING,
    SERVICE_STOPPED, StartServiceW,
};

use super::{ServiceManager, ServiceState, valid_service_name};

#[derive(Debug)]
pub(super) struct Scm;

/// Closes an SCM handle on drop.
struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle was returned by OpenSCManagerW/OpenServiceW and is closed once.
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open(name: &str, access: u32) -> Result<Option<(Handle, Handle)>, String> {
    if !valid_service_name(name) {
        return Err(format!("invalid service name `{name}`"));
    }
    // SAFETY: null machine/database names select the local SCM; the result is checked.
    let scm = Handle(unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) });
    if scm.0.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let wname = wide(name);
    // SAFETY: `wname` is a NUL-terminated UTF-16 buffer that outlives the call.
    let svc = Handle(unsafe { OpenServiceW(scm.0, wname.as_ptr(), access) });
    if svc.0.is_null() {
        let err = std::io::Error::last_os_error();
        // ERROR_SERVICE_DOES_NOT_EXIST = 1060
        return if err.raw_os_error() == Some(1060) { Ok(None) } else { Err(err.to_string()) };
    }
    Ok(Some((scm, svc)))
}

fn query(svc: &Handle) -> Result<u32, String> {
    // SAFETY: SERVICE_STATUS is plain data; zeroed is a valid initial value.
    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `svc` is a valid service handle with SERVICE_QUERY_STATUS access.
    if unsafe { QueryServiceStatus(svc.0, &mut status) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(status.dwCurrentState)
}

fn map_state(s: u32) -> ServiceState {
    match s {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED | SERVICE_PAUSED => ServiceState::Stopped,
        SERVICE_START_PENDING | SERVICE_CONTINUE_PENDING => ServiceState::Starting,
        SERVICE_STOP_PENDING => ServiceState::Stopping,
        _ => ServiceState::Unknown,
    }
}

#[async_trait]
impl ServiceManager for Scm {
    async fn status(&self, name: &str) -> Result<ServiceState, String> {
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || match open(&name, SERVICE_QUERY_STATUS)? {
            None => Ok(ServiceState::NotFound),
            Some((_scm, svc)) => query(&svc).map(map_state),
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn restart(&self, name: &str) -> Result<(), String> {
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            let (_scm, svc) =
                open(&name, SERVICE_QUERY_STATUS | SERVICE_STOP | SERVICE_START)?.ok_or("service not found")?;
            if query(&svc)? != SERVICE_STOPPED {
                // SAFETY: SERVICE_STATUS is plain data; zeroed is valid.
                let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
                // SAFETY: valid handle with SERVICE_STOP access.
                unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) };
                for _ in 0..60 {
                    if query(&svc)? == SERVICE_STOPPED {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
            // SAFETY: valid handle with SERVICE_START access; no arguments.
            if unsafe { StartServiceW(svc.0, 0, std::ptr::null()) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
