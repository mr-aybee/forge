use std::path::Path;
use std::time::Duration;

use forge_core::Timestamp;
use forge_secrets::crypto::{random_bytes, sha256_hex};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequestParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType, SerialNumber,
};
use time::OffsetDateTime;

use crate::{PkiError, agent_dns_name};

/// A certificate issued to an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCertificate {
    pub cert_pem: String,
    pub serial_hex: String,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

/// How long a server certificate from [`CertificateAuthority::issue_server_certificate`] is valid.
pub const SERVER_CERT_VALIDITY: Duration = Duration::from_secs(90 * 24 * 3600);

/// TLS identity for the control plane's agent listener.
#[derive(Debug, Clone)]
pub struct ServerCertificate {
    pub cert_pem: String,
    pub key_pem: String,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

/// Agent-side: a freshly generated key and its CSR. The private key never
/// leaves the agent.
#[derive(Debug)]
pub struct AgentCsr {
    pub csr_pem: String,
    pub key_pem: String,
}

impl AgentCsr {
    /// Generates a new CA for `hostname`.
    pub fn generate(hostname: &str) -> Result<Self, PkiError> {
        let key = KeyPair::generate().map_err(gen_err)?;
        let mut params = CertificateParams::new(Vec::<String>::new()).map_err(gen_err)?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, hostname);
        params.distinguished_name = dn;
        let csr = params.serialize_request(&key).map_err(gen_err)?;

        Ok(Self { csr_pem: csr.pem().map_err(gen_err)?, key_pem: key.serialize_pem() })
    }
}

/// The internal certificate authority.
pub struct CertificateAuthority {
    issuer: Certificate,
    key: KeyPair,
    cert_pem: String,
    fingerprint: String,
}

impl std::fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateAuthority").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}

fn gen_err(e: rcgen::Error) -> PkiError {
    PkiError::Generate(e.to_string())
}

fn odt(t: Timestamp) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(t.0 / 1000).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn serial() -> (SerialNumber, String) {
    let mut bytes = random_bytes::<16>();

    // Positive (top bit clear) and never a leading zero byte: DER drops
    // leading zero bytes, and the hex recorded here must be the one a peer
    // presents (`PeerIdentity::serial_hex`). 126 random bits.
    bytes[0] = (bytes[0] & 0x7f) | 0x40;

    (SerialNumber::from_slice(&bytes), hex::encode(bytes))
}

impl CertificateAuthority {
    /// Generates a new root valid for ten years.
    pub fn generate(common_name: &str, now: Timestamp) -> Result<Self, PkiError> {
        let key = KeyPair::generate().map_err(gen_err)?;
        let mut params = CertificateParams::new(Vec::<String>::new()).map_err(gen_err)?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        dn.push(DnType::OrganizationName, "Orbyn Forge");
        params.distinguished_name = dn;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages =
            vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        params.not_before = odt(now.minus(Duration::from_secs(300)));
        params.not_after = odt(now.plus(Duration::from_secs(10 * 365 * 24 * 3600)));
        params.serial_number = Some(serial().0);
        let cert = params.self_signed(&key).map_err(gen_err)?;
        let cert_pem = cert.pem();

        Self::assemble(cert, key, cert_pem)
    }

    /// Loads `ca.crt`/`ca.key` from `dir`, creating them on first start.
    pub fn load_or_create(dir: &Path, now: Timestamp) -> Result<Self, PkiError> {
        let cert_path = dir.join("ca.crt");
        let key_path = dir.join("ca.key");
        if cert_path.exists() && key_path.exists() {
            return Self::from_pem(&std::fs::read_to_string(&cert_path)?, &std::fs::read_to_string(&key_path)?);
        }

        let ca = Self::generate("Orbyn Forge Internal CA", now)?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(&cert_path, &ca.cert_pem)?;
        forge_secrets::write_private(&key_path, ca.key.serialize_pem().as_bytes())?;

        Ok(ca)
    }

    /// Loads a CA from its PEM certificate and key.
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, PkiError> {
        let key = KeyPair::from_pem(key_pem).map_err(gen_err)?;

        // Rebuild an issuer object from the stored certificate's parameters;
        // only its subject and key are used for signing.
        let params = CertificateParams::from_ca_cert_pem(cert_pem).map_err(gen_err)?;
        let issuer = params.self_signed(&key).map_err(gen_err)?;

        Self::assemble(issuer, key, cert_pem.to_owned())
    }

    fn assemble(issuer: Certificate, key: KeyPair, cert_pem: String) -> Result<Self, PkiError> {
        let der = pem_to_der(&cert_pem)?;
        Ok(Self { issuer, key, fingerprint: sha256_hex(&der), cert_pem })
    }

    /// PEM of the root certificate, distributed to agents.
    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// SHA-256 of the root certificate DER (hex), pinned in enrollment tokens.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Signs an agent CSR. The subject and SANs come from `server_id`, not the
    /// CSR; only the CSR's public key is used.
    pub fn issue_agent_certificate(
        &self,
        csr_pem: &str,
        server_id: &str,
        now: Timestamp,
        validity: Duration,
    ) -> Result<IssuedCertificate, PkiError> {
        let mut csr = CertificateSigningRequestParams::from_pem(csr_pem).map_err(|e| PkiError::Csr(e.to_string()))?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, server_id);
        dn.push(DnType::OrganizationName, "Orbyn Forge Agent");
        csr.params.distinguished_name = dn;
        csr.params.subject_alt_names = vec![SanType::DnsName(
            agent_dns_name(server_id).try_into().map_err(|e: rcgen::Error| PkiError::Csr(e.to_string()))?,
        )];
        csr.params.is_ca = IsCa::ExplicitNoCa;
        csr.params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        csr.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let not_before = now.minus(Duration::from_secs(300));
        let not_after = now.plus(validity);
        csr.params.not_before = odt(not_before);
        csr.params.not_after = odt(not_after);
        let (sn, serial_hex) = serial();
        csr.params.serial_number = Some(sn);
        csr.params.custom_extensions.clear();
        let cert = csr.signed_by(&self.issuer, &self.key).map_err(gen_err)?;

        Ok(IssuedCertificate { cert_pem: cert.pem(), serial_hex, not_before, not_after })
    }

    /// Issues the control plane's TLS server certificate for `hostnames`
    /// (DNS names or IP addresses), with a fresh key.
    pub fn issue_server_certificate(
        &self,
        hostnames: &[String],
        now: Timestamp,
    ) -> Result<ServerCertificate, PkiError> {
        let key = KeyPair::generate().map_err(gen_err)?;
        self.server_certificate(&key, hostnames, now)
    }

    /// Issues a listener certificate for the control plane's long-lived key
    /// (D-167): renewing it keeps the key, so agents that pinned the key
    /// never notice. Valid for [`SERVER_CERT_VALIDITY`].
    pub fn issue_server_certificate_for_key(
        &self,
        key: &crate::ControlPlaneKey,
        hostnames: &[String],
        now: Timestamp,
    ) -> Result<ServerCertificate, PkiError> {
        self.server_certificate(key.key_pair(), hostnames, now)
    }

    fn server_certificate(
        &self,
        key: &KeyPair,
        hostnames: &[String],
        now: Timestamp,
    ) -> Result<ServerCertificate, PkiError> {
        let mut params = CertificateParams::new(hostnames.to_vec()).map_err(gen_err)?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, hostnames.first().map_or("forge", String::as_str));
        params.distinguished_name = dn;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let (not_before, not_after) = (now.minus(Duration::from_secs(300)), now.plus(SERVER_CERT_VALIDITY));
        params.not_before = odt(not_before);
        params.not_after = odt(not_after);
        params.serial_number = Some(serial().0);
        let cert = params.signed_by(key, &self.issuer, &self.key).map_err(gen_err)?;

        Ok(ServerCertificate { cert_pem: cert.pem(), key_pem: key.serialize_pem(), not_before, not_after })
    }
}

impl CertificateAuthority {
    /// PEM of the CA's private key, for a caller that seals it at rest (a
    /// node's Docker CA is kept sealed in the database, D-125).
    #[must_use]
    pub fn key_pem(&self) -> String {
        self.key.serialize_pem()
    }

    /// Signs a node's Docker daemon CSR (D-125). Only the CSR's public key
    /// is used: the names are `crate::docker_dns_name(server_id)` and the
    /// addresses the daemon listens on, the purpose is TLS server only.
    pub fn issue_docker_daemon_certificate(
        &self,
        csr_pem: &str,
        server_id: &str,
        addresses: &[std::net::IpAddr],
        now: Timestamp,
        validity: Duration,
    ) -> Result<IssuedCertificate, PkiError> {
        let mut csr = CertificateSigningRequestParams::from_pem(csr_pem).map_err(|e| PkiError::Csr(e.to_string()))?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, crate::docker_dns_name(server_id));
        dn.push(DnType::OrganizationName, "Orbyn Forge Docker");
        csr.params.distinguished_name = dn;
        let mut sans = vec![SanType::DnsName(
            crate::docker_dns_name(server_id).try_into().map_err(|e: rcgen::Error| PkiError::Csr(e.to_string()))?,
        )];
        sans.extend(addresses.iter().map(|a| SanType::IpAddress(*a)));
        csr.params.subject_alt_names = sans;
        csr.params.is_ca = IsCa::ExplicitNoCa;
        csr.params.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
        csr.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let not_before = now.minus(Duration::from_secs(300));
        let not_after = now.plus(validity);
        csr.params.not_before = odt(not_before);
        csr.params.not_after = odt(not_after);
        let (sn, serial_hex) = serial();
        csr.params.serial_number = Some(sn);
        csr.params.custom_extensions.clear();
        let cert = csr.signed_by(&self.issuer, &self.key).map_err(gen_err)?;

        Ok(IssuedCertificate { cert_pem: cert.pem(), serial_hex, not_before, not_after })
    }

    /// A TLS client certificate and key named `common_name`, valid for
    /// `validity` (the control plane's short-lived Docker client identity).
    pub fn issue_client_certificate(
        &self,
        common_name: &str,
        now: Timestamp,
        validity: Duration,
    ) -> Result<ServerCertificate, PkiError> {
        let key = KeyPair::generate().map_err(gen_err)?;
        let mut params = CertificateParams::new(Vec::<String>::new()).map_err(gen_err)?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, common_name);
        params.distinguished_name = dn;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let (not_before, not_after) = (now.minus(Duration::from_secs(300)), now.plus(validity));
        params.not_before = odt(not_before);
        params.not_after = odt(not_after);
        params.serial_number = Some(serial().0);
        let cert = params.signed_by(&key, &self.issuer, &self.key).map_err(gen_err)?;

        Ok(ServerCertificate { cert_pem: cert.pem(), key_pem: key.serialize_pem(), not_before, not_after })
    }
}

/// Decodes the first PEM block to DER.
pub(crate) fn pem_to_der(pem: &str) -> Result<Vec<u8>, PkiError> {
    let (_, p) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).map_err(|e| PkiError::Parse(e.to_string()))?;
    Ok(p.contents)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::PeerIdentity;

    const NOW: Timestamp = Timestamp(1_800_000_000_000);

    #[test]
    fn issues_agent_cert_with_server_identity_not_csr_subject() {
        let ca = CertificateAuthority::generate("test", NOW).unwrap();
        let csr = AgentCsr::generate("evil-claims-to-be-admin").unwrap();
        let issued = ca.issue_agent_certificate(&csr.csr_pem, "srv-123", NOW, Duration::from_secs(3600)).unwrap();
        let id = PeerIdentity::from_der(&pem_to_der(&issued.cert_pem).unwrap()).unwrap();
        assert_eq!(id.common_name, "srv-123");
        assert_eq!(id.serial_hex, issued.serial_hex);
        assert_eq!(id.not_after.0 / 1000, issued.not_after.0 / 1000);
    }

    #[test]
    fn ca_round_trips_through_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = CertificateAuthority::load_or_create(dir.path(), NOW).unwrap();
        let b = CertificateAuthority::load_or_create(dir.path(), NOW).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());

        // The reloaded CA can still issue.
        let csr = AgentCsr::generate("h").unwrap();
        b.issue_agent_certificate(&csr.csr_pem, "s", NOW, Duration::from_secs(60)).unwrap();
    }

    #[test]
    fn rejects_garbage_csr() {
        let ca = CertificateAuthority::generate("test", NOW).unwrap();
        assert!(ca.issue_agent_certificate("not a csr", "s", NOW, Duration::from_secs(60)).is_err());
    }

    #[test]
    fn docker_certificates_name_the_node_and_its_purpose() {
        let ca = CertificateAuthority::generate("Docker CA srv-1", NOW).unwrap();
        let csr = AgentCsr::generate("ignored").unwrap();
        let ip: std::net::IpAddr = "10.0.0.5".parse().unwrap();
        let d =
            ca.issue_docker_daemon_certificate(&csr.csr_pem, "srv-1", &[ip], NOW, Duration::from_secs(86400)).unwrap();
        let (_, pem) = x509_parser::pem::parse_x509_pem(d.cert_pem.as_bytes()).unwrap();
        let cert = pem.parse_x509().unwrap();
        let san = cert.subject_alternative_name().unwrap().unwrap().value.general_names.clone();
        let text = format!("{san:?}");
        assert!(text.contains("srv-1.docker.forge.internal") && text.contains("[10, 0, 0, 5]"), "{text}");
        let client = ca.issue_client_certificate("forge-server", NOW, Duration::from_secs(3600)).unwrap();
        assert!(client.key_pem.contains("PRIVATE KEY"));
        let again = CertificateAuthority::from_pem(ca.cert_pem(), &ca.key_pem()).unwrap();
        assert_eq!(again.fingerprint(), ca.fingerprint());
    }

    #[test]
    fn server_cert_issues() {
        let ca = CertificateAuthority::generate("test", NOW).unwrap();
        let s = ca.issue_server_certificate(&["localhost".into(), "127.0.0.1".into()], NOW).unwrap();
        assert!(s.cert_pem.contains("BEGIN CERTIFICATE"));
    }
}
