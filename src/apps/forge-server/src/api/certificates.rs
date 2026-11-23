//! Uploaded TLS certificates for ingress endpoints (paid / company CAs).
//!
//! * `GET    /api/v1/certificates` — metadata only (`workspace view`)
//! * `POST   /api/v1/certificates` — `{name, chain_pem, key_pem}`; validated,
//!   key sealed (`workspace.manage`)
//! * `DELETE /api/v1/certificates/{id}` — refused while an endpoint uses it
//!
//! The private key is never returned: it is sealed at upload and unsealed
//! only when the `agent.command` job sends an endpoint to its edge nodes.
//! Expiry is shown everywhere a certificate is listed.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::model::TlsCertificate;
use forge_core::ports::{CertificateRepository, SiteRepository};
use forge_core::{Action, CertificateId, DomainError, EntityKind, EntityRef, Timestamp, WorkspaceId};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::jobs::agent_command::certificate_aad;
use crate::state::AppState;

fn ws_ref(ws: WorkspaceId) -> EntityRef {
    EntityRef::new(EntityKind::Workspace, ws)
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Upload {
    pub name: String,
    pub chain_pem: String,
    pub key_pem: String,
}

impl std::fmt::Debug for Upload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upload").field("name", &self.name).field("key_pem", &"<redacted>").finish()
    }
}

/// What the upload contains, once checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Parsed {
    pub domains: Vec<String>,
    pub issuer: String,
    pub not_before: i64,
    pub not_after: i64,
    pub fingerprint: String,
    /// The chain re-encoded as clean PEM (leaf first).
    pub chain_pem: String,
}

/// Checks a chain and key: parseable PEM, the key matches the leaf, the leaf
/// is currently valid. Returns what the console shows.
pub fn parse(chain_pem: &str, key_pem: &str, now_ms: i64) -> Result<Parsed, String> {
    if chain_pem.len() > 64 * 1024 || key_pem.len() > 16 * 1024 {
        return Err("certificate or key too large".into());
    }
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|_| "the certificate is not valid PEM".to_owned())?;
    let leaf = certs.first().ok_or("no certificate found (expected -----BEGIN CERTIFICATE-----)")?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|_| "the private key is not valid PEM (PKCS#8, PKCS#1 or SEC1, unencrypted)".to_owned())?;

    let (_, x) = x509_parser::parse_x509_certificate(leaf.as_ref()).map_err(|_| "the certificate cannot be parsed")?;
    let signing = rustls::crypto::ring::default_provider()
        .key_provider
        .load_private_key(key)
        .map_err(|_| "unsupported private key type (RSA, ECDSA P-256/P-384 or Ed25519)".to_owned())?;
    let spki = signing.public_key().ok_or("cannot derive the public key from the private key")?;
    if spki.as_ref() != x.tbs_certificate.subject_pki.raw {
        return Err("the private key does not belong to this certificate".into());
    }

    let not_before = x.validity().not_before.timestamp() * 1000;
    let not_after = x.validity().not_after.timestamp() * 1000;
    if not_after <= now_ms {
        return Err("the certificate has expired".into());
    }
    let mut domains: Vec<String> = x
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    x509_parser::extensions::GeneralName::DNSName(d) => Some(d.to_ascii_lowercase()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if domains.is_empty() {
        domains = x.subject().iter_common_name().filter_map(|c| c.as_str().ok()).map(str::to_ascii_lowercase).collect();
    }
    let issuer = x
        .issuer()
        .iter_organization()
        .chain(x.issuer().iter_common_name())
        .filter_map(|a| a.as_str().ok())
        .next()
        .unwrap_or("unknown")
        .to_owned();
    let fingerprint = hex::encode(ring::digest::digest(&ring::digest::SHA256, leaf.as_ref()));

    let b64 = |der: &[u8]| {
        use base64::Engine;
        let s = base64::engine::general_purpose::STANDARD.encode(der);
        let lines: Vec<&str> = s.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
        format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
    };
    let chain_pem = certs.iter().map(|c| b64(c.as_ref())).collect::<String>();
    Ok(Parsed { domains, issuer, not_before, not_after, fingerprint, chain_pem })
}

/// Whether `cert_domains` covers `name` (exact, or one-label wildcard).
#[must_use]
pub fn covers(cert_domains: &[String], name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    cert_domains.iter().any(|d| {
        d == &name
            || d.strip_prefix("*.").is_some_and(|base| name.split_once('.').is_some_and(|(_, rest)| rest == base))
    })
}

fn view(c: &TlsCertificate, used_by: &[String], now: Timestamp) -> Value {
    json!({
        "id": c.id,
        "name": c.name,
        "domains": c.domains,
        "issuer": c.issuer,
        "not_before": c.not_before,
        "not_after": c.not_after,
        "days_left": (c.not_after.0 - now.0).div_euclid(86_400_000),
        "fingerprint": c.fingerprint,
        "used_by": used_by,
        "created_at": c.created_at,
        "updated_at": c.updated_at,
    })
}

/// Endpoint names using each certificate.
/// Per uploaded certificate, the endpoints that serve it: every endpoint
/// with `whole`, else only those the caller may view.
async fn usage(
    state: &AppState,
    caller: &Caller,
    whole: bool,
) -> ApiResult<std::collections::HashMap<String, Vec<String>>> {
    let mut out: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let sites = state.store.list_sites().await?;
    let visible = if whole { vec![true; sites.len()] } else { super::sites::visible_sites(caller, &sites).await };
    for (s, _) in sites.into_iter().zip(visible).filter(|(_, ok)| *ok) {
        if s.config["spec"]["tls"] == "uploaded"
            && let Some(id) = s.config["spec"]["cert_id"].as_str()
        {
            out.entry(id.to_owned()).or_default().push(s.name.clone());
        }
    }
    Ok(out)
}

pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Value>>> {
    // `view` on the workspace shows every certificate. Any other member of
    // the workspace sees the certificates the endpoints they may view serve
    // (and only those endpoints in "used by"). Without any role here the
    // request is refused (and recorded).
    let whole = caller.can(Action::View, ws_ref(ws)).await;
    if !whole && !crate::console_access::is_member(&state.store, &state.store, caller.session.user.id, ws).await? {
        caller.authorize(Action::View, ws_ref(ws)).await?;
    }
    let used = usage(&state, &caller, whole).await?;
    let now = state.clock.now();
    Ok(Json(
        state
            .store
            .list_certificates(Some(ws))
            .await?
            .iter()
            .filter(|c| whole || used.contains_key(&c.id.to_string()))
            .map(|c| view(c, used.get(&c.id.to_string()).map_or(&[], Vec::as_slice), now))
            .collect(),
    ))
}

/// Loads a certificate of this workspace (for endpoint validation).
pub async fn load(state: &AppState, ws: WorkspaceId, id: &str) -> ApiResult<TlsCertificate> {
    let id: CertificateId = id.parse().map_err(|_| ApiError::bad_request("invalid certificate id"))?;
    match state.store.get_certificate(id).await? {
        Some(c) if c.workspace_id == ws => Ok(c),
        _ => Err(ApiError::bad_request("certificate not found")),
    }
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<Upload>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    caller.authorize(Action::WorkspaceManage, ws_ref(ws)).await?;
    let name = req.name.trim().to_owned();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::bad_request("name: 1 to 100 characters"));
    }
    let now = state.clock.now();
    let p = parse(&req.chain_pem, &req.key_pem, now.0).map_err(ApiError::bad_request)?;
    let id = CertificateId::new();
    let sealed_key =
        state.envelope.seal(req.key_pem.as_bytes(), &certificate_aad(id)).map_err(DomainError::internal)?;
    let cert = TlsCertificate {
        id,
        workspace_id: ws,
        name,
        domains: p.domains,
        issuer: p.issuer,
        not_before: Timestamp(p.not_before),
        not_after: Timestamp(p.not_after),
        fingerprint: p.fingerprint,
        chain_pem: p.chain_pem,
        sealed_key,
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
    };
    state.store.put_certificate(&cert).await?;
    caller
        .audit(
            "certificate.upload",
            &ws_ref(ws),
            Some(ws),
            json!({ "certificate": id, "name": cert.name, "domains": cert.domains, "not_after": cert.not_after, "fingerprint": cert.fingerprint }),
        )
        .await;
    Ok((StatusCode::CREATED, Json(view(&cert, &[], now))))
}

pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::WorkspaceManage, ws_ref(ws)).await?;
    let cert = load(&state, ws, &id).await.map_err(|_| ApiError::not_found("certificate not found"))?;
    if let Some(users) = usage(&state, &caller, true).await?.get(&cert.id.to_string()) {
        return Err(ApiError::conflict(format!("used by {}; switch those endpoints first", users.join(", "))));
    }
    state.store.delete_certificate(cert.id).await?;
    caller
        .audit("certificate.delete", &ws_ref(ws), Some(ws), json!({ "certificate": cert.id, "name": cert.name }))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn self_signed(names: &[&str]) -> (String, String) {
        let c = rcgen::generate_simple_self_signed(names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap();
        (c.cert.pem(), c.key_pair.serialize_pem())
    }

    #[test]
    fn accepts_a_matching_pair_and_reports_names() {
        let (crt, key) = self_signed(&["shop.example.com", "*.shop.example.com"]);
        let p = parse(&crt, &key, 0).unwrap();
        assert_eq!(p.domains, vec!["shop.example.com", "*.shop.example.com"]);
        assert_eq!(p.fingerprint.len(), 64);
        assert!(p.not_after > p.not_before);
        assert!(covers(&p.domains, "shop.example.com"));
        assert!(covers(&p.domains, "api.shop.example.com"));
        assert!(!covers(&p.domains, "a.b.shop.example.com"));
        assert!(!covers(&p.domains, "example.com"));
    }

    #[test]
    fn rejects_mismatched_keys_garbage_and_expired() {
        let (crt, _) = self_signed(&["a.example.com"]);
        let (_, other_key) = self_signed(&["b.example.com"]);
        assert!(parse(&crt, &other_key, 0).unwrap_err().contains("does not belong"));
        assert!(parse("nope", &other_key, 0).is_err());
        let (crt, key) = self_signed(&["a.example.com"]);
        assert!(parse(&crt, &key, i64::MAX).unwrap_err().contains("expired"));
    }
}
