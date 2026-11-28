//! Local allowlist: the server's operator decides which command kinds run.

use std::collections::HashSet;

use forge_proto::Command;

#[derive(Debug, Clone)]
pub struct LocalPolicy {
    allowed: HashSet<String>,
}

impl LocalPolicy {
    #[must_use]
    pub fn new(allowed: &[String]) -> Self {
        Self { allowed: allowed.iter().cloned().collect() }
    }

    pub fn check(&self, cmd: &Command) -> Result<(), String> {
        if self.allowed.contains(cmd.name()) {
            Ok(())
        } else {
            Err(format!("command `{}` is not allowed by this agent's local policy", cmd.name()))
        }
    }
}

#[cfg(test)]
mod tests {
    use forge_proto::messages::{Ping, ServiceStatus};

    use super::*;

    #[test]
    fn only_listed_commands_pass() {
        let p = LocalPolicy::new(&["ping".into()]);
        assert!(p.check(&Command::Ping(Ping {})).is_ok());
        assert!(p.check(&Command::ServiceStatus(ServiceStatus { name: "x".into() })).is_err());
    }
}
