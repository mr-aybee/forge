//! The update migration guard (`forge-server check-migrations`, D-098).

use forge_store::Store;

#[tokio::test]
async fn a_database_newer_than_the_binary_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.db");
    let url = format!("sqlite:{}?mode=rwc", path.display().to_string().replace('\\', "/"));

    // A new database: nothing applied, nothing unknown.
    assert!(Store::unknown_migrations(&url).await.unwrap().is_empty());
    let store = Store::connect(&url).await.unwrap();
    assert!(Store::unknown_migrations(&url).await.unwrap().is_empty(), "every applied migration is known");

    // A newer binary applied a migration this one does not have.
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
         VALUES (99999999, 'from the future', CURRENT_TIMESTAMP, 1, x'00', 0)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(Store::unknown_migrations(&url).await.unwrap(), vec![99_999_999]);
}

#[tokio::test]
async fn api_keys_remember_the_elevated_flag() {
    use forge_core::model::ApiKey;
    use forge_core::ports::ApiKeyRepository;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    let user = forge_core::UserId::new();
    sqlx::query("INSERT INTO users (id, display_name, disabled, created_at) VALUES ($1, 'u', 0, 0)")
        .bind(user.to_string())
        .execute(store.pool())
        .await
        .unwrap();

    for (id, elevated) in [("a", true), ("b", false)] {
        store
            .create_api_key(&ApiKey {
                id: id.into(),
                user_id: user,
                name: id.into(),
                prefix: format!("fk_{id}"),
                secret_hash: format!("hash-{id}"),
                expires_at: None,
                last_used_at: None,
                created_at: forge_core::Timestamp(0),
                elevated,
                restrictions: None,
            })
            .await
            .unwrap();
    }

    assert!(store.api_key_by_hash("hash-a").await.unwrap().unwrap().elevated);
    assert!(!store.api_key_by_hash("hash-b").await.unwrap().unwrap().elevated);
}

/// Regression for the D-100 upgrade: a database written by an older build
/// still lists the removed WireGuard actions in its builtin permission
/// profiles. The loader drops them, so the server starts instead of failing
/// with "unknown action".
#[tokio::test]
async fn profiles_that_name_retired_actions_still_load_after_the_upgrade() {
    use forge_core::permissions::PermissionRepository;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    let id = forge_core::PermissionProfileId::new();

    // Write the row as an old build would have, bypassing the strict parser.
    sqlx::query(
        "INSERT INTO permission_profiles (id, name, description, builtin, revision, created_by, created_at, updated_at)
         VALUES ($1, 'old-operator', '', 1, 1, 'system', 0, 0)",
    )
    .bind(id.to_string())
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO permission_profile_revisions (profile_id, revision, actions, created_by, created_at)
         VALUES ($1, 1, $2, 'system', 0)",
    )
    .bind(id.to_string())
    .bind(r#"["deploy","network.access","network.connect","network.manage","view"]"#)
    .execute(store.pool())
    .await
    .unwrap();

    // The loader must tolerate the retired names.
    let rev = store.get_revision(id, 1).await.unwrap().expect("revision loads");
    let names: Vec<&str> = rev.actions.iter().map(|a| a.as_str()).collect();
    assert_eq!(names, vec!["deploy", "view"]);

    // And the migration's data fix must leave valid JSON with the names gone
    // (re-run its statements on this row: the same SQL, same effect).
    for name in ["network.access", "network.connect", "network.manage"] {
        for pat in [format!("\"{name}\","), format!(",\"{name}\"")] {
            sqlx::query("UPDATE permission_profile_revisions SET actions = REPLACE(actions, $1, '')")
                .bind(pat)
                .execute(store.pool())
                .await
                .unwrap();
        }
    }

    let raw: (String,) = sqlx::query_as("SELECT actions FROM permission_profile_revisions WHERE profile_id = $1")
        .bind(id.to_string())
        .fetch_one(store.pool())
        .await
        .unwrap();
    let parsed: Vec<String> = serde_json::from_str(&raw.0).expect("still valid JSON after the REPLACEs");
    assert_eq!(parsed, vec!["deploy", "view"]);
}

/// The schema starts from one baseline migration (`0001_initial.sql`) with
/// later changes as numbered migrations after it (CLAUDE.md rule 5). They all
/// apply on a fresh database and create the whole schema, project →
/// environment → service included.
#[tokio::test]
async fn the_baseline_and_later_migrations_apply_on_a_fresh_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(store.pool())
        .await
        .unwrap();
    let expected: Vec<i64> = (1..=i64::try_from(versions.len()).unwrap()).collect();
    assert_eq!(versions, expected, "the baseline first, then one migration per number with no gaps");

    for table in
        ["projects", "environments", "services", "service_instances", "log_ingest_keys", "deployments", "user_pins"]
    {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = $1")
            .bind(table)
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(n, 1, "{table}");
    }

    let objects: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();

    // 149 from the baseline, + 5 tables and 4 indexes from 0002 (container
    // platform: swarm, registries, webhooks), + 1 table and 1 index from 0003
    // (webhook replay markers; its primary key index is internal).
    assert_eq!(objects, 160, "tables and indexes after all migrations");
}
