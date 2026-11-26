//! Request bodies of `forge services create | update | place`, built from flags.
//!
//! A definition is the REST service body (`AppInput`). `create` and
//! `update` start from `--file` when it is given, else (update) from the
//! stored definition, and the flags change single fields of it. Secret
//! values come from stdin or a file, never from the command line.

use std::path::PathBuf;

use clap::{Args, ValueEnum};
use serde_json::{Map, Value, json};

use crate::error::{CliError, Result};

/// What a service runs as.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// A native system service (systemd unit or Windows service) from a command.
    Systemd,
    /// A container from an image.
    Container,
}

impl Runtime {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Container => "container",
        }
    }
}

/// The definition fields `create` and `update` take as flags.
#[derive(Args, Debug, Default, Clone)]
pub struct AppFlags {
    /// What runs: systemd (native, from --command) or container (from --image).
    #[arg(long, value_enum)]
    pub runtime: Option<Runtime>,
    /// Container image, such as `nginx:1.27` (containers only).
    #[arg(long)]
    pub image: Option<String>,
    /// The command to run (a container: instead of the image's command).
    #[arg(long)]
    pub command: Option<String>,
    /// Container port mapping `[IP:]HOST:CONTAINER[/tcp|udp]`; repeat for
    /// more. Given at all, the flags replace the whole list.
    #[arg(long = "port", value_name = "PORT")]
    pub ports: Vec<String>,
    /// A variable `KEY=VALUE`; repeat for more. Sets or replaces that
    /// variable and keeps the others.
    #[arg(long = "env", value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// A secret variable. `KEY` reads its value from stdin (several: one
    /// `KEY=VALUE` line each); `KEY=@FILE` reads it from a file. A value on
    /// the command line is refused.
    #[arg(long = "secret-env", value_name = "KEY[=@FILE]")]
    pub secret_env: Vec<String>,
    /// TCP port a native service listens on (its default health check).
    #[arg(long)]
    pub listen_port: Option<u16>,
    /// Container volume `HOST_PATH:CONTAINER_PATH[:ro]`; repeat for more.
    /// Given at all, the flags replace the whole list.
    #[arg(long = "volume", value_name = "VOLUME")]
    pub volumes: Vec<String>,
    /// Restart it when it stops: true or false (the server's default is true).
    #[arg(long, value_name = "BOOL")]
    pub restart: Option<bool>,
    /// Restart policy: no, always, unless-stopped or on-failure.
    #[arg(long, value_enum, value_name = "POLICY")]
    pub restart_policy: Option<RestartPolicy>,
    /// With `--restart-policy on-failure`: give up after N restarts (0 = never).
    #[arg(long, value_name = "N")]
    pub restart_max_attempts: Option<u32>,
    /// Kernel capability to add, such as NET_ADMIN (deploying needs
    /// stack.privileged); repeat for more. Given at all, replaces the list.
    #[arg(long = "cap-add", value_name = "CAP")]
    pub cap_add: Vec<String>,
    /// Kernel capability to drop (`ALL` for every one); repeat for more.
    #[arg(long = "cap-drop", value_name = "CAP")]
    pub cap_drop: Vec<String>,
    /// DNS server address for the container; repeat for more.
    #[arg(long = "dns", value_name = "IP")]
    pub dns: Vec<String>,
    /// tmpfs mount `PATH[:SIZE_MB]`; repeat for more.
    #[arg(long = "tmpfs", value_name = "PATH[:SIZE_MB]")]
    pub tmpfs: Vec<String>,
    /// Run Docker's init as PID 1: true or false.
    #[arg(long, value_name = "BOOL")]
    pub init: Option<bool>,
    /// Existing Docker network to connect to as well; repeat for more.
    #[arg(long = "network", value_name = "NAME")]
    pub networks: Vec<String>,
}

/// A restart policy (`--restart-policy`).
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    No,
    Always,
    UnlessStopped,
    OnFailure,
}

impl RestartPolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::Always => "always",
            Self::UnlessStopped => "unless-stopped",
            Self::OnFailure => "on-failure",
        }
    }
}

/// `PATH[:SIZE_MB]` of `--tmpfs`.
fn parse_tmpfs(spec: &str) -> Result<Value> {
    let (target, size) = match spec.split_once(':') {
        Some((t, s)) => {
            let mb: u32 =
                s.parse().map_err(|_| CliError::usage(format!("--tmpfs {spec}: SIZE_MB must be a number")))?;
            (t, mb)
        }
        None => (spec, 0),
    };

    Ok(json!({ "target": target, "size_mb": size }))
}

impl AppFlags {
    /// Whether any definition flag was given.
    pub fn any(&self) -> bool {
        self.runtime.is_some()
            || self.image.is_some()
            || self.command.is_some()
            || !self.ports.is_empty()
            || !self.env.is_empty()
            || !self.secret_env.is_empty()
            || self.listen_port.is_some()
            || !self.volumes.is_empty()
            || self.restart.is_some()
            || self.restart_policy.is_some()
            || self.restart_max_attempts.is_some()
            || !self.cap_add.is_empty()
            || !self.cap_drop.is_empty()
            || !self.dns.is_empty()
            || !self.tmpfs.is_empty()
            || self.init.is_some()
            || !self.networks.is_empty()
    }
}

/// Where a secret variable's value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    Stdin,
    File(PathBuf),
}

fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && !k.starts_with(|c: char| c.is_ascii_digit())
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `KEY=VALUE` pairs of `--env` (the value may be empty and may hold `=`).
pub fn parse_env(pairs: &[String]) -> Result<Vec<(String, String)>> {
    pairs
        .iter()
        .map(|p| match p.split_once('=') {
            Some((k, v)) if valid_key(k) => Ok((k.to_owned(), v.to_owned())),
            _ => Err(CliError::usage(format!("--env {p}: expected KEY=VALUE (letters, digits and `_` in KEY)"))),
        })
        .collect()
}

/// `KEY` or `KEY=@FILE` of `--secret-env`. `KEY=value` is refused: a value
/// on the command line ends up in the shell history and the process list.
pub fn parse_secret_env(specs: &[String]) -> Result<Vec<(String, SecretSource)>> {
    specs
        .iter()
        .map(|s| {
            let (key, source) = match s.split_once('=') {
                None => (s.as_str(), SecretSource::Stdin),
                Some((k, v)) => match v.strip_prefix('@').filter(|p| !p.is_empty() && *p != "-") {
                    Some(path) => (k, SecretSource::File(PathBuf::from(path))),
                    None if v == "@-" => (k, SecretSource::Stdin),
                    None => {
                        return Err(CliError::usage(format!(
                            "--secret-env {k}: a secret is never taken from the command line; pass `{k}` and pipe the value on stdin, or `{k}=@<file>`"
                        ))
                        .with_code("secret_on_command_line"));
                    }
                },
            };

            if valid_key(key) {
                Ok((key.to_owned(), source))
            } else {
                Err(CliError::usage(format!("--secret-env {s}: `{key}` is not a variable name")))
            }
        })
        .collect()
}

fn trim_newline(s: &str) -> &str {
    s.strip_suffix('\n').map_or(s, |t| t.strip_suffix('\r').unwrap_or(t))
}

/// Resolves every secret's value. `stdin` is read at most once: one key
/// read from stdin takes all of it; several take one `KEY=VALUE` line each.
pub fn read_secrets(
    specs: &[(String, SecretSource)],
    stdin: impl FnOnce() -> Result<String>,
) -> Result<Vec<(String, String)>> {
    let from_stdin: Vec<&str> =
        specs.iter().filter(|(_, s)| *s == SecretSource::Stdin).map(|(k, _)| k.as_str()).collect();
    let piped = if from_stdin.is_empty() { String::new() } else { stdin()? };
    let mut out = Vec::new();

    for (key, source) in specs {
        let value = match source {
            SecretSource::File(path) => {
                let text =
                    std::fs::read_to_string(path).map_err(|e| CliError::usage(format!("{}: {e}", path.display())))?;
                trim_newline(&text).to_owned()
            }
            SecretSource::Stdin if from_stdin.len() == 1 => trim_newline(&piped).to_owned(),
            SecretSource::Stdin => piped
                .lines()
                .find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.to_owned()))
                .ok_or_else(|| CliError::usage(format!("stdin has no `{key}=<value>` line for --secret-env {key}")))?,
        };

        if value.is_empty() {
            return Err(CliError::usage(format!("the value of secret `{key}` is empty")));
        }

        if value.contains(['\n', '\r']) {
            return Err(CliError::usage(format!("the value of secret `{key}` may not contain line breaks")));
        }

        out.push((key.clone(), value));
    }

    Ok(out)
}

/// Spec fields of a stored service that are not part of a definition:
/// the server fills `environment` itself, and the health policy has its own
/// endpoint (an update keeps it).
const NOT_DEFINITION: &[&str] = &["environment", "health", "name", "runtime", "env", "stack"];

/// The definition (`AppInput`) of a stored service, as `GET
/// /services/{id}` returns it. Secret variables keep their stored value:
/// they are sent without one.
pub fn definition(app: &Value) -> Value {
    let mut d = Map::new();
    d.insert("runtime".into(), app["runtime"].clone());

    // Sent back on update: the server refuses it (409) if someone saved in
    // between, instead of overwriting their change.
    if app["version"].is_i64() {
        d.insert("version".into(), app["version"].clone());
    }

    for (k, v) in app["spec"].as_object().into_iter().flatten() {
        if !NOT_DEFINITION.contains(&k.as_str()) && !v.is_null() {
            d.insert(k.clone(), v.clone());
        }
    }

    let env: Vec<Value> = app["env"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            if e["secret"] == true {
                json!({ "key": e["key"], "secret": true })
            } else {
                json!({ "key": e["key"], "value": e["value"].as_str().unwrap_or_default() })
            }
        })
        .collect();
    d.insert("env".into(), Value::Array(env));

    Value::Object(d)
}

fn set_env(body: &mut Map<String, Value>, entry: Value) {
    let list = body.entry("env").or_insert_with(|| json!([]));
    if !list.is_array() {
        *list = json!([]);
    }

    if let Some(list) = list.as_array_mut() {
        match list.iter_mut().find(|e| e["key"] == entry["key"]) {
            Some(slot) => *slot = entry,
            None => list.push(entry),
        }
    }
}

/// Changes to one definition: the flags, the resolved secret values and the
/// variables to drop.
#[derive(Debug)]
pub struct Changes<'a> {
    pub flags: &'a AppFlags,
    pub secrets: Vec<(String, String)>,
    pub unset_env: &'a [String],
}

impl<'a> Changes<'a> {
    /// The flags alone: no secret values, nothing dropped.
    #[cfg(test)]
    pub const fn of(flags: &'a AppFlags) -> Self {
        Self { flags, secrets: Vec::new(), unset_env: &[] }
    }
}

/// Applies `changes` to the definition `body`.
pub fn apply(body: &mut Value, changes: Changes<'_>) -> Result<()> {
    let Some(b) = body.as_object_mut() else {
        return Err(CliError::usage("the definition must be a JSON object"));
    };

    let f = changes.flags;
    if let Some(r) = f.runtime {
        b.insert("runtime".into(), json!(r.as_str()));
    }

    if let Some(i) = &f.image {
        b.insert("image".into(), json!(i));
    }

    if let Some(c) = &f.command {
        b.insert("command".into(), json!(c));
    }

    if !f.ports.is_empty() {
        b.insert("ports".into(), json!(f.ports));
    }

    if !f.volumes.is_empty() {
        b.insert("volumes".into(), json!(f.volumes));
    }

    if let Some(p) = f.listen_port {
        b.insert("listen_port".into(), json!(p));
    }

    if let Some(r) = f.restart {
        b.insert("restart".into(), json!(r));
    }

    if let Some(p) = f.restart_policy {
        b.insert("restart_policy".into(), json!(p.as_str()));
        b.insert("restart".into(), json!(p != RestartPolicy::No));
    }

    if let Some(n) = f.restart_max_attempts {
        b.insert("restart_max_attempts".into(), json!(n));
    }

    for (key, list) in [("cap_add", &f.cap_add), ("cap_drop", &f.cap_drop), ("dns", &f.dns), ("networks", &f.networks)]
    {
        if !list.is_empty() {
            b.insert(key.into(), json!(list));
        }
    }

    if !f.tmpfs.is_empty() {
        let t = f.tmpfs.iter().map(|t| parse_tmpfs(t)).collect::<Result<Vec<_>>>()?;
        b.insert("tmpfs".into(), Value::Array(t));
    }

    if let Some(i) = f.init {
        b.insert("init".into(), json!(i));
    }

    for key in changes.unset_env {
        let had = b.get("env").and_then(Value::as_array).is_some_and(|l| l.iter().any(|e| e["key"] == *key));
        if !had {
            return Err(CliError::usage(format!("--unset-env {key}: the service has no such variable")));
        }

        if let Some(list) = b.get_mut("env").and_then(Value::as_array_mut) {
            list.retain(|e| e["key"] != *key);
        }
    }

    for (key, value) in parse_env(&f.env)? {
        set_env(b, json!({ "key": key, "value": value }));
    }

    for (key, value) in changes.secrets {
        set_env(b, json!({ "key": key, "value": value, "secret": true }));
    }

    Ok(())
}

/// What `forge services create` sends: the file (if any) with the flags applied,
/// in the environment and under the name given.
pub fn create_body(
    file: Option<Value>,
    environment_id: Option<String>,
    name: Option<&str>,
    changes: Changes<'_>,
) -> Result<Value> {
    let mut body = file.unwrap_or_else(|| json!({}));
    apply(&mut body, changes)?;

    if let Some(ns) = environment_id {
        body["environment_id"] = json!(ns);
    }

    if let Some(n) = name {
        body["name"] = json!(n);
    }

    for (field, flag) in [("environment_id", "--environment"), ("name", "--name"), ("runtime", "--runtime")] {
        if body[field].as_str().is_none_or(str::is_empty) {
            return Err(CliError::usage(format!("pass {flag} (or `{field}` in --file)")));
        }
    }

    Ok(body)
}

/// The role of a placed instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Active,
    Standby,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Standby => "standby",
        }
    }
}

/// `--node <node>[:active|standby]`: the node and its role (`default` when
/// no role is given).
pub fn parse_placement(spec: &str, default: Role) -> Result<(String, Role)> {
    let (node, role) = match spec.rsplit_once(':') {
        None => (spec, default),
        Some((n, "active")) => (n, Role::Active),
        Some((n, "standby")) => (n, Role::Standby),
        Some((_, other)) => {
            return Err(CliError::usage(format!("--node {spec}: role `{other}` is not active or standby")));
        }
    };

    if node.is_empty() {
        return Err(CliError::usage(format!("--node {spec}: name the node")));
    }

    Ok((node.to_owned(), role))
}

/// The body of `PUT /services/{id}/instances` from resolved placements.
pub fn placement_body(placements: &[(String, Role)]) -> Value {
    Value::Array(placements.iter().map(|(id, role)| json!({ "server_id": id, "role": role.as_str() })).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    fn no_stdin() -> Result<String> {
        Err(CliError::usage("stdin must not be read"))
    }

    #[test]
    fn a_container_is_created_from_flags_alone() {
        let flags = AppFlags {
            runtime: Some(Runtime::Container),
            image: Some("nginx:1.27".into()),
            ports: s(&["8080:80"]),
            env: s(&["MODE=prod", "URL=https://x/?a=b"]),
            volumes: s(&["/srv/forge/apps/shop-prod/web/html:/usr/share/nginx/html:ro"]),
            restart: Some(false),
            ..AppFlags::default()
        };
        let changes = Changes { flags: &flags, secrets: vec![("TOKEN".into(), "s3cr3t".into())], unset_env: &[] };
        let body = create_body(None, Some("ns-1".into()), Some("web"), changes).unwrap_or_default();
        assert_eq!(
            body,
            json!({
                "environment_id": "ns-1", "name": "web", "runtime": "container", "image": "nginx:1.27",
                "ports": ["8080:80"], "restart": false,
                "volumes": ["/srv/forge/apps/shop-prod/web/html:/usr/share/nginx/html:ro"],
                "env": [
                    { "key": "MODE", "value": "prod" },
                    { "key": "URL", "value": "https://x/?a=b" },
                    { "key": "TOKEN", "value": "s3cr3t", "secret": true },
                ],
            })
        );
    }

    #[test]
    fn flags_change_a_file_and_required_fields_are_named() {
        let file = json!({ "environment_id": "ns-1", "name": "api", "runtime": "systemd", "command": "old",
                           "env": [{ "key": "A", "value": "1" }] });
        let flags = AppFlags { command: Some("new".into()), listen_port: Some(8080), ..AppFlags::default() };
        let body = create_body(Some(file), None, None, Changes::of(&flags)).unwrap_or_default();
        assert_eq!(
            (&body["command"], &body["listen_port"], &body["env"][0]["key"]),
            (&json!("new"), &json!(8080), &json!("A"))
        );
        let none = AppFlags::default();
        let e = create_body(None, Some("ns".into()), Some("x"), Changes::of(&none));
        assert!(e.as_ref().is_err_and(|e| e.exit == Exit::Usage && e.message.contains("--runtime")), "{e:?}");
        let e = create_body(None, None, Some("x"), Changes::of(&none));
        assert!(e.is_err_and(|e| e.message.contains("--environment")));
    }

    #[test]
    fn an_update_starts_from_the_stored_definition() {
        let stored = json!({
            "id": "a1", "name": "web", "runtime": "container", "environment_id": "ns-1",
            "spec": { "environment": "shop-prod", "image": "nginx:1.26", "ports": ["8080:80"], "listen_port": null,
                      "restart": true, "health": { "checks": [] }, "command": "" },
            "env": [{ "key": "MODE", "value": "prod", "secret": false }, { "key": "TOKEN", "value": null, "secret": true },
                    { "key": "OLD", "value": "x", "secret": false }],
        });
        let mut body = definition(&stored);

        // The server's own fields and the health policy are not sent back.
        assert!(body.get("environment").is_none() && body.get("health").is_none() && body.get("listen_port").is_none());
        let flags = AppFlags { image: Some("nginx:1.27".into()), env: s(&["MODE=uat"]), ..AppFlags::default() };
        let unset = s(&["OLD"]);
        assert!(apply(&mut body, Changes { flags: &flags, secrets: Vec::new(), unset_env: &unset }).is_ok());
        assert_eq!(body["image"], "nginx:1.27");
        assert_eq!(body["ports"], json!(["8080:80"]), "lists that were not given stay");
        assert_eq!(
            body["env"],
            json!([{ "key": "MODE", "value": "uat" }, { "key": "TOKEN", "secret": true }]),
            "MODE replaced, the stored secret kept (no value sent), OLD dropped"
        );
        let missing = s(&["NOPE"]);
        let e = apply(&mut body, Changes { flags: &AppFlags::default(), secrets: Vec::new(), unset_env: &missing });
        assert!(e.is_err_and(|e| e.exit == Exit::Usage));
    }

    #[test]
    fn container_option_flags_map_onto_the_definition() {
        let flags = AppFlags {
            restart_policy: Some(RestartPolicy::OnFailure),
            restart_max_attempts: Some(3),
            cap_drop: s(&["ALL"]),
            dns: s(&["192.0.2.53"]),
            tmpfs: s(&["/tmp:64", "/run"]),
            init: Some(true),
            networks: s(&["backend"]),
            ..AppFlags::default()
        };
        let mut body = definition(&json!({ "runtime": "container", "version": 7, "spec": {}, "env": [] }));
        assert!(apply(&mut body, Changes::of(&flags)).is_ok());

        assert_eq!(body["version"], 7, "the loaded version goes back for the 409 check");
        assert_eq!((&body["restart_policy"], &body["restart"]), (&json!("on-failure"), &json!(true)));
        assert_eq!(body["restart_max_attempts"], 3);
        assert_eq!(body["tmpfs"], json!([{ "target": "/tmp", "size_mb": 64 }, { "target": "/run", "size_mb": 0 }]));
        assert_eq!((&body["init"], &body["networks"]), (&json!(true), &json!(["backend"])));
        assert!(parse_tmpfs("/tmp:big").is_err());
    }

    #[test]
    fn env_pairs_are_checked() {
        assert_eq!(
            parse_env(&s(&["A=", "B=x=y"])).ok(),
            Some(vec![("A".into(), String::new()), ("B".into(), "x=y".into())])
        );

        for bad in ["A", "=x", "1A=x", "A B=x"] {
            assert!(parse_env(&s(&[bad])).is_err(), "{bad}");
        }
    }

    #[test]
    fn secrets_never_come_from_the_command_line() {
        let e = parse_secret_env(&s(&["TOKEN=hunter2"]));
        assert!(e.is_err_and(|e| e.exit == Exit::Usage
            && e.code == "secret_on_command_line"
            && !e.message.contains("hunter2")));
        assert_eq!(
            parse_secret_env(&s(&["A", "B=@b.txt", "C=@-"])).ok(),
            Some(vec![
                ("A".into(), SecretSource::Stdin),
                ("B".into(), SecretSource::File("b.txt".into())),
                ("C".into(), SecretSource::Stdin),
            ])
        );
        assert!(parse_secret_env(&s(&["bad key"])).is_err());
    }

    #[test]
    fn secret_values_are_read_from_stdin_or_files() {
        // One key: all of stdin, without the trailing newline.
        let one = vec![("TOKEN".to_owned(), SecretSource::Stdin)];
        assert_eq!(
            read_secrets(&one, || Ok("p=ss word\r\n".into())).ok(),
            Some(vec![("TOKEN".into(), "p=ss word".into())])
        );

        // Several: one KEY=VALUE line each.
        let two = vec![("A".to_owned(), SecretSource::Stdin), ("B".to_owned(), SecretSource::Stdin)];
        assert_eq!(
            read_secrets(&two, || Ok("B=2=2\nA=1\n".into())).ok(),
            Some(vec![("A".into(), "1".into()), ("B".into(), "2=2".into())])
        );
        assert!(read_secrets(&two, || Ok("A=1\n".into())).is_err_and(|e| e.message.contains("`B=<value>`")));
        assert!(read_secrets(&one, || Ok("\n".into())).is_err(), "an empty secret is refused");

        // A file: stdin is not touched.
        let dir = tempfile::tempdir().ok();
        let path = dir.as_ref().map(|d| d.path().join("token")).unwrap_or_default();
        assert!(std::fs::write(&path, "from-file\n").is_ok());
        let file = vec![("TOKEN".to_owned(), SecretSource::File(path))];
        assert_eq!(read_secrets(&file, no_stdin).ok(), Some(vec![("TOKEN".into(), "from-file".into())]));
        let gone = vec![("TOKEN".to_owned(), SecretSource::File("does/not/exist".into()))];
        assert!(read_secrets(&gone, no_stdin).is_err_and(|e| e.exit == Exit::Usage));
    }

    #[test]
    fn placements_take_a_role_per_node() {
        assert_eq!(parse_placement("lab-b", Role::Active).ok(), Some(("lab-b".into(), Role::Active)));
        assert_eq!(parse_placement("lab-c:standby", Role::Active).ok(), Some(("lab-c".into(), Role::Standby)));
        assert_eq!(parse_placement("lab-c:active", Role::Standby).ok(), Some(("lab-c".into(), Role::Active)));
        assert_eq!(parse_placement("lab-c", Role::Standby).ok(), Some(("lab-c".into(), Role::Standby)));
        assert!(parse_placement("lab-c:primary", Role::Active).is_err_and(|e| e.exit == Exit::Usage));
        assert!(parse_placement(":standby", Role::Active).is_err());
        assert_eq!(
            placement_body(&[("s1".into(), Role::Active), ("s2".into(), Role::Standby)]),
            json!([{ "server_id": "s1", "role": "active" }, { "server_id": "s2", "role": "standby" }])
        );
    }
}
