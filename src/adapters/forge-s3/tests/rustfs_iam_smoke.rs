//! Creates a prefix-limited key in a real RustFS and prints its credential
//! for an external S3 client check. Ignored by default:
//! `FORGE_SMOKE_S3=http://127.0.0.1:19000 FORGE_SMOKE_AK=… FORGE_SMOKE_SK=…
//! cargo test -p forge-s3 --test rustfs_iam_smoke -- --ignored --nocapture`.
#![allow(clippy::unwrap_used)] // reason: test

use forge_core::storage::BucketAccess;
use forge_core::storage_iam::{KeyGrant, StoreIam, grants_of, normalize_grants, policy_for};
use forge_s3::{Client, S3Config};

#[tokio::test]
#[ignore = "needs a running RustFS"]
async fn a_prefix_grant_round_trips_through_rustfs() {
    let (Ok(ep), Ok(ak), Ok(sk)) =
        (std::env::var("FORGE_SMOKE_S3"), std::env::var("FORGE_SMOKE_AK"), std::env::var("FORGE_SMOKE_SK"))
    else {
        return;
    };

    let c = Client::new(
        S3Config {
            endpoint: ep.parse().unwrap(),
            region: "us-east-1".into(),
            access_key: ak,
            secret_key: sk,
            path_style: true,
        },
        reqwest::Client::new(),
    )
    .unwrap();
    let _ = c.create_bucket("smoke").await;
    let grants = normalize_grants(&[KeyGrant {
        bucket: "smoke".into(),
        prefix: "team-a/".into(),
        access: BucketAccess::ReadWrite,
    }])
    .unwrap();
    let (key, secret) = c.create_key("smoke-prefix", "smoke test", &policy_for(&grants)).await.unwrap();
    let back = c.key_policy(&key).await.unwrap().unwrap();
    assert_eq!(grants_of(&back), Some(grants.clone()), "RustFS returns the policy Forge wrote");
    assert!(c.list_keys().await.unwrap().iter().any(|k| k.access_key == key));
    println!("SMOKE_KEY={key}\nSMOKE_SECRET={secret}");
}
