//! A node's Docker Engine API on TCP with mutual TLS (D-125).
//!
//! The root helper runs these operations from a signed command only:
//!
//! * [`DockerTlsOp::Prepare`] makes dockerd's key on the node (it never
//!   leaves) and returns a CSR with what the control plane needs to decide:
//!   the node's addresses, Docker's version, whether `live-restore` keeps
//!   containers running through a daemon restart, and how many run.
//! * [`DockerTlsOp::Apply`] installs the node's Docker CA and dockerd's
//!   signed certificate, adds `-H tcp://<listen>:<port> --tlsverify` to
//!   dockerd through a systemd drop-in (the local socket stays as it is),
//!   restarts dockerd and rolls back if it does not come up.
//! * [`DockerTlsOp::Remove`] takes the listener away again.
//! * [`DockerTlsOp::Status`] reads what is installed.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// The port Docker documents for TLS.
pub const DEFAULT_PORT: u16 = 2376;
/// Upper bound on a PEM a command may carry.
const MAX_PEM: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DockerTlsOp {
    Prepare {},
    Apply {
        /// The node's Docker CA certificate (PEM): dockerd accepts only
        /// clients it signed.
        ca_pem: String,
        /// dockerd's certificate (PEM), signed for the key `Prepare` made.
        cert_pem: String,
        /// Address dockerd listens on (one of the node's, or `0.0.0.0`).
        listen: IpAddr,
        port: u16,
    },
    Remove {},
    Status {},
}

fn pem_ok(pem: &str, label: &str) -> bool {
    pem.len() <= MAX_PEM
        && pem.contains(&format!("-----BEGIN {label}-----"))
        && pem.contains(&format!("-----END {label}-----"))
        && pem.bytes().all(|b| b.is_ascii_graphic() || matches!(b, b' ' | b'\n' | b'\r'))
}

impl DockerTlsOp {
    /// Checks the operation before anything runs as root.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Apply { ca_pem, cert_pem, listen, port } => {
                if !pem_ok(ca_pem, "CERTIFICATE") || !pem_ok(cert_pem, "CERTIFICATE") {
                    return Err("certificates must be PEM".into());
                }

                if *port < 1024 {
                    return Err("the port must be 1024 or higher".into());
                }

                if listen.is_loopback() || listen.is_multicast() {
                    return Err("dockerd must listen on an address the control plane can reach".into());
                }

                Ok(())
            }
            Self::Prepare {} | Self::Remove {} | Self::Status {} => Ok(()),
        }
    }
}

/// What `Prepare` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prepared {
    pub csr_pem: String,
    /// The node's non-loopback addresses.
    pub addresses: Vec<IpAddr>,
    pub docker_version: String,
    /// Containers keep running while dockerd restarts.
    pub live_restore: bool,
    pub running_containers: u64,
    /// The TLS listener is already installed (`listen:port`), if so.
    pub installed: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";

    #[test]
    fn apply_is_checked() {
        let ok = DockerTlsOp::Apply {
            ca_pem: PEM.into(),
            cert_pem: PEM.into(),
            listen: "10.0.0.5".parse().unwrap(),
            port: 2376,
        };
        assert!(ok.validate().is_ok());
        let mut bad = ok.clone();
        if let DockerTlsOp::Apply { listen, .. } = &mut bad {
            *listen = "127.0.0.1".parse().unwrap();
        }
        assert!(bad.validate().is_err());
        let shell = DockerTlsOp::Apply {
            ca_pem: "x; rm -rf /".into(),
            cert_pem: PEM.into(),
            listen: "10.0.0.5".parse().unwrap(),
            port: 2376,
        };
        assert!(shell.validate().is_err());
        let low = DockerTlsOp::Apply {
            ca_pem: PEM.into(),
            cert_pem: PEM.into(),
            listen: "10.0.0.5".parse().unwrap(),
            port: 22,
        };
        assert!(low.validate().is_err());
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(serde_json::from_str::<DockerTlsOp>(r#"{"op":"prepare","extra":1}"#).is_err());
        assert_eq!(serde_json::from_str::<DockerTlsOp>(r#"{"op":"status"}"#).unwrap(), DockerTlsOp::Status {});
    }
}
