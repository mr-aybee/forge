#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use forge_audit::{AuditLog, AuditSigner, verify_chain};
use forge_core::model::{AuditEvent, AuditOutcome};
use forge_core::{SystemClock, ports::AuditRepository};
use forge_store::Store;
use serde_json::json;

async fn setup() -> (AuditLog, Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("a.db")).await.unwrap();
    let log = AuditLog::new(Arc::new(store.clone()), Arc::new(AuditSigner::generate().unwrap()), Arc::new(SystemClock));
    (log, store, dir)
}

async fn append_n(log: &AuditLog, n: usize) {
    for i in 0..n {
        let ev = AuditEvent::new("user:alice", "workspace.create", format!("workspace:{i}"), AuditOutcome::Success)
            .with_details(json!({"name": format!("org-{i}"), "b": 1, "a": 2}));
        log.append(ev).await.unwrap();
    }
}

#[tokio::test]
async fn intact_chain_verifies() {
    let (log, _s, _d) = setup().await;
    append_n(&log, 25).await;
    let r = log.verify().await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.entries_checked, 25);
    assert!(r.head_hash.is_some());
}

#[tokio::test]
async fn modified_entry_is_detected() {
    let (log, store, _d) = setup().await;
    append_n(&log, 5).await;
    sqlx::query("UPDATE audit_log SET actor = 'user:mallory' WHERE seq = 3").execute(store.pool()).await.unwrap();
    let r = log.verify().await.unwrap();
    assert!(!r.ok);
    assert_eq!(r.first_invalid.unwrap().0, 3);
}

#[tokio::test]
async fn modified_details_are_detected() {
    let (log, store, _d) = setup().await;
    append_n(&log, 3).await;
    sqlx::query(r#"UPDATE audit_log SET details = '{"name":"evil"}' WHERE seq = 2"#)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(log.verify().await.unwrap().first_invalid.unwrap().0, 2);
}

#[tokio::test]
async fn deleted_entry_is_detected() {
    let (log, store, _d) = setup().await;
    append_n(&log, 5).await;
    sqlx::query("DELETE FROM audit_log WHERE seq = 2").execute(store.pool()).await.unwrap();
    let r = log.verify().await.unwrap();
    assert!(!r.ok);
    assert_eq!(r.first_invalid.unwrap().0, 3);
}

#[tokio::test]
async fn rehashed_forgery_without_key_is_detected() {
    // An attacker with DB access edits an entry and recomputes its hash, but
    // cannot produce a valid signature.
    let (log, store, _d) = setup().await;
    append_n(&log, 1).await;
    let mut e = store.last_entry().await.unwrap().unwrap();
    e.actor = "user:mallory".into();
    let forged = forge_audit::entry_hash(&e);
    sqlx::query("UPDATE audit_log SET actor = $1, hash = $2 WHERE seq = 1")
        .bind(&e.actor)
        .bind(&forged)
        .execute(store.pool())
        .await
        .unwrap();
    let r = log.verify().await.unwrap();
    assert_eq!(r.first_invalid.unwrap().1, "invalid signature");
}

#[tokio::test]
async fn concurrent_appends_keep_chain_linear() {
    let (log, store, _d) = setup().await;
    let log = Arc::new(log);
    let mut handles = vec![];
    for _ in 0..8 {
        let l = log.clone();
        handles.push(tokio::spawn(async move { append_n(&l, 10).await }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let r = verify_chain(&store, &log.verifier()).await.unwrap();
    assert!(r.ok);
    assert_eq!(r.entries_checked, 80);
}

#[tokio::test]
async fn histogram_counts_outcomes_per_bucket() {
    use forge_core::model::AuditFilter;
    use forge_core::{ManualClock, Timestamp};
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("a.db")).await.unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp(3_600_000)));
    let log = AuditLog::new(Arc::new(store), Arc::new(AuditSigner::generate().unwrap()), clock.clone());
    // One entry a minute for ten minutes; every third is denied.
    for i in 0..10 {
        let outcome = if i % 3 == 0 { AuditOutcome::Denied } else { AuditOutcome::Success };
        log.append(AuditEvent::new("user:alice", "auth.login", "platform:root", outcome)).await.unwrap();
        clock.advance(Duration::from_secs(60));
    }
    // All time: from the first entry to now, in round buckets.
    let h = log.histogram(&AuditFilter::default(), 60).await.unwrap();
    assert_eq!(h.from, 3_600_000);
    assert_eq!(h.to, 3_600_000 + 600_000);
    assert_eq!(h.step_ms, 10_000);
    assert_eq!(h.buckets.len(), 60);
    assert_eq!(h.buckets.iter().map(|b| b.success).sum::<i64>(), 6);
    assert_eq!(h.buckets.iter().map(|b| b.denied).sum::<i64>(), 4);
    assert_eq!(h.buckets[0].denied, 1);
    assert_eq!(h.buckets[6].success, 1);
    // A window with filters; buckets are clamped.
    let f = AuditFilter { since: Some(3_600_000 + 120_000), outcome: Some(AuditOutcome::Denied), ..Default::default() };
    let h = log.histogram(&f, 100_000).await.unwrap();
    assert!(h.buckets.len() <= forge_audit::MAX_BUCKETS as usize + 1);
    assert_eq!(h.buckets.iter().map(|b| b.denied + b.success).sum::<i64>(), 3, "denied at minutes 3, 6 and 9");
}
