//! Ingress endpoint rules that need stored state (D-169): basic-auth users
//! (Forge keeps only crypt hashes) and static directories (inside an
//! approved file root on every edge). The API handlers in `api/sites.rs`
//! call this service; it depends only on ports.

use std::sync::Arc;

use forge_catalog::SiteSpec;
use forge_catalog::sites::BasicAuthUser;
use forge_core::files::{FilePath, FileRepository, RootSpec};
use forge_core::{DomainError, DomainResult, ServerId};
use serde_json::Value;

/// Endpoint checks over the file-root and site ports.
#[derive(Debug)]
pub struct EndpointService {
    pub files: Arc<dyn FileRepository>,
}

impl EndpointService {
    /// Every static directory must sit inside an approved directory root
    /// (a service folder or an approved directory, never the whole node or a
    /// container) on every edge, so an endpoint cannot publish `/etc`.
    pub async fn check_static_roots(&self, edges: &[ServerId], spec: &SiteSpec) -> DomainResult<()> {
        let dirs = spec.static_roots();
        if dirs.is_empty() {
            return Ok(());
        }

        let paths: Vec<FilePath> = dirs
            .iter()
            .map(|d| FilePath::parse(d).map_err(|e| DomainError::Invalid(format!("static directory `{d}`: {e}"))))
            .collect::<DomainResult<_>>()?;

        for edge in edges {
            let roots = self.files.list_roots(*edge).await?;

            for (dir, path) in dirs.iter().zip(&paths) {
                let inside = roots.iter().any(|r| match &r.spec {
                    RootSpec::Dir { .. } | RootSpec::App { .. } => {
                        let base = r.spec.base();
                        let base: Vec<&str> = base.components().iter().map(String::as_str).collect();
                        !base.is_empty() && path.starts_with(&base)
                    }
                    RootSpec::Node | RootSpec::Container { .. } => false,
                });

                if !inside {
                    return Err(DomainError::Invalid(format!(
                        "static directory `{dir}` is not inside an approved file root on node {edge} \
                         (Files → Approve a folder on that node first)"
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Hashes new basic-auth passwords and keeps the stored hash of a user sent
/// without one (the console never receives hashes). A user with neither is
/// refused by validation.
pub fn prepare_basic_auth(spec: &mut SiteSpec, stored: Option<&SiteSpec>) -> DomainResult<()> {
    let Some(auth) = spec.access.basic_auth.as_mut() else { return Ok(()) };
    let old: Vec<&BasicAuthUser> =
        stored.and_then(|s| s.access.basic_auth.as_ref()).map(|b| b.users.iter().collect()).unwrap_or_default();

    for u in &mut auth.users {
        match u.password.take().filter(|p| !p.is_empty()) {
            Some(pw) => {
                if pw.len() > 256 {
                    return Err(DomainError::Invalid("basic auth passwords are at most 256 characters".into()));
                }

                u.hash = crypt_sha512(pw.as_bytes(), &random_salt()?);
            }
            None => {
                if let Some(o) = old.iter().find(|o| o.user == u.user) {
                    u.hash.clone_from(&o.hash);
                }
            }
        }
    }

    Ok(())
}

/// Removes basic-auth hashes from a stored site config before it is sent
/// to the browser.
pub fn redact(config: &mut Value) {
    if let Some(users) = config.pointer_mut("/spec/access/basic_auth/users").and_then(Value::as_array_mut) {
        for u in users {
            if let Some(o) = u.as_object_mut() {
                o.remove("hash");
            }
        }
    }
}

const ITOA64: &[u8] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn random_salt() -> DomainResult<Vec<u8>> {
    use ring::rand::SecureRandom;
    let mut raw = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut raw).map_err(|_| DomainError::internal("no randomness"))?;

    Ok(raw.iter().map(|b| ITOA64[usize::from(b % 64)]).collect())
}

/// SHA-512 crypt (`$6$`, Drepper's specification, 5000 rounds), which
/// nginx's `auth_basic_user_file` reads through the system `crypt()`.
/// The default round count keeps the per-request cost on the edge low.
#[must_use]
pub fn crypt_sha512(pw: &[u8], salt: &[u8]) -> String {
    use ring::digest::{Context, SHA512};
    let salt = &salt[..salt.len().min(16)];
    let digest = |parts: &[&[u8]]| {
        let mut c = Context::new(&SHA512);

        for p in parts {
            c.update(p);
        }

        c.finish().as_ref().to_vec()
    };
    let b = digest(&[pw, salt, pw]);
    let mut a = Context::new(&SHA512);
    a.update(pw);
    a.update(salt);
    let mut n = pw.len();

    while n > 64 {
        a.update(&b);
        n -= 64;
    }

    a.update(&b[..n]);
    let mut n = pw.len();

    while n > 0 {
        if n & 1 == 1 {
            a.update(&b);
        } else {
            a.update(pw);
        }

        n >>= 1;
    }

    let a = a.finish().as_ref().to_vec();
    let repeat = |d: &[u8], len: usize| d.iter().copied().cycle().take(len).collect::<Vec<u8>>();

    let dp = {
        let mut c = Context::new(&SHA512);

        for _ in 0..pw.len() {
            c.update(pw);
        }

        c.finish().as_ref().to_vec()
    };

    let p = repeat(&dp, pw.len());

    let ds = {
        let mut c = Context::new(&SHA512);

        for _ in 0..16 + usize::from(a[0]) {
            c.update(salt);
        }

        c.finish().as_ref().to_vec()
    };

    let s = repeat(&ds, salt.len());
    let mut c = a;

    for i in 0..5000 {
        let mut x = Context::new(&SHA512);
        x.update(if i % 2 == 1 { &p } else { &c });

        if i % 3 != 0 {
            x.update(&s);
        }

        if i % 7 != 0 {
            x.update(&p);
        }

        x.update(if i % 2 == 1 { &c } else { &p });
        c = x.finish().as_ref().to_vec();
    }

    const ORDER: [(usize, usize, usize); 21] = [
        (0, 21, 42),
        (22, 43, 1),
        (44, 2, 23),
        (3, 24, 45),
        (25, 46, 4),
        (47, 5, 26),
        (6, 27, 48),
        (28, 49, 7),
        (50, 8, 29),
        (9, 30, 51),
        (31, 52, 10),
        (53, 11, 32),
        (12, 33, 54),
        (34, 55, 13),
        (56, 14, 35),
        (15, 36, 57),
        (37, 58, 16),
        (59, 17, 38),
        (18, 39, 60),
        (40, 61, 19),
        (62, 20, 41),
    ];
    let mut out = String::from("$6$");
    out.push_str(&String::from_utf8_lossy(salt));
    out.push('$');
    let mut b64 = |w: u32, n: usize| {
        let mut w = w;

        for _ in 0..n {
            out.push(char::from(ITOA64[(w & 0x3f) as usize]));
            w >>= 6;
        }
    };

    for (x, y, z) in ORDER {
        b64((u32::from(c[x]) << 16) | (u32::from(c[y]) << 8) | u32::from(c[z]), 4);
    }

    b64(u32::from(c[63]), 2);

    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn sha512_crypt_matches_the_reference_vectors() {
        assert_eq!(
            crypt_sha512(b"Hello world!", b"saltstring"),
            "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1"
        );
        let h = crypt_sha512(b"a much longer password than sixty-four bytes, to cover the long branch!!", b"abc");
        assert!(forge_catalog::sites::valid_crypt_hash(&h), "{h}");
    }

    #[test]
    fn passwords_become_hashes_and_stored_hashes_are_kept() {
        let mut spec: SiteSpec = serde_json::from_value(serde_json::json!({
            "name": "shop", "domains": ["shop.example.com"], "upstreams": ["10.0.0.1:80"],
            "access": { "basic_auth": { "users": [{ "user": "a", "password": "pw" }, { "user": "b" }] } },
        }))
        .unwrap();
        let mut stored = spec.clone();
        stored.access.basic_auth.as_mut().unwrap().users[1].hash = "$6$old$x".into();
        prepare_basic_auth(&mut spec, Some(&stored)).unwrap();
        let users = &spec.access.basic_auth.as_ref().unwrap().users;
        assert!(forge_catalog::sites::valid_crypt_hash(&users[0].hash));
        assert!(users[0].password.is_none());
        assert_eq!(users[1].hash, "$6$old$x");
        let mut v = serde_json::json!({ "spec": spec });
        redact(&mut v);
        assert!(!v.to_string().contains("$6$"));
    }
}
