//! Minimal S3 client for the operations Forge exposes: buckets and objects
//! on RustFS, Garage, AWS S3 and other S3-compatible services.
//!
//! Requests are signed with AWS Signature Version 4 using `ring` (the
//! workspace's one crypto provider). Object bodies are streamed in both
//! directions and sent with `UNSIGNED-PAYLOAD`, which every supported
//! provider accepts. Errors never contain the secret key or a signature.

mod sigv4;
mod xml;

use std::fmt;
use std::time::SystemTime;

use reqwest::{Method, StatusCode};
use url::Url;

pub use xml::{Bucket, ObjectEntry, ObjectList};

/// Where a store answers and how to sign for it.
#[derive(Clone)]
pub struct S3Config {
    /// `https://s3.eu-west-1.amazonaws.com`, `http://10.0.0.5:9000`...
    pub endpoint: Url,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// `endpoint/bucket/key` (self-hosted stores) instead of
    /// `bucket.endpoint/key` (AWS default).
    pub path_style: bool,
}

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint.as_str())
            .field("region", &self.region)
            .field("access_key", &self.access_key)
            .field("secret_key", &"***")
            .field("path_style", &self.path_style)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("cannot reach the store: {0}")]
    Transport(String),
    /// The service answered with an error (`code` is the S3 error code).
    #[error("{code}: {message}")]
    Service { status: u16, code: String, message: String },
}

impl S3Error {
    /// HTTP status the service returned, if it answered.
    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        match self {
            Self::Service { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type S3Result<T> = Result<T, S3Error>;

/// Bucket names: 3-63 of `[a-z0-9.-]`, starting and ending with a letter or
/// digit, no `..` (the rules all supported providers share).
#[must_use]
pub fn valid_bucket(name: &str) -> bool {
    let b = name.as_bytes();
    (3..=63).contains(&b.len())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-'))
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && !name.contains("..")
}

/// Object keys: 1-1024 bytes of UTF-8 without control characters and
/// without `.` or `..` segments. URL parsing resolves dot segments, so a key
/// like `../admin/x` would otherwise be signed and sent as a request to
/// another path on the endpoint (such as an admin API) with the store's
/// credentials.
#[must_use]
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 1024
        && !key.chars().any(char::is_control)
        && !key.split('/').any(|seg| seg == "." || seg == "..")
}

/// Metadata of one object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHead {
    pub size: u64,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Client {
    cfg: S3Config,
    http: reqwest::Client,
}

impl Client {
    pub fn new(cfg: S3Config, http: reqwest::Client) -> S3Result<Self> {
        if !matches!(cfg.endpoint.scheme(), "http" | "https") || cfg.endpoint.host_str().is_none() {
            return Err(S3Error::Invalid("endpoint must be http(s)://host[:port]".into()));
        }
        if cfg.region.is_empty() || cfg.access_key.is_empty() || cfg.secret_key.is_empty() {
            return Err(S3Error::Invalid("region, access key and secret key are required".into()));
        }
        Ok(Self { cfg, http })
    }

    /// The URL for a bucket and key (either may be empty).
    fn url(&self, bucket: &str, key: &str, query: &[(&str, &str)]) -> S3Result<Url> {
        let mut url = self.cfg.endpoint.clone();
        let base = url.path().trim_end_matches('/').to_owned();
        let mut path = base;
        if !bucket.is_empty() {
            if self.cfg.path_style {
                path.push('/');
                path.push_str(bucket);
            } else {
                let host = url.host_str().unwrap_or_default().to_owned();
                url.set_host(Some(&format!("{bucket}.{host}"))).map_err(|e| S3Error::Invalid(e.to_string()))?;
            }
        }
        if !key.is_empty() {
            path.push('/');
            path.push_str(&sigv4::encode(key, false));
        }
        if path.is_empty() {
            path.push('/');
        }
        // Already encoded: set without re-encoding.
        url.set_path(&path);
        // The parser must not have changed the path (dot segments, also in
        // percent-encoded form): a request is only ever signed for, and sent
        // to, the bucket and key asked for.
        if url.path() != path {
            return Err(S3Error::Invalid("object key: not a valid path".into()));
        }
        let q = sigv4::canonical_query(query);
        url.set_query(if q.is_empty() { None } else { Some(&q) });
        Ok(url)
    }

    fn request(&self, method: Method, url: Url, payload_hash: &str) -> reqwest::RequestBuilder {
        let headers = sigv4::sign(
            &sigv4::Request { method: method.as_str(), url: &url, payload_hash },
            &self.cfg.region,
            &self.cfg.access_key,
            &self.cfg.secret_key,
            SystemTime::now(),
        );
        let mut rb = self.http.request(method, url);
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        rb
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> S3Result<reqwest::Response> {
        let resp = rb.send().await.map_err(|e| S3Error::Transport(transport_message(&e)))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(service_error(resp).await)
    }

    async fn send_empty(&self, method: Method, bucket: &str, key: &str, query: &[(&str, &str)]) -> S3Result<String> {
        let url = self.url(bucket, key, query)?;
        let resp = self.send(self.request(method, url, sigv4::EMPTY_SHA256)).await?;
        read_capped(resp, MAX_RESPONSE).await
    }

    /// Lists buckets (also a connectivity and credential check).
    pub async fn list_buckets(&self) -> S3Result<Vec<Bucket>> {
        let body = self.send_empty(Method::GET, "", "", &[]).await?;
        xml::buckets(&body)
    }

    pub async fn create_bucket(&self, bucket: &str) -> S3Result<()> {
        if !valid_bucket(bucket) {
            return Err(S3Error::Invalid(format!("bucket name `{bucket}`")));
        }
        // AWS needs the region outside us-east-1; others ignore or accept it.
        let body = if self.cfg.region == "us-east-1" || self.cfg.path_style {
            String::new()
        } else {
            format!(
                "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>",
                self.cfg.region
            )
        };
        let url = self.url(bucket, "", &[])?;
        let hash = sigv4::sha256_hex(body.as_bytes());
        self.send(self.request(Method::PUT, url, &hash).body(body)).await?;
        Ok(())
    }

    /// Deletes an empty bucket.
    pub async fn delete_bucket(&self, bucket: &str) -> S3Result<()> {
        if !valid_bucket(bucket) {
            return Err(S3Error::Invalid(format!("bucket name `{bucket}`")));
        }
        self.send_empty(Method::DELETE, bucket, "", &[]).await.map(|_| ())
    }

    /// One page of objects under `prefix`, grouped at `/` like folders.
    pub async fn list_objects(
        &self,
        bucket: &str,
        prefix: &str,
        token: Option<&str>,
        max_keys: u32,
    ) -> S3Result<ObjectList> {
        if !valid_bucket(bucket) {
            return Err(S3Error::Invalid(format!("bucket name `{bucket}`")));
        }
        let max = max_keys.clamp(1, 1000).to_string();
        let mut q = vec![("list-type", "2"), ("delimiter", "/"), ("max-keys", max.as_str()), ("prefix", prefix)];
        if let Some(t) = token {
            q.push(("continuation-token", t));
        }
        let body = self.send_empty(Method::GET, bucket, "", &q).await?;
        xml::objects(&body)
    }

    pub async fn head_object(&self, bucket: &str, key: &str) -> S3Result<ObjectHead> {
        check(bucket, key)?;
        let url = self.url(bucket, key, &[])?;
        let resp = self.send(self.request(Method::HEAD, url, sigv4::EMPTY_SHA256)).await?;
        Ok(head(&resp))
    }

    /// Starts a download; the caller streams the response body.
    pub async fn get_object(&self, bucket: &str, key: &str) -> S3Result<(ObjectHead, reqwest::Response)> {
        check(bucket, key)?;
        let url = self.url(bucket, key, &[])?;
        let resp = self.send(self.request(Method::GET, url, sigv4::EMPTY_SHA256)).await?;
        Ok((head(&resp), resp))
    }

    /// Uploads a body of exactly `len` bytes (streamed, unsigned payload).
    pub async fn put_object(
        &self,
        bucket: &str,
        key: &str,
        body: reqwest::Body,
        len: u64,
        content_type: &str,
    ) -> S3Result<()> {
        check(bucket, key)?;
        let url = self.url(bucket, key, &[])?;
        let rb = self
            .request(Method::PUT, url, sigv4::UNSIGNED)
            .header(reqwest::header::CONTENT_LENGTH, len)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body);
        self.send(rb).await.map(|_| ())
    }

    pub async fn delete_object(&self, bucket: &str, key: &str) -> S3Result<()> {
        check(bucket, key)?;
        self.send_empty(Method::DELETE, bucket, key, &[]).await.map(|_| ())
    }

    // ------------------------------------------------------------ RustFS admin
    //
    // RustFS's native admin API (`/rustfs/admin/v3/…`) takes and returns
    // plain JSON signed with SigV4; only the MinIO-compatible prefix encrypts
    // bodies, so Forge uses the native one.

    async fn admin(&self, method: Method, op: &str, query: &[(&str, &str)], body: Option<String>) -> S3Result<String> {
        let mut url = self.cfg.endpoint.clone();
        let base = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base}/rustfs/admin/v3/{op}"));
        let q = sigv4::canonical_query(query);
        url.set_query(if q.is_empty() { None } else { Some(&q) });
        let body = body.unwrap_or_default();
        let hash = sigv4::sha256_hex(body.as_bytes());
        let mut rb = self.request(method, url, &hash);
        if !body.is_empty() {
            rb = rb.header(reqwest::header::CONTENT_TYPE, "application/json").body(body);
        }
        let resp = self.send(rb).await?;
        read_capped(resp, MAX_RESPONSE).await
    }

    /// Creates an access key (RustFS service account of the signing user)
    /// limited by `policy` (IAM JSON). Returns `(access_key, secret_key)`.
    pub async fn add_service_account(
        &self,
        name: &str,
        description: &str,
        policy: &serde_json::Value,
    ) -> S3Result<(String, String)> {
        let body = serde_json::json!({ "name": name, "description": description, "policy": policy });
        let text = self.admin(Method::PUT, "add-service-accounts", &[], Some(body.to_string())).await?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| S3Error::Transport("unexpected admin response".into()))?;
        let c = &v["credentials"];
        match (c["accessKey"].as_str(), c["secretKey"].as_str()) {
            (Some(a), Some(s)) if !a.is_empty() && !s.is_empty() => Ok((a.to_owned(), s.to_owned())),
            _ => Err(S3Error::Transport("the store did not return the new key".into())),
        }
    }

    /// Access keys of the signing user: `[{accessKey, name, description,
    /// accountStatus, expiration}]` as the store reports them.
    pub async fn list_service_accounts(&self) -> S3Result<Vec<serde_json::Value>> {
        let text = self.admin(Method::GET, "list-service-accounts", &[], None).await?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| S3Error::Transport("unexpected admin response".into()))?;
        Ok(v.get("accounts").and_then(serde_json::Value::as_array).cloned().unwrap_or_default())
    }

    pub async fn delete_service_account(&self, access_key: &str) -> S3Result<()> {
        if access_key.is_empty() || access_key.len() > 128 || !access_key.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(S3Error::Invalid("access key".into()));
        }
        self.admin(Method::DELETE, "delete-service-accounts", &[("accessKey", access_key)], None).await.map(|_| ())
    }
}

/// IAM policy limiting a key to one bucket: read-only or read-write.
#[must_use]
pub fn bucket_policy(bucket: &str, write: bool) -> serde_json::Value {
    let mut object = vec!["s3:GetObject"];
    if write {
        object.extend(["s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"]);
    }
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": ["s3:ListBucket", "s3:GetBucketLocation", "s3:ListBucketMultipartUploads"],
              "Resource": [format!("arn:aws:s3:::{bucket}")] },
            { "Effect": "Allow", "Action": object, "Resource": [format!("arn:aws:s3:::{bucket}/*")] }
        ]
    })
}

fn check(bucket: &str, key: &str) -> S3Result<()> {
    if !valid_bucket(bucket) {
        return Err(S3Error::Invalid(format!("bucket name `{bucket}`")));
    }
    if !valid_key(key) {
        return Err(S3Error::Invalid("object key: 1-1024 bytes, no control characters".into()));
    }
    Ok(())
}

fn head(resp: &reqwest::Response) -> ObjectHead {
    let h = |n: reqwest::header::HeaderName| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_owned);
    ObjectHead {
        size: resp.content_length().unwrap_or_default(),
        content_type: h(reqwest::header::CONTENT_TYPE),
        etag: h(reqwest::header::ETAG),
        last_modified: h(reqwest::header::LAST_MODIFIED),
    }
}

/// Transport errors without the URL's query (it never holds secrets here,
/// but keeps messages short).
fn transport_message(e: &reqwest::Error) -> String {
    let host = e.url().map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or_default()));
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "connection failed"
    } else if e.is_body() || e.is_decode() {
        "response body error"
    } else {
        "request failed"
    };
    let mut detail = String::new();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        detail = s.to_string();
        src = s.source();
    }
    let detail = if detail.is_empty() { String::new() } else { format!(" ({detail})") };
    match host {
        Some(h) => format!("{h}: {kind}{detail}"),
        None => format!("{kind}{detail}"),
    }
}

/// Largest list or admin response read into memory (a listing page of 1,000
/// keys is a few hundred KiB).
const MAX_RESPONSE: usize = 16 * 1024 * 1024;
/// Largest error body read (S3 error documents are tiny).
const MAX_ERROR_BODY: usize = 64 * 1024;

/// Reads a response body as text, failing once it exceeds `cap` bytes, so a
/// broken or hostile endpoint cannot exhaust the control plane's memory.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> S3Result<String> {
    if resp.content_length().is_some_and(|l| l > cap as u64) {
        return Err(S3Error::Transport("the store's response is too large".into()));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| S3Error::Transport(transport_message(&e)))? {
        if buf.len() + chunk.len() > cap {
            return Err(S3Error::Transport("the store's response is too large".into()));
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|_| S3Error::Transport("the store's response is not UTF-8".into()))
}

async fn service_error(resp: reqwest::Response) -> S3Error {
    let status = resp.status();
    let body = read_capped(resp, MAX_ERROR_BODY).await.unwrap_or_default();
    let (code, message) = xml::error(&body).unwrap_or_else(|| {
        let code = match status {
            StatusCode::NOT_FOUND => "NoSuchKey",
            StatusCode::FORBIDDEN => "AccessDenied",
            _ => "Error",
        };
        (code.to_owned(), status.canonical_reason().unwrap_or("request failed").to_owned())
    });
    S3Error::Service { status: status.as_u16(), code, message }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Security review: `../rustfs/admin/v3/...` as an object key resolved to
    /// a root-signed request on the store's admin API.
    #[test]
    fn keys_cannot_leave_the_bucket() {
        for key in ["../rustfs/admin/v3/list-service-accounts", "a/../../x", "./a", "a/.", "..", "a/.."] {
            assert!(!valid_key(key), "{key} must be refused");
        }
        for key in ["a..b", "..a/b", "dir/.hidden", "a/b...", "x/y.tar.gz"] {
            assert!(valid_key(key), "{key} is a normal key");
        }
        let virtual_host = Client::new(
            S3Config {
                endpoint: Url::parse("https://s3.example.com").unwrap(),
                region: "us-east-1".into(),
                access_key: "ak".into(),
                secret_key: "sk".into(),
                path_style: false,
            },
            reqwest::Client::new(),
        )
        .unwrap();
        for c in [client(true), virtual_host] {
            assert!(c.url("bucket", "../rustfs/admin/v3/info", &[]).is_err());
            let encoded = c.url("bucket", "a/%2e%2e/x", &[]).map(|u| u.path().to_owned());
            assert!(encoded.as_ref().is_ok_and(|p| p.contains("%252")), "percent signs are encoded: {encoded:?}");
            let u = c.url("bucket", "dir/file.txt", &[]).unwrap();
            assert!(u.path().ends_with("/dir/file.txt"));
        }
    }

    /// A one-shot HTTP server that answers every request with `body_len`
    /// bytes, chunked (no Content-Length).
    async fn huge_server(body_len: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut req = [0u8; 4096];
                let _ = s.read(&mut req).await;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").await;
                let chunk = vec![b'a'; 64 * 1024];
                let mut sent = 0;
                while sent < body_len {
                    let head = format!("{:x}\r\n", chunk.len());
                    if s.write_all(head.as_bytes()).await.is_err()
                        || s.write_all(&chunk).await.is_err()
                        || s.write_all(b"\r\n").await.is_err()
                    {
                        break;
                    }
                    sent += chunk.len();
                }
                let _ = s.write_all(b"0\r\n\r\n").await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn oversized_responses_are_refused() {
        let ep = huge_server(MAX_RESPONSE + 128 * 1024).await;
        let c = Client::new(
            S3Config {
                endpoint: Url::parse(&ep).unwrap(),
                region: "us-east-1".into(),
                access_key: "ak".into(),
                secret_key: "sk".into(),
                path_style: true,
            },
            reqwest::Client::new(),
        )
        .unwrap();
        let err = c.list_buckets().await.unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    fn client(path_style: bool) -> Client {
        Client::new(
            S3Config {
                endpoint: Url::parse("http://10.0.0.5:9000").unwrap(),
                region: "us-east-1".into(),
                access_key: "ak".into(),
                secret_key: "sk".into(),
                path_style,
            },
            reqwest::Client::new(),
        )
        .unwrap()
    }

    #[test]
    fn bucket_and_key_rules() {
        assert!(valid_bucket("media-01"));
        assert!(valid_bucket("a.b.c"));
        for bad in ["ab", "Media", "-a1", "a1-", "a..b", "a_b", &"a".repeat(64)] {
            assert!(!valid_bucket(bad), "{bad}");
        }
        assert!(valid_key("photos/2026/a b.jpg"));
        assert!(!valid_key(""));
        assert!(!valid_key("a\nb"));
    }

    #[test]
    fn builds_path_and_virtual_host_urls() {
        let u = client(true).url("media", "a b/ü.txt", &[]).unwrap();
        assert_eq!(u.as_str(), "http://10.0.0.5:9000/media/a%20b/%C3%BC.txt");
        let mut c = client(false);
        c.cfg.endpoint = Url::parse("https://s3.eu-west-1.amazonaws.com").unwrap();
        let u = c.url("media", "k", &[("list-type", "2")]).unwrap();
        assert_eq!(u.as_str(), "https://media.s3.eu-west-1.amazonaws.com/k?list-type=2");
        let u = client(true).url("", "", &[]).unwrap();
        assert_eq!(u.as_str(), "http://10.0.0.5:9000/");
    }

    #[test]
    fn bucket_policies_are_scoped() {
        let ro = bucket_policy("media", false);
        let s = ro.to_string();
        assert!(s.contains("arn:aws:s3:::media\"") && s.contains("arn:aws:s3:::media/*"));
        assert!(!s.contains("PutObject") && !s.contains("arn:aws:s3:::*"));
        assert!(bucket_policy("media", true).to_string().contains("s3:PutObject"));
    }

    #[test]
    fn debug_hides_the_secret() {
        let c = client(true);
        assert!(!format!("{:?}", c.cfg).contains("\"sk\""));
    }
}
