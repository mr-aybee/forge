//! SQL persistence for Orbyn Forge.
//!
//! [`Store`] implements every repository port from `forge_core::ports` on a
//! `sqlx` [`AnyPool`], so the same code and migrations run on SQLite (WAL,
//! the default small profile) and PostgreSQL (feature `postgres`).
//!
//! Queries are written in the common subset of both dialects: `$n`
//! placeholders, `TEXT`/`BIGINT` columns, `ON CONFLICT`, and `RETURNING`.

mod alerts;
mod approvals;
mod audit;
mod bucket_grants;
mod certificates;
mod cluster_profiles;
mod clusters;
mod connectors;
mod deploy;
mod docker;
mod fabrics;
mod files;
mod idempotency;
mod identity;
mod installations;
mod jobs;
mod log_keys;
mod notifications;
mod object_store_iam;
mod object_stores;
mod observations;
mod passkeys;
mod permissions;
mod pins;
mod plugins;
mod projects;
pub mod provision;
mod registries;
mod resolver;
mod row;
mod secrets;
mod servers;
mod services;
mod settings;
mod silences;
mod sites;
mod ssh;
mod stacks;
mod swarm;
mod tags;
mod tenancy;
mod tunnels;
mod vault;
mod webhooks;

use std::sync::Once;
use std::time::Duration;

use forge_core::DomainError;
use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::{AnyPool, Executor};

pub use sqlx::AnyPool as Pool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
static DRIVERS: Once = Once::new();

/// `0001_initial.sql` declares `servers.cluster_id REFERENCES clusters(id)`
/// before `clusters` exists. SQLite accepts a forward reference; Postgres
/// rejects it. The file is released and must not change (its checksum is in
/// every SQLite install), so on Postgres the migrator runs this patched copy:
/// the column without the constraint, and the constraint added at the end.
/// The checksum stays the original's, so both backends record the same
/// migration history. See D-115.
const PG_0001_FORWARD_REF: &str = "cluster_id     TEXT REFERENCES clusters(id),";
const PG_0001_COLUMN: &str = "cluster_id     TEXT,";
const PG_0001_FK: &str = "\nALTER TABLE servers ADD CONSTRAINT servers_cluster_id_fkey \
                          FOREIGN KEY (cluster_id) REFERENCES clusters(id);\n";

/// The migrator for Postgres: [`MIGRATOR`] with migration 1 patched as above.
fn postgres_migrator() -> Result<sqlx::migrate::Migrator, DomainError> {
    let mut migrations = MIGRATOR.migrations.to_vec();
    let first = migrations
        .iter_mut()
        .find(|m| m.version == 1)
        .ok_or_else(|| DomainError::internal("migration 1 is missing"))?;

    if first.sql.matches(PG_0001_FORWARD_REF).count() != 1 {
        return Err(DomainError::internal("migration 1 no longer matches the Postgres patch"));
    }

    let patched = first.sql.replacen(PG_0001_FORWARD_REF, PG_0001_COLUMN, 1) + PG_0001_FK;
    first.sql = patched.into();

    Ok(sqlx::migrate::Migrator {
        migrations: migrations.into(),
        ignore_missing: MIGRATOR.ignore_missing,
        locking: MIGRATOR.locking,
        no_tx: MIGRATOR.no_tx,
    })
}

/// Handle to the database. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Store {
    pool: AnyPool,
    /// Serialises observation writes in this process. A transition takes
    /// `MAX(seq) + 1` inside its transaction; on Postgres two concurrent
    /// transactions would read the same maximum and collide on the key
    /// (SQLite serialises writers anyway).
    observation_writes: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl Store {
    /// Connects to `url` (`sqlite://path/forge.db` or `postgres://...`) and
    /// applies pending migrations.
    pub async fn connect(url: &str) -> Result<Self, DomainError> {
        DRIVERS.call_once(install_default_drivers);
        let is_sqlite = url.starts_with("sqlite:");
        let mut opts = AnyPoolOptions::new()
            .max_connections(if is_sqlite { 8 } else { 16 })
            .acquire_timeout(Duration::from_secs(10));

        if is_sqlite {
            opts = opts.after_connect(|conn, _meta| {
                Box::pin(async move {
                    conn.execute("PRAGMA journal_mode = WAL;").await?;
                    conn.execute("PRAGMA synchronous = NORMAL;").await?;
                    conn.execute("PRAGMA foreign_keys = ON;").await?;
                    conn.execute("PRAGMA busy_timeout = 5000;").await?;
                    Ok(())
                })
            });
        }

        let pool = opts.connect(url).await.map_err(DomainError::internal)?;
        let store = Self { pool, observation_writes: std::sync::Arc::default() };

        if is_sqlite {
            store.migrate().await?;
        } else {
            postgres_migrator()?.run(&store.pool).await.map_err(DomainError::internal)?;
        }

        Ok(store)
    }

    /// Opens (creating if needed) a SQLite database file.
    pub async fn open_sqlite(path: &std::path::Path) -> Result<Self, DomainError> {
        let url = format!("sqlite:{}?mode=rwc", path.display().to_string().replace('\\', "/"));
        Self::connect(&url).await
    }

    async fn migrate(&self) -> Result<(), DomainError> {
        MIGRATOR.run(&self.pool).await.map_err(DomainError::internal)
    }

    /// Versions of migrations the database at `url` has applied that this
    /// binary does not know (the database is newer than the binary).
    /// Connects without migrating, so a dry run never changes the database.
    /// A new database has none.
    pub async fn unknown_migrations(url: &str) -> Result<Vec<i64>, DomainError> {
        DRIVERS.call_once(install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect(url)
            .await
            .map_err(DomainError::internal)?;

        // No table yet: nothing applied.
        let rows = sqlx::query("SELECT version FROM _sqlx_migrations").fetch_all(&pool).await.unwrap_or_default();
        pool.close().await;

        Ok(unknown_versions(rows.iter().filter_map(|r| sqlx::Row::try_get::<i64, _>(r, "version").ok())))
    }

    /// SQLite only: a consistent copy of the database at `path` (`VACUUM
    /// INTO`), e.g. before an upgrade. Returns `false` for other databases.
    pub async fn backup_sqlite(&self, url: &str, path: &std::path::Path) -> forge_core::DomainResult<bool> {
        if !url.starts_with("sqlite:") {
            return Ok(false);
        }

        let p = path.display().to_string().replace('\\', "/").replace('\'', "''");
        sqlx::query(&format!("VACUUM INTO '{p}'")).execute(self.pool()).await.map_err(db_err)?;

        Ok(true)
    }

    #[must_use]
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Liveness probe used by `/api/health`.
    pub async fn ping(&self) -> Result<(), DomainError> {
        sqlx::query("SELECT 1").execute(&self.pool).await.map(|_| ()).map_err(DomainError::internal)
    }
}

/// The applied versions this binary has no migration for, sorted.
fn unknown_versions(applied: impl Iterator<Item = i64>) -> Vec<i64> {
    let known: Vec<i64> = MIGRATOR.iter().map(|m| m.version).collect();
    let mut out: Vec<i64> = applied.filter(|v| !known.contains(v)).collect();
    out.sort_unstable();
    out.dedup();

    out
}

/// Maps sqlx errors into domain errors (unique violations become conflicts).
pub(crate) fn db_err(e: sqlx::Error) -> DomainError {
    match &e {
        sqlx::Error::RowNotFound => DomainError::NotFound("row".into()),
        sqlx::Error::Database(d) if d.is_unique_violation() => DomainError::Conflict(d.message().to_owned()),
        _ => DomainError::Internal(e.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // tests: a panic is the failure report
mod postgres_migration_tests {
    use super::*;

    #[test]
    fn postgres_migrator_moves_the_forward_reference_and_keeps_checksums() {
        let pg = postgres_migrator().unwrap();
        assert_eq!(pg.migrations.len(), MIGRATOR.migrations.len());

        for (a, b) in pg.migrations.iter().zip(MIGRATOR.migrations.iter()) {
            assert_eq!(a.checksum, b.checksum, "migration {}", a.version);
        }

        let first = pg.migrations.iter().find(|m| m.version == 1).unwrap();
        let col = first.sql.find(PG_0001_COLUMN).unwrap();
        let clusters = first.sql.find("CREATE TABLE clusters").unwrap();
        let fk = first.sql.find("ADD CONSTRAINT servers_cluster_id_fkey").unwrap();
        assert!(col < clusters && clusters < fk);
        assert!(!first.sql.contains(PG_0001_FORWARD_REF));
    }

    /// Runs only when `FORGE_TEST_POSTGRES_URL` points at an empty database.
    #[tokio::test]
    async fn migrations_apply_on_postgres() {
        let Ok(url) = std::env::var("FORGE_TEST_POSTGRES_URL") else { return };
        Store::connect(&url).await.unwrap();

        // A second connect re-checks every checksum and applies nothing.
        Store::connect(&url).await.unwrap();
    }
}
