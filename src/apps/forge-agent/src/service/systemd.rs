//! systemd over the system D-Bus (`org.freedesktop.systemd1`).

use async_trait::async_trait;
use zbus::OwnedObjectPath;

use super::{ServiceManager, ServiceState, valid_service_name};

const DEST: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";

#[derive(Debug)]
pub(super) struct Systemd;

fn unit_name(name: &str) -> Result<String, String> {
    if !valid_service_name(name) {
        return Err(format!("invalid unit name `{name}`"));
    }

    Ok(if name.contains('.') { name.to_owned() } else { format!("{name}.service") })
}

#[async_trait]
impl ServiceManager for Systemd {
    async fn status(&self, name: &str) -> Result<ServiceState, String> {
        let unit = unit_name(name)?;
        let conn = zbus::Connection::system().await.map_err(|e| e.to_string())?;
        let reply = match conn.call_method(Some(DEST), PATH, Some(MANAGER), "GetUnit", &(unit.as_str(),)).await {
            Ok(r) => r,
            Err(zbus::Error::MethodError(n, _, _)) if n.as_str().ends_with("NoSuchUnit") => {
                return Ok(ServiceState::NotFound);
            }
            Err(e) => return Err(e.to_string()),
        };

        let path: OwnedObjectPath = reply.body().deserialize().map_err(|e| e.to_string())?;
        let proxy = zbus::Proxy::new(&conn, DEST, path, UNIT).await.map_err(|e| e.to_string())?;
        let active: String = proxy.get_property("ActiveState").await.map_err(|e| e.to_string())?;

        Ok(match active.as_str() {
            "active" | "reloading" => ServiceState::Running,
            "inactive" => ServiceState::Stopped,
            "activating" => ServiceState::Starting,
            "deactivating" => ServiceState::Stopping,
            "failed" => ServiceState::Failed,
            _ => ServiceState::Unknown,
        })
    }

    async fn restart(&self, name: &str) -> Result<(), String> {
        let unit = unit_name(name)?;
        let conn = zbus::Connection::system().await.map_err(|e| e.to_string())?;

        conn.call_method(Some(DEST), PATH, Some(MANAGER), "RestartUnit", &(unit.as_str(), "replace"))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// A unit's `ActiveState` and its current `InvocationID` (hex), for health
/// checks (D-085). `None` when the unit doesn't exist. Read-only over D-Bus,
/// so the unprivileged agent can call it.
pub(super) async fn active_state(name: &str) -> Result<Option<(String, String)>, String> {
    let unit = unit_name(name)?;
    let conn = zbus::Connection::system().await.map_err(|e| e.to_string())?;
    let reply = match conn.call_method(Some(DEST), PATH, Some(MANAGER), "GetUnit", &(unit.as_str(),)).await {
        Ok(r) => r,
        Err(zbus::Error::MethodError(n, _, _)) if n.as_str().ends_with("NoSuchUnit") => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };

    let path: OwnedObjectPath = reply.body().deserialize().map_err(|e| e.to_string())?;
    let proxy = zbus::Proxy::new(&conn, DEST, path, UNIT).await.map_err(|e| e.to_string())?;
    let active: String = proxy.get_property("ActiveState").await.map_err(|e| e.to_string())?;
    let id: Vec<u8> = proxy.get_property("InvocationID").await.unwrap_or_default();
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();

    Ok(Some((active, hex)))
}
