//! Audit search: server-side filters, keyset pagination, counts and distinct actions.
#![allow(clippy::unwrap_used)]

use forge_core::model::*;
use forge_core::ports::*;
use forge_core::*;
use forge_store::Store;
use serde_json::json;

async fn seeded() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    let actions = ["auth.login", "stack.deploy", "files.upload", "auth.logout", "stack.remove"];

    for seq in 1..=25_i64 {
        let action = actions[(seq as usize - 1) % actions.len()];
        let outcome = if seq % 4 == 0 { AuditOutcome::Denied } else { AuditOutcome::Success };
        let e = AuditEntry {
            seq,
            id: format!("e{seq}"),
            at: Timestamp(seq * 1_000),
            actor: if seq % 2 == 0 { "user:Alice".into() } else { "system:forge".into() },
            action: action.into(),
            resource: format!("server:{}", seq % 3),
            workspace_id: None,
            outcome,
            details: json!({}),
            prev_hash: "0".into(),
            hash: format!("h{seq}"),
            signature: "s".into(),
        };
        s.insert_entry(&e).await.unwrap();
    }

    // A literal `%` must not act as a wildcard.
    let odd = AuditEntry {
        seq: 26,
        id: "e26".into(),
        at: Timestamp(26_000),
        actor: "user:100%".into(),
        action: "weird_action".into(),
        resource: "platform:root".into(),
        workspace_id: None,
        outcome: AuditOutcome::Failure,
        details: json!({}),
        prev_hash: "0".into(),
        hash: "h26".into(),
        signature: "s".into(),
    };
    s.insert_entry(&odd).await.unwrap();

    (s, dir)
}

fn seqs(v: &[AuditEntry]) -> Vec<i64> {
    v.iter().map(|e| e.seq).collect()
}

#[tokio::test]
async fn filters_combine_and_match_server_side() {
    let (s, _d) = seeded().await;
    let all = s.search_entries(&AuditFilter { limit: 500, ..Default::default() }).await.unwrap();
    assert_eq!(all.len(), 26);
    assert_eq!(all[0].seq, 26, "newest first");

    // Several action prefixes are ORed.
    let f = AuditFilter { actions: vec!["auth.".into(), "files.".into()], limit: 500, ..Default::default() };
    let got = s.search_entries(&f).await.unwrap();
    assert!(got.iter().all(|e| e.action.starts_with("auth.") || e.action.starts_with("files.")));
    assert_eq!(got.len(), 15);
    assert_eq!(s.count_entries(&f).await.unwrap(), 15);

    // Date range: since inclusive, until exclusive.
    let f = AuditFilter { since: Some(5_000), until: Some(10_000), limit: 500, ..Default::default() };
    assert_eq!(seqs(&s.search_entries(&f).await.unwrap()), vec![9, 8, 7, 6, 5]);

    // Outcome, actor (case-insensitive) and resource together.
    let f = AuditFilter {
        outcome: Some(AuditOutcome::Denied),
        actor: Some("alice".into()),
        resource: Some("SERVER:0".into()),
        limit: 500,
        ..Default::default()
    };
    assert_eq!(seqs(&s.search_entries(&f).await.unwrap()), vec![24, 12]);

    // LIKE metacharacters are literal.
    let f = AuditFilter { actor: Some("100%".into()), limit: 500, ..Default::default() };
    assert_eq!(seqs(&s.search_entries(&f).await.unwrap()), vec![26]);
    let f = AuditFilter { actions: vec!["weird_".into()], limit: 500, ..Default::default() };
    assert_eq!(seqs(&s.search_entries(&f).await.unwrap()), vec![26]);
    let f = AuditFilter { actions: vec!["auth_".into()], limit: 500, ..Default::default() };
    assert!(s.search_entries(&f).await.unwrap().is_empty(), "`_` is not a wildcard");
}

#[tokio::test]
async fn keyset_pages_cover_every_entry_once() {
    let (s, _d) = seeded().await;
    let mut f = AuditFilter { actions: vec!["stack.".into()], limit: 4, ..Default::default() };
    let mut seen = Vec::new();

    loop {
        let page = s.search_entries(&f).await.unwrap();
        if page.is_empty() {
            break;
        }

        seen.extend(seqs(&page));
        f.before = page.last().map(|e| e.seq);
    }

    assert_eq!(seen, vec![25, 22, 20, 17, 15, 12, 10, 7, 5, 2]);

    // The count ignores the cursor.
    assert_eq!(s.count_entries(&f).await.unwrap(), 10);
}

#[tokio::test]
async fn distinct_actions_are_sorted_with_counts() {
    let (s, _d) = seeded().await;
    let got = s.distinct_actions(None).await.unwrap();
    let names: Vec<&str> = got.iter().map(|a| a.action.as_str()).collect();
    assert_eq!(
        names,
        vec!["auth.login", "auth.logout", "files.upload", "stack.deploy", "stack.remove", "weird_action"]
    );
    assert_eq!(got[0].count, 5);
    assert!(s.distinct_actions(Some(WorkspaceId::new())).await.unwrap().is_empty());
}

#[tokio::test]
async fn free_text_matches_action_actor_or_resource() {
    let (s, _d) = seeded().await;

    // Action.
    let f = AuditFilter { text: Some("LOGOUT".into()), limit: 500, ..Default::default() };
    assert_eq!(seqs(&s.search_entries(&f).await.unwrap()), vec![24, 19, 14, 9, 4]);

    // Actor or resource, with the other filters ANDed.
    let f = AuditFilter {
        text: Some("alice".into()),
        outcome: Some(AuditOutcome::Denied),
        limit: 500,
        ..Default::default()
    };
    assert_eq!(seqs(&s.search_entries(&f).await.unwrap()), vec![24, 20, 16, 12, 8, 4]);
    let f = AuditFilter { text: Some("platform:".into()), limit: 500, ..Default::default() };
    assert_eq!(seqs(&s.search_entries(&f).await.unwrap()), vec![26]);

    // LIKE metacharacters stay literal.
    let f = AuditFilter { text: Some("100%".into()), limit: 500, ..Default::default() };
    assert_eq!(s.count_entries(&f).await.unwrap(), 1);
}

#[tokio::test]
async fn bucket_counts_group_by_step_and_outcome() {
    let (s, _d) = seeded().await;

    // Entries are at 1 s .. 26 s; 10 s buckets from 0.
    let got = s.bucket_counts(&AuditFilter::default(), 0, 10_000).await.unwrap();
    let total: i64 = got.iter().map(|b| b.count).sum();
    assert_eq!(total, 26);
    let denied: Vec<(i64, i64)> =
        got.iter().filter(|b| b.outcome == AuditOutcome::Denied).map(|b| (b.bucket, b.count)).collect();

    // Denied: 4, 8 | 12, 16 | 20, 24.
    assert_eq!(denied, vec![(0, 2), (1, 2), (2, 2)]);
    assert!(got.iter().any(|b| b.bucket == 2 && b.outcome == AuditOutcome::Failure && b.count == 1));

    // `from` excludes earlier entries, and the other filters apply.
    let f = AuditFilter { actions: vec!["auth.".into()], ..Default::default() };
    let got = s.bucket_counts(&f, 10_000, 5_000).await.unwrap();
    let total: i64 = got.iter().map(|b| b.count).sum();
    assert_eq!(total, 6, "auth.* at 11, 14, 16, 19, 21, 24 s");
    assert!(got.iter().all(|b| (0..=3).contains(&b.bucket)));
}
