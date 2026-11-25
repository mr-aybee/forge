use forge_core::Timestamp;
use x509_parser::prelude::*;

use crate::PkiError;

/// Identity extracted from a peer certificate presented over mTLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    /// For agents, the server id assigned at enrollment.
    pub common_name: String,
    pub serial_hex: String,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

impl PeerIdentity {
    /// Reads an identity from a DER certificate.
    pub fn from_der(der: &[u8]) -> Result<Self, PkiError> {
        let (_, cert) = X509Certificate::from_der(der).map_err(|e| PkiError::Parse(e.to_string()))?;
        let common_name = cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .ok_or_else(|| PkiError::Parse("certificate has no common name".into()))?
            .to_owned();
        let validity = cert.validity();

        Ok(Self {
            common_name,
            serial_hex: hex::encode(cert.raw_serial()),
            not_before: Timestamp(validity.not_before.timestamp() * 1000),
            not_after: Timestamp(validity.not_after.timestamp() * 1000),
        })
    }
}
