//! S3 XML responses (ListAllMyBuckets, ListObjectsV2, Error).

use serde::{Deserialize, Serialize};

use crate::{S3Error, S3Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    #[serde(rename(deserialize = "Name"))]
    pub name: String,
    #[serde(rename(deserialize = "CreationDate"), default)]
    pub created: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectEntry {
    #[serde(rename(deserialize = "Key"))]
    pub key: String,
    #[serde(rename(deserialize = "Size"), default)]
    pub size: u64,
    #[serde(rename(deserialize = "LastModified"), default)]
    pub last_modified: String,
    #[serde(rename(deserialize = "ETag"), default)]
    pub etag: String,
}

/// One page of a listing: objects and "folders" (common prefixes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ObjectList {
    pub objects: Vec<ObjectEntry>,
    pub prefixes: Vec<String>,
    /// Pass back to get the next page.
    pub next_token: Option<String>,
}

#[derive(Deserialize)]
struct ListAllMyBuckets {
    #[serde(rename = "Buckets", default)]
    buckets: BucketsWrap,
}

#[derive(Deserialize, Default)]
struct BucketsWrap {
    #[serde(rename = "Bucket", default)]
    bucket: Vec<Bucket>,
}

#[derive(Deserialize)]
struct ListBucketResult {
    #[serde(rename = "Contents", default)]
    contents: Vec<ObjectEntry>,
    #[serde(rename = "CommonPrefixes", default)]
    prefixes: Vec<Prefix>,
    #[serde(rename = "IsTruncated", default)]
    truncated: bool,
    #[serde(rename = "NextContinuationToken", default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct Prefix {
    #[serde(rename = "Prefix")]
    prefix: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(rename = "Code", default)]
    code: String,
    #[serde(rename = "Message", default)]
    message: String,
}

fn parse<'a, T: Deserialize<'a>>(body: &'a str) -> S3Result<T> {
    quick_xml::de::from_str(body).map_err(|e| S3Error::Transport(format!("unexpected response from the store: {e}")))
}

pub fn buckets(body: &str) -> S3Result<Vec<Bucket>> {
    Ok(parse::<ListAllMyBuckets>(body)?.buckets.bucket)
}

pub fn objects(body: &str) -> S3Result<ObjectList> {
    let r: ListBucketResult = parse(body)?;
    Ok(ObjectList {
        objects: r
            .contents
            .into_iter()
            .map(|mut o| {
                o.etag = o.etag.trim_matches('"').to_owned();
                o
            })
            .collect(),
        prefixes: r.prefixes.into_iter().map(|p| p.prefix).collect(),
        next_token: if r.truncated { r.next } else { None },
    })
}

/// `(code, message)` of an S3 error body.
pub fn error(body: &str) -> Option<(String, String)> {
    let e: ErrorBody = quick_xml::de::from_str(body).ok()?;
    (!e.code.is_empty()).then(|| (e.code, e.message.chars().take(300).collect()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_bucket_list() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Owner><ID>x</ID></Owner>
<Buckets><Bucket><Name>media</Name><CreationDate>2026-01-01T00:00:00.000Z</CreationDate></Bucket>
<Bucket><Name>logs</Name><CreationDate>2026-01-02T00:00:00.000Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>"#;
        let b = buckets(body).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].name, "logs");
        assert!(buckets("<ListAllMyBucketsResult><Buckets/></ListAllMyBucketsResult>").unwrap().is_empty());
    }

    #[test]
    fn parses_object_page() {
        let body = r#"<ListBucketResult><Name>media</Name><Prefix>a/</Prefix><IsTruncated>true</IsTruncated>
<NextContinuationToken>tok==</NextContinuationToken>
<Contents><Key>a/1.txt</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified><ETag>"abc"</ETag><Size>12</Size></Contents>
<CommonPrefixes><Prefix>a/sub/</Prefix></CommonPrefixes></ListBucketResult>"#;
        let l = objects(body).unwrap();
        assert_eq!(l.objects[0].size, 12);
        assert_eq!(l.objects[0].etag, "abc");
        assert_eq!(l.prefixes, vec!["a/sub/".to_owned()]);
        assert_eq!(l.next_token.as_deref(), Some("tok=="));
    }

    #[test]
    fn parses_errors() {
        let e = error("<Error><Code>NoSuchBucket</Code><Message>gone</Message></Error>").unwrap();
        assert_eq!(e.0, "NoSuchBucket");
        assert!(error("not xml").is_none());
    }
}
