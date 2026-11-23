//! WebAuthn (passkeys) verification with `ring` (D-055).
//!
//! Forge verifies registrations and assertions itself instead of using
//! webauthn-rs, which needs OpenSSL. The scope is deliberately small:
//!
//! * Attestation format `none` only (browsers send it unless asked otherwise);
//!   Forge does not check authenticator makes.
//! * Algorithms ES256 (-7), EdDSA (-8) and RS256 (-257); nothing else.
//! * Every check the spec requires for a relying party: client data type,
//!   single-use challenge, origin, RP ID hash, user present (UP), user
//!   verified (UV, always required: passkeys stand in for MFA), attested
//!   credential data at registration, signature counter regression, and the
//!   credential belonging to the user and to the allowed list.
//!
//! Challenges are generated, stored hashed with an expiry and consumed once
//! by the caller (see `passkeys.rs`); this module compares them.

use ring::signature::{self, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

use crate::cbor::{self, Value};

/// Flags in authenticator data.
const UP: u8 = 0x01;
const UV: u8 = 0x04;
const AT: u8 = 0x40;
const ED: u8 = 0x80;

/// Longest credential id accepted (the spec allows up to 1023 bytes).
pub const MAX_CREDENTIAL_ID: usize = 1023;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebAuthnError {
    #[error("malformed passkey data: {0}")]
    Malformed(String),
    #[error("the passkey response does not match this sign-in: {0}")]
    Mismatch(&'static str),
    #[error("unsupported passkey: {0}")]
    Unsupported(String),
    #[error("the passkey signature is not valid")]
    BadSignature,
    #[error("the passkey's counter went backwards; it may have been cloned")]
    CounterRegression,
}

impl From<cbor::CborError> for WebAuthnError {
    fn from(e: cbor::CborError) -> Self {
        Self::Malformed(e.to_string())
    }
}

/// This Forge as a WebAuthn relying party.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    /// RP ID: the host of the console URL (`forge.example.com`).
    pub id: String,
    /// Exact origin the browser reports (`https://forge.example.com`).
    pub origin: String,
}

impl RelyingParty {
    /// From the console's public URL.
    pub fn from_public_url(url: &str) -> Result<Self, WebAuthnError> {
        let u = url::Url::parse(url).map_err(|_| WebAuthnError::Malformed("public_url".into()))?;
        let host = u.host_str().ok_or_else(|| WebAuthnError::Malformed("public_url has no host".into()))?;
        Ok(Self { id: host.to_ascii_lowercase(), origin: u.origin().ascii_serialization() })
    }
}

/// A credential public key as Forge stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "alg", rename_all = "snake_case")]
pub enum PublicKey {
    /// P-256, uncompressed point `04 || x || y` (65 bytes).
    Es256 { point: Vec<u8> },
    /// Ed25519 (32 bytes).
    EdDsa { key: Vec<u8> },
    /// RSA modulus and exponent (big-endian).
    Rs256 { n: Vec<u8>, e: Vec<u8> },
}

impl PublicKey {
    /// COSE algorithm number.
    #[must_use]
    pub const fn cose_alg(&self) -> i64 {
        match self {
            Self::Es256 { .. } => -7,
            Self::EdDsa { .. } => -8,
            Self::Rs256 { .. } => -257,
        }
    }

    /// Parses a COSE_Key with the algorithms Forge accepts.
    pub fn from_cose(v: &Value) -> Result<Self, WebAuthnError> {
        let int = |k: i128| v.get_int(k).and_then(Value::as_int);
        let bytes = |k: i128| v.get_int(k).and_then(Value::as_bytes).map(<[u8]>::to_vec);
        match (int(1), int(3)) {
            // kty EC2, alg ES256, crv P-256
            (Some(2), Some(-7)) => {
                if int(-1) != Some(1) {
                    return Err(WebAuthnError::Unsupported("EC2 key on a curve other than P-256".into()));
                }
                let (x, y) = (bytes(-2), bytes(-3));
                match (x, y) {
                    (Some(x), Some(y)) if x.len() == 32 && y.len() == 32 => {
                        let mut point = vec![0x04];
                        point.extend(x);
                        point.extend(y);
                        Ok(Self::Es256 { point })
                    }
                    _ => Err(WebAuthnError::Malformed("P-256 coordinates".into())),
                }
            }
            // kty OKP, alg EdDSA, crv Ed25519
            (Some(1), Some(-8)) => {
                if int(-1) != Some(6) {
                    return Err(WebAuthnError::Unsupported("OKP key on a curve other than Ed25519".into()));
                }
                match bytes(-2) {
                    Some(k) if k.len() == 32 => Ok(Self::EdDsa { key: k }),
                    _ => Err(WebAuthnError::Malformed("Ed25519 key".into())),
                }
            }
            // kty RSA, alg RS256
            (Some(3), Some(-257)) => match (bytes(-1), bytes(-2)) {
                (Some(n), Some(e)) if (256..=512).contains(&n.len()) && !e.is_empty() && e.len() <= 8 => {
                    Ok(Self::Rs256 { n, e })
                }
                _ => Err(WebAuthnError::Malformed("RSA key (2048 to 4096 bits)".into())),
            },
            (kty, alg) => Err(WebAuthnError::Unsupported(format!("key type {kty:?} with algorithm {alg:?}"))),
        }
    }

    /// Verifies `sig` over `msg`.
    pub fn verify(&self, msg: &[u8], sig: &[u8]) -> Result<(), WebAuthnError> {
        let ok = match self {
            Self::Es256 { point } => UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, point).verify(msg, sig),
            Self::EdDsa { key } => UnparsedPublicKey::new(&signature::ED25519, key).verify(msg, sig),
            Self::Rs256 { n, e } => {
                signature::RsaPublicKeyComponents { n, e }.verify(&signature::RSA_PKCS1_2048_8192_SHA256, msg, sig)
            }
        };
        ok.map_err(|_| WebAuthnError::BadSignature)
    }
}

/// Parsed authenticator data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    /// Present at registration (`AT` flag).
    pub credential: Option<(Vec<u8>, PublicKey)>,
}

impl AuthData {
    pub fn parse(data: &[u8]) -> Result<Self, WebAuthnError> {
        if data.len() < 37 {
            return Err(WebAuthnError::Malformed("authenticator data too short".into()));
        }
        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&data[..32]);
        let flags = data[32];
        let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
        let mut rest = &data[37..];
        let mut credential = None;
        if flags & AT != 0 {
            if rest.len() < 18 {
                return Err(WebAuthnError::Malformed("attested credential data too short".into()));
            }
            let id_len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
            if id_len == 0 || id_len > MAX_CREDENTIAL_ID {
                return Err(WebAuthnError::Malformed("credential id length".into()));
            }
            let id = rest
                .get(18..18 + id_len)
                .ok_or_else(|| WebAuthnError::Malformed("credential id truncated".into()))?
                .to_vec();
            rest = &rest[18 + id_len..];
            let (key, used) = cbor::decode_prefix(rest)?;
            rest = &rest[used..];
            credential = Some((id, PublicKey::from_cose(&key)?));
        }
        if flags & ED != 0 {
            // Extensions: one CBOR map; Forge requests none and ignores them.
            let (ext, used) = cbor::decode_prefix(rest)?;
            if !matches!(ext, Value::Map(_)) {
                return Err(WebAuthnError::Malformed("extensions must be a map".into()));
            }
            rest = &rest[used..];
        }
        if !rest.is_empty() {
            return Err(WebAuthnError::Malformed("trailing bytes in authenticator data".into()));
        }
        Ok(Self { rp_id_hash, flags, sign_count, credential })
    }
}

#[derive(Debug, Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: bool,
}

fn sha256(data: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, data).as_ref().to_vec()
}

/// Checks client data: type, challenge, origin, not cross-origin.
fn check_client_data(rp: &RelyingParty, json: &[u8], kind: &str, challenge: &[u8]) -> Result<(), WebAuthnError> {
    if json.len() > 8 * 1024 {
        return Err(WebAuthnError::Malformed("client data too large".into()));
    }
    let c: ClientData =
        serde_json::from_slice(json).map_err(|_| WebAuthnError::Malformed("client data JSON".into()))?;
    if c.kind != kind {
        return Err(WebAuthnError::Mismatch("client data type"));
    }
    let got = forge_secrets::crypto::b64url_decode(&c.challenge)
        .ok_or_else(|| WebAuthnError::Malformed("challenge encoding".into()))?;
    if !forge_secrets::crypto::constant_time_eq(&got, challenge) {
        return Err(WebAuthnError::Mismatch("challenge"));
    }
    if c.origin != rp.origin {
        return Err(WebAuthnError::Mismatch("origin"));
    }
    if c.cross_origin {
        return Err(WebAuthnError::Mismatch("cross-origin request"));
    }
    Ok(())
}

fn check_flags(rp: &RelyingParty, auth: &AuthData) -> Result<(), WebAuthnError> {
    if !forge_secrets::crypto::constant_time_eq(&auth.rp_id_hash, &sha256(rp.id.as_bytes())) {
        return Err(WebAuthnError::Mismatch("relying party"));
    }
    if auth.flags & UP == 0 {
        return Err(WebAuthnError::Mismatch("user presence"));
    }
    if auth.flags & UV == 0 {
        return Err(WebAuthnError::Mismatch("user verification (PIN or biometrics) is required"));
    }
    Ok(())
}

/// A passkey being registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCredential {
    pub credential_id: Vec<u8>,
    pub public_key: PublicKey,
    pub sign_count: u32,
}

/// Verifies a registration (`navigator.credentials.create()` response).
pub fn verify_registration(
    rp: &RelyingParty,
    challenge: &[u8],
    client_data_json: &[u8],
    attestation_object: &[u8],
) -> Result<NewCredential, WebAuthnError> {
    check_client_data(rp, client_data_json, "webauthn.create", challenge)?;
    let att = cbor::decode(attestation_object)?;
    match att.get("fmt").and_then(Value::as_text) {
        Some("none") => {}
        Some(other) => {
            return Err(WebAuthnError::Unsupported(format!("attestation format `{other}` (Forge requests `none`)")));
        }
        None => return Err(WebAuthnError::Malformed("attestation format".into())),
    }
    if !matches!(att.get("attStmt"), Some(Value::Map(m)) if m.is_empty()) {
        return Err(WebAuthnError::Malformed("`none` attestation must have an empty statement".into()));
    }
    let auth_data =
        att.get("authData").and_then(Value::as_bytes).ok_or_else(|| WebAuthnError::Malformed("authData".into()))?;
    let auth = AuthData::parse(auth_data)?;
    check_flags(rp, &auth)?;
    let (credential_id, public_key) =
        auth.credential.ok_or_else(|| WebAuthnError::Malformed("no attested credential".into()))?;
    Ok(NewCredential { credential_id, public_key, sign_count: auth.sign_count })
}

/// A stored passkey, as far as verification needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredential<'a> {
    pub credential_id: &'a [u8],
    pub public_key: &'a PublicKey,
    pub sign_count: u32,
}

/// An assertion (`navigator.credentials.get()` response).
#[derive(Debug, Clone)]
pub struct Assertion<'a> {
    pub credential_id: &'a [u8],
    pub client_data_json: &'a [u8],
    pub authenticator_data: &'a [u8],
    pub signature: &'a [u8],
}

/// Verifies an assertion against the stored credential the caller looked up
/// by `credential_id` for the expected user (and, for step-up, checked to be
/// among that user's credentials). Returns the new signature counter.
pub fn verify_assertion(
    rp: &RelyingParty,
    challenge: &[u8],
    stored: &StoredCredential<'_>,
    a: &Assertion<'_>,
) -> Result<u32, WebAuthnError> {
    if !forge_secrets::crypto::constant_time_eq(a.credential_id, stored.credential_id) {
        return Err(WebAuthnError::Mismatch("credential"));
    }
    check_client_data(rp, a.client_data_json, "webauthn.get", challenge)?;
    let auth = AuthData::parse(a.authenticator_data)?;
    if auth.credential.is_some() {
        return Err(WebAuthnError::Malformed("unexpected attested data in a sign-in".into()));
    }
    check_flags(rp, &auth)?;
    let mut msg = a.authenticator_data.to_vec();
    msg.extend(sha256(a.client_data_json));
    stored.public_key.verify(&msg, a.signature)?;
    // Counters: 0 means the authenticator does not count (synced passkeys).
    if (auth.sign_count != 0 || stored.sign_count != 0) && auth.sign_count <= stored.sign_count {
        return Err(WebAuthnError::CounterRegression);
    }
    Ok(auth.sign_count)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, Ed25519KeyPair, KeyPair};

    use super::*;

    fn rp() -> RelyingParty {
        RelyingParty::from_public_url("https://forge.example.com").unwrap()
    }

    const CHALLENGE: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn client_data(kind: &str, challenge: &[u8], origin: &str) -> Vec<u8> {
        format!(
            r#"{{"type":"{kind}","challenge":"{}","origin":"{origin}","crossOrigin":false}}"#,
            forge_secrets::crypto::b64url(challenge)
        )
        .into_bytes()
    }

    /// Minimal CBOR writer for the tests' own fixtures.
    fn head(major: u8, n: usize) -> Vec<u8> {
        let n = u64::try_from(n).unwrap();
        match n {
            0..=23 => vec![(major << 5) | u8::try_from(n).unwrap()],
            24..=255 => vec![(major << 5) | 24, u8::try_from(n).unwrap()],
            _ => {
                let mut v = vec![(major << 5) | 25];
                v.extend(u16::try_from(n).unwrap().to_be_bytes());
                v
            }
        }
    }
    fn bstr(b: &[u8]) -> Vec<u8> {
        let mut v = head(2, b.len());
        v.extend(b);
        v
    }
    fn tstr(s: &str) -> Vec<u8> {
        let mut v = head(3, s.len());
        v.extend(s.as_bytes());
        v
    }
    fn int(i: i64) -> Vec<u8> {
        if i >= 0 { head(0, usize::try_from(i).unwrap()) } else { head(1, usize::try_from(-1 - i).unwrap()) }
    }

    fn es256_cose(public: &[u8]) -> Vec<u8> {
        let mut m = head(5, 5);
        for (k, v) in [(1, int(2)), (3, int(-7)), (-1, int(1)), (-2, bstr(&public[1..33])), (-3, bstr(&public[33..65]))]
        {
            m.extend(int(k));
            m.extend(v);
        }
        m
    }

    fn ed25519_cose(public: &[u8]) -> Vec<u8> {
        let mut m = head(5, 4);
        for (k, v) in [(1, int(1)), (3, int(-8)), (-1, int(6)), (-2, bstr(public))] {
            m.extend(int(k));
            m.extend(v);
        }
        m
    }

    fn auth_data(rp_id: &str, flags: u8, count: u32, cred: Option<(&[u8], &[u8])>) -> Vec<u8> {
        let mut d = sha256(rp_id.as_bytes());
        d.push(flags);
        d.extend(count.to_be_bytes());
        if let Some((id, cose)) = cred {
            d.extend([0u8; 16]);
            d.extend(u16::try_from(id.len()).unwrap().to_be_bytes());
            d.extend(id);
            d.extend(cose);
        }
        d
    }

    fn attestation(auth: &[u8]) -> Vec<u8> {
        let mut m = head(5, 3);
        m.extend(tstr("fmt"));
        m.extend(tstr("none"));
        m.extend(tstr("attStmt"));
        m.extend(head(5, 0));
        m.extend(tstr("authData"));
        m.extend(bstr(auth));
        m
    }

    struct Es {
        key: EcdsaKeyPair,
    }
    impl Es {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            Self {
                key: EcdsaKeyPair::from_pkcs8(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap(),
            }
        }
        fn sign(&self, auth: &[u8], cd: &[u8]) -> Vec<u8> {
            let mut msg = auth.to_vec();
            msg.extend(sha256(cd));
            self.key.sign(&SystemRandom::new(), &msg).unwrap().as_ref().to_vec()
        }
    }

    fn register_es() -> (Es, NewCredential) {
        let es = Es::new();
        let cose = es256_cose(es.key.public_key().as_ref());
        let auth = auth_data("forge.example.com", UP | UV | AT, 0, Some((b"cred-1", &cose)));
        let cd = client_data("webauthn.create", CHALLENGE, "https://forge.example.com");
        let cred = verify_registration(&rp(), CHALLENGE, &cd, &attestation(&auth)).unwrap();
        (es, cred)
    }

    #[test]
    fn es256_registration_and_sign_in() {
        let (es, cred) = register_es();
        assert_eq!(cred.credential_id, b"cred-1");
        assert_eq!(cred.public_key.cose_alg(), -7);
        let auth = auth_data("forge.example.com", UP | UV, 5, None);
        let cd = client_data("webauthn.get", CHALLENGE, "https://forge.example.com");
        let sig = es.sign(&auth, &cd);
        let stored = StoredCredential { credential_id: b"cred-1", public_key: &cred.public_key, sign_count: 0 };
        let a =
            Assertion { credential_id: b"cred-1", client_data_json: &cd, authenticator_data: &auth, signature: &sig };
        assert_eq!(verify_assertion(&rp(), CHALLENGE, &stored, &a).unwrap(), 5);
    }

    #[test]
    fn eddsa_registration_and_sign_in() {
        let rng = SystemRandom::new();
        let kp = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&rng).unwrap().as_ref()).unwrap();
        let cose = ed25519_cose(kp.public_key().as_ref());
        let auth = auth_data("forge.example.com", UP | UV | AT, 0, Some((b"ed", &cose)));
        let cd = client_data("webauthn.create", CHALLENGE, "https://forge.example.com");
        let cred = verify_registration(&rp(), CHALLENGE, &cd, &attestation(&auth)).unwrap();
        let auth = auth_data("forge.example.com", UP | UV, 0, None);
        let cd = client_data("webauthn.get", CHALLENGE, "https://forge.example.com");
        let mut msg = auth.clone();
        msg.extend(sha256(&cd));
        let sig = kp.sign(&msg).as_ref().to_vec();
        let stored = StoredCredential { credential_id: b"ed", public_key: &cred.public_key, sign_count: 0 };
        let a = Assertion { credential_id: b"ed", client_data_json: &cd, authenticator_data: &auth, signature: &sig };
        assert_eq!(verify_assertion(&rp(), CHALLENGE, &stored, &a).unwrap(), 0, "synced passkeys report 0");
    }

    #[test]
    fn registration_rejects_each_violation() {
        let es = Es::new();
        let cose = es256_cose(es.key.public_key().as_ref());
        let good_auth = auth_data("forge.example.com", UP | UV | AT, 0, Some((b"c", &cose)));
        let good_cd = client_data("webauthn.create", CHALLENGE, "https://forge.example.com");
        let reg = |cd: &[u8], att: &[u8]| verify_registration(&rp(), CHALLENGE, cd, att);
        assert!(reg(&good_cd, &attestation(&good_auth)).is_ok());
        // Wrong type, challenge, origin.
        assert!(
            reg(&client_data("webauthn.get", CHALLENGE, "https://forge.example.com"), &attestation(&good_auth))
                .is_err()
        );
        assert!(
            reg(&client_data("webauthn.create", b"other", "https://forge.example.com"), &attestation(&good_auth))
                .is_err()
        );
        assert!(
            reg(&client_data("webauthn.create", CHALLENGE, "https://evil.example.com"), &attestation(&good_auth))
                .is_err()
        );
        // RP id hash of another site; missing UP; missing UV; no attested data.
        assert!(
            reg(&good_cd, &attestation(&auth_data("evil.example.com", UP | UV | AT, 0, Some((b"c", &cose))))).is_err()
        );
        assert!(reg(&good_cd, &attestation(&auth_data("forge.example.com", UV | AT, 0, Some((b"c", &cose))))).is_err());
        assert!(reg(&good_cd, &attestation(&auth_data("forge.example.com", UP | AT, 0, Some((b"c", &cose))))).is_err());
        assert!(reg(&good_cd, &attestation(&auth_data("forge.example.com", UP | UV, 0, None))).is_err());
        // Other attestation formats and unsupported algorithms.
        let mut packed = attestation(&good_auth);
        let at = packed.windows(4).position(|w| w == b"none").unwrap();
        packed[at..at + 4].copy_from_slice(b"tpm ");
        assert!(reg(&good_cd, &packed).is_err());
        let mut cose_bad = head(5, 3);
        for (k, v) in [(1, int(2)), (3, int(-35)), (-1, int(2))] {
            cose_bad.extend(int(k));
            cose_bad.extend(v);
        }
        assert!(
            reg(&good_cd, &attestation(&auth_data("forge.example.com", UP | UV | AT, 0, Some((b"c", &cose_bad)))))
                .is_err()
        );
        // Trailing junk after the key.
        let mut junk = good_auth.clone();
        junk.push(0);
        assert!(reg(&good_cd, &attestation(&junk)).is_err());
    }

    #[test]
    fn sign_in_rejects_each_violation() {
        let (es, cred) = register_es();
        let stored = StoredCredential { credential_id: b"cred-1", public_key: &cred.public_key, sign_count: 7 };
        let ok_auth = auth_data("forge.example.com", UP | UV, 8, None);
        let ok_cd = client_data("webauthn.get", CHALLENGE, "https://forge.example.com");
        let check = |auth: &[u8], cd: &[u8], sig: &[u8], id: &[u8]| {
            verify_assertion(
                &rp(),
                CHALLENGE,
                &stored,
                &Assertion { credential_id: id, client_data_json: cd, authenticator_data: auth, signature: sig },
            )
        };
        assert_eq!(check(&ok_auth, &ok_cd, &es.sign(&ok_auth, &ok_cd), b"cred-1").unwrap(), 8);
        // Another credential, bad signature, signature over other data.
        assert!(check(&ok_auth, &ok_cd, &es.sign(&ok_auth, &ok_cd), b"cred-2").is_err());
        assert_eq!(check(&ok_auth, &ok_cd, b"not a signature", b"cred-1"), Err(WebAuthnError::BadSignature));
        let other_cd = client_data("webauthn.get", CHALLENGE, "https://forge.example.com:8443");
        assert!(check(&ok_auth, &other_cd, &es.sign(&ok_auth, &other_cd), b"cred-1").is_err(), "origin");
        // Counter regression (clone detection).
        let old = auth_data("forge.example.com", UP | UV, 7, None);
        assert_eq!(check(&old, &ok_cd, &es.sign(&old, &ok_cd), b"cred-1"), Err(WebAuthnError::CounterRegression));
        // UV missing; replayed registration type.
        let no_uv = auth_data("forge.example.com", UP, 9, None);
        assert!(check(&no_uv, &ok_cd, &es.sign(&no_uv, &ok_cd), b"cred-1").is_err());
        let create_cd = client_data("webauthn.create", CHALLENGE, "https://forge.example.com");
        assert!(check(&ok_auth, &create_cd, &es.sign(&ok_auth, &create_cd), b"cred-1").is_err());
    }

    #[test]
    fn public_keys_round_trip_through_storage() {
        let (_, cred) = register_es();
        let json = serde_json::to_string(&cred.public_key).unwrap();
        assert!(json.contains("\"alg\":\"es256\""));
        assert_eq!(serde_json::from_str::<PublicKey>(&json).unwrap(), cred.public_key);
    }

    #[test]
    fn malformed_authenticator_data_never_panics() {
        let mut s: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..20_000 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let len = usize::try_from(s % 120).unwrap();
            let mut buf: Vec<u8> = (0..len).map(|i| u8::try_from((s >> (i % 56)) & 0xff).unwrap()).collect();
            if buf.len() > 32 {
                buf[32] = u8::try_from(s & 0xff).unwrap() | AT;
            }
            let _ = AuthData::parse(&buf);
        }
    }
}
