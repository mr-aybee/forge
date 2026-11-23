//! AWS Signature Version 4 for S3 (header-based), with `ring`.

use std::time::{SystemTime, UNIX_EPOCH};

use ring::{digest, hmac};
use url::Url;

/// SHA-256 of an empty body.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// Payload marker for streamed bodies.
pub const UNSIGNED: &str = "UNSIGNED-PAYLOAD";

pub struct Request<'a> {
    pub method: &'a str,
    /// Final URL: path already encoded, query already canonical.
    pub url: &'a Url,
    pub payload_hash: &'a str,
}

#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(digest::digest(&digest::SHA256, data))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data).as_ref().to_vec()
}

/// URI-encodes per SigV4: unreserved characters stay, everything else is
/// `%XX`; `/` stays when `slash` is false (object key paths).
#[must_use]
pub fn encode(s: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') || (b == b'/' && !encode_slash) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Canonical query string: encoded pairs sorted by key, then value.
#[must_use]
pub fn canonical_query(pairs: &[(&str, &str)]) -> String {
    let mut enc: Vec<(String, String)> = pairs.iter().map(|(k, v)| (encode(k, true), encode(v, true))).collect();
    enc.sort();
    enc.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// `(YYYYMMDD'T'HHMMSS'Z', YYYYMMDD)` for a time.
fn amz_dates(t: SystemTime) -> (String, String) {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    let days = i64::try_from(secs / 86_400).unwrap_or_default();
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let date = format!("{y:04}{m:02}{d:02}");
    let stamp = format!("{date}T{:02}{:02}{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60);
    (stamp, date)
}

/// The `Host` header value reqwest will send for this URL.
fn host(url: &Url) -> String {
    let h = url.host_str().unwrap_or_default();
    match url.port() {
        Some(p) => format!("{h}:{p}"),
        None => h.to_owned(),
    }
}

/// Returns the headers to add: `x-amz-date`, `x-amz-content-sha256` and
/// `authorization`.
#[must_use]
pub fn sign(
    req: &Request<'_>,
    region: &str,
    access_key: &str,
    secret_key: &str,
    now: SystemTime,
) -> Vec<(&'static str, String)> {
    let (stamp, date) = amz_dates(now);
    let host = host(req.url);
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical = format!(
        "{}\n{}\n{}\nhost:{host}\nx-amz-content-sha256:{}\nx-amz-date:{stamp}\n\n{signed_headers}\n{}",
        req.method,
        req.url.path(),
        req.url.query().unwrap_or_default(),
        req.payload_hash,
        req.payload_hash
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}", sha256_hex(canonical.as_bytes()));
    let k_date = hmac_sha256(format!("AWS4{secret_key}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex::encode(hmac_sha256(&k_signing, to_sign.as_bytes()));
    vec![
        ("x-amz-date", stamp),
        ("x-amz-content-sha256", req.payload_hash.to_owned()),
        (
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
            ),
        ),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn formats_dates() {
        // 2013-05-24T00:00:00Z
        let t = UNIX_EPOCH + Duration::from_secs(1_369_353_600);
        assert_eq!(amz_dates(t), ("20130524T000000Z".into(), "20130524".into()));
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_723); // 2000-02-29T01:02:03Z
        assert_eq!(amz_dates(t).0, "20000229T010203Z");
    }

    /// The "GET Object" example from the AWS SigV4 S3 documentation, with
    /// the Range header left out of the signed headers (so only the date,
    /// scope and key derivation are compared through a known signing key).
    #[test]
    fn derives_the_documented_signing_key() {
        let k_date = hmac_sha256(b"AWS4wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", b"20130524");
        let k_region = hmac_sha256(&k_date, b"us-east-1");
        let k_service = hmac_sha256(&k_region, b"s3");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        // GET /?lifecycle example: known string to sign and signature.
        let to_sign = "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n9766c798316ff2757b517bc739a67f6213b4ab36dd5da2f94eaebf79c77395ca";
        assert_eq!(
            hex::encode(hmac_sha256(&k_signing, to_sign.as_bytes())),
            "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
    }

    #[test]
    fn documented_list_objects_request() {
        // AWS example: GET /?max-keys=2&prefix=J on examplebucket.
        let url = Url::parse("https://examplebucket.s3.amazonaws.com/?max-keys=2&prefix=J").unwrap();
        let headers = sign(
            &Request { method: "GET", url: &url, payload_hash: EMPTY_SHA256 },
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            UNIX_EPOCH + Duration::from_secs(1_369_353_600),
        );
        let auth = &headers[2].1;
        assert!(
            auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request")
        );
        assert!(auth.ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"), "{auth}");
    }

    #[test]
    fn encodes_like_sigv4() {
        assert_eq!(encode("a b/ü~", false), "a%20b/%C3%BC~");
        assert_eq!(encode("a/b", true), "a%2Fb");
        assert_eq!(canonical_query(&[("prefix", "J"), ("max-keys", "2")]), "max-keys=2&prefix=J");
    }
}
