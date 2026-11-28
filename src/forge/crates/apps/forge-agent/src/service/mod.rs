//! Native service control: systemd over D-Bus (Linux) and the Service
//! Control Manager (Windows). No shelling out.

use async_trait::async_trait;
use serde::Serialize;

#[cfg(target_os = "linux")]
mod systemd;
#[cfg(windows)]
mod windows_scm;

/// Normalised service state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Running,
    Stopped,
    Starting,
    Stopping,
    Failed,
    NotFound,
    Unknown,
}

#[async_trait]
pub trait ServiceManager: Send + Sync + std::fmt::Debug {
    async fn status(&self, name: &str) -> Result<ServiceState, String>;
    async fn restart(&self, name: &str) -> Result<(), String>;
}

/// Unit/service names: letters, digits and `@._:-`, at most 256 characters.
#[must_use]
pub fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | ':' | '-'))
}

/// A systemd unit's `ActiveState` (`active`, `failed`, …) and `InvocationID`
/// (hex), or `None` when the unit doesn't exist. Linux only.
pub async fn unit_active_state(name: &str) -> Result<Option<(String, String)>, String> {
    #[cfg(target_os = "linux")]
    {
        systemd::active_state(name).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
        Err("systemd units exist on Linux only".into())
    }
}

/// The service manager for this platform.
#[must_use]
pub fn platform() -> Box<dyn ServiceManager> {
    #[cfg(target_os = "linux")]
    {
        Box::new(systemd::Systemd)
    }
    #[cfg(windows)]
    {
        Box::new(windows_scm::Scm)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Box::new(Unsupported)
    }
}

#[derive(Debug)]
#[allow(dead_code)]
struct Unsupported;

#[async_trait]
impl ServiceManager for Unsupported {
    async fn status(&self, _name: &str) -> Result<ServiceState, String> {
        Err("service management is not supported on this platform".into())
    }
    async fn restart(&self, _name: &str) -> Result<(), String> {
        Err("service management is not supported on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_names_are_validated() {
        assert!(valid_service_name("nginx.service"));
        assert!(valid_service_name("getty@tty1.service"));
        assert!(valid_service_name("W3SVC"));
        assert!(!valid_service_name(""));
        assert!(!valid_service_name("--help"));
        assert!(!valid_service_name("a b"));
        assert!(!valid_service_name("x;rm -rf /"));
        assert!(!valid_service_name(&"a".repeat(300)));
    }
}
