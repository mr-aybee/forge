//! Request bodies that reach the node itself.
//!
//! Creating a container is `container.manage`, but some settings hand the
//! container the host: privileged mode, added capabilities, host bind
//! mounts, devices, the host's PID, IPC, UTS, user or network environment, a
//! disabled security profile. A request with any of them needs the `Admin`
//! class (`server.command`, MFA). The same holds for a privileged exec and
//! for a `local` volume that binds a host path.
//!
//! The guard lists the reasons so the refusal and the audit entry can name
//! them.

use serde_json::Value;

use crate::catalog::Class;

/// Host-reaching settings in a request body (empty when there are none).
#[must_use]
pub fn host_access(op: &str, body: &Value) -> Vec<&'static str> {
    match op {
        "container.create" => container_create(body),
        "exec.create" => {
            if body.get("Privileged").and_then(Value::as_bool) == Some(true) {
                vec!["Privileged"]
            } else {
                Vec::new()
            }
        }
        "volume.create" => volume_create(body),
        _ => Vec::new(),
    }
}

/// The class a request needs: the endpoint's own, raised to `Admin` when
/// the body reaches the host.
#[must_use]
pub fn effective_class(class: Class, reasons: &[&str]) -> Class {
    if reasons.is_empty() { class } else { Class::Admin }
}

fn non_empty(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        _ => false,
    }
}

fn is_host(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str) == Some("host")
}

fn container_create(body: &Value) -> Vec<&'static str> {
    let mut out = Vec::new();
    let Some(h) = body.get("HostConfig") else {
        return out;
    };
    if h.get("Privileged").and_then(Value::as_bool) == Some(true) {
        out.push("HostConfig.Privileged");
    }
    if non_empty(h.get("CapAdd")) {
        out.push("HostConfig.CapAdd");
    }
    if non_empty(h.get("Binds")) {
        out.push("HostConfig.Binds");
    }
    let host_mount = h.get("Mounts").and_then(Value::as_array).is_some_and(|m| {
        m.iter().any(|m| {
            let t = m.get("Type").and_then(Value::as_str).unwrap_or("volume");
            t == "bind" || t == "npipe" || t == "cluster"
        })
    });
    if host_mount {
        out.push("HostConfig.Mounts (bind)");
    }
    if non_empty(h.get("Devices")) {
        out.push("HostConfig.Devices");
    }
    if non_empty(h.get("DeviceCgroupRules")) {
        out.push("HostConfig.DeviceCgroupRules");
    }
    for (key, label) in [
        ("PidMode", "HostConfig.PidMode=host"),
        ("IpcMode", "HostConfig.IpcMode=host"),
        ("UTSMode", "HostConfig.UTSMode=host"),
        ("UsernsMode", "HostConfig.UsernsMode=host"),
        ("CgroupnsMode", "HostConfig.CgroupnsMode=host"),
        ("NetworkMode", "HostConfig.NetworkMode=host"),
    ] {
        if is_host(h.get(key)) {
            out.push(label);
        }
    }
    let weak_profile = h.get("SecurityOpt").and_then(Value::as_array).is_some_and(|a| {
        a.iter().filter_map(Value::as_str).any(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("unconfined")
                || s.contains("label=disable")
                || s.contains("label:disable")
                || s.starts_with("seccomp")
        })
    });
    if weak_profile {
        out.push("HostConfig.SecurityOpt");
    }
    out
}

fn volume_create(body: &Value) -> Vec<&'static str> {
    let opts = body.get("DriverOpts").and_then(Value::as_object);
    let binds = opts.is_some_and(|o| {
        o.get("device").is_some()
            || o.get("o").and_then(Value::as_str).is_some_and(|s| s.split(',').any(|p| p.trim() == "bind"))
    });
    if binds { vec!["DriverOpts.device (host path)"] } else { Vec::new() }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_plain_container_stays_manage() {
        let body = json!({ "Image": "nginx", "HostConfig": { "PortBindings": {}, "Mounts": [{ "Type": "volume", "Source": "data", "Target": "/d" }] } });
        assert!(host_access("container.create", &body).is_empty());
        assert_eq!(effective_class(Class::Manage, &[]), Class::Manage);
    }

    #[test]
    fn host_reaching_settings_are_named() {
        let body = json!({ "Image": "x", "HostConfig": {
            "Privileged": true, "CapAdd": ["SYS_ADMIN"], "Binds": ["/:/host"], "PidMode": "host",
            "NetworkMode": "host", "SecurityOpt": ["apparmor=unconfined"], "Devices": [{ "PathOnHost": "/dev/sda" }],
            "Mounts": [{ "Type": "bind", "Source": "/etc", "Target": "/e" }]
        }});
        let r = host_access("container.create", &body);
        for want in [
            "HostConfig.Privileged",
            "HostConfig.CapAdd",
            "HostConfig.Binds",
            "HostConfig.PidMode=host",
            "HostConfig.NetworkMode=host",
            "HostConfig.SecurityOpt",
            "HostConfig.Devices",
            "HostConfig.Mounts (bind)",
        ] {
            assert!(r.contains(&want), "{want}: {r:?}");
        }
        assert_eq!(effective_class(Class::Manage, &r), Class::Admin);
    }

    #[test]
    fn privileged_exec_and_bind_volumes_are_admin() {
        assert_eq!(host_access("exec.create", &json!({ "Privileged": true, "Cmd": ["sh"] })), vec!["Privileged"]);
        assert!(host_access("exec.create", &json!({ "Cmd": ["sh"] })).is_empty());
        let bind =
            json!({ "Name": "v", "Driver": "local", "DriverOpts": { "type": "none", "o": "bind", "device": "/etc" } });
        assert!(!host_access("volume.create", &bind).is_empty());
        assert!(host_access("volume.create", &json!({ "Name": "v" })).is_empty());
    }
}
