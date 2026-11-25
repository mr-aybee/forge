//! Preparing the operational database before Forge uses it: test the
//! connection, create a named PostgreSQL database when it is missing, and
//! say whether an existing one is safe to use. Never destructive.

use std::time::Duration;

use forge_core::DomainError;
use serde::Serialize;
use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};

use crate::{DRIVERS, unknown_versions};

/// What is already in the target database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Contents {
    /// The database does not exist yet.
    Missing,
    /// It exists and has no tables.
    Empty,
    /// Forge tables this build knows: migrations will bring it up to date.
    Forge,
    /// Forge tables from a newer Forge build: refuse.
    ForgeNewer,
    /// Tables that are not Forge's: use only when the operator says so.
    Foreign,
}

/// Result of [`check`].
#[derive(Debug, Clone, Serialize)]
pub struct DbReport {
    pub engine: &'static str,
    pub server_version: Option<String>,
    pub contents: Contents,
    /// For a missing PostgreSQL database: whether the role may create it.
    pub can_create: Option<bool>,
    /// Number of tables found (0 when missing).
    pub tables: usize,
}

impl DbReport {
    /// Whether Forge may use this database. `allow_shared` accepts a
    /// database that also holds other tables.
    #[must_use]
    pub fn usable(&self, allow_shared: bool) -> bool {
        match self.contents {
            Contents::Empty | Contents::Forge => true,
            Contents::Missing => self.can_create.unwrap_or(true),
            Contents::Foreign => allow_shared,
            Contents::ForgeNewer => false,
        }
    }
}

async fn pool(url: &str) -> Result<AnyPool, DomainError> {
    DRIVERS.call_once(sqlx::any::install_default_drivers);
    AnyPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(url)
        .await
        .map_err(DomainError::internal)
}

/// `url` with its path replaced by `db` (for the maintenance database).
fn with_database(url: &str, db: &str) -> Result<String, DomainError> {
    let mut u = url::Url::parse(url).map_err(DomainError::internal)?;
    u.set_path(&format!("/{db}"));

    Ok(u.to_string())
}

fn database_of(url: &str) -> Result<String, DomainError> {
    let u = url::Url::parse(url).map_err(DomainError::internal)?;
    let name = u.path().trim_start_matches('/').to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(DomainError::Invalid("database name must be letters, digits and _".into()));
    }

    Ok(name)
}

fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

/// Inspects the database at `url` without changing anything.
pub async fn check(url: &str) -> Result<DbReport, DomainError> {
    if !is_postgres(url) {
        return check_sqlite(url).await;
    }

    let name = database_of(url)?;
    let admin = pool(&with_database(url, "postgres")?).await?;
    let version: Option<String> =
        sqlx::query("SELECT version()").fetch_one(&admin).await.ok().and_then(|r| r.try_get(0).ok());
    let exists = sqlx::query("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(&name)
        .fetch_optional(&admin)
        .await
        .map_err(DomainError::internal)?
        .is_some();

    if !exists {
        let can = sqlx::query(
            "SELECT CASE WHEN rolcreatedb OR rolsuper THEN 1 ELSE 0 END FROM pg_roles WHERE rolname = current_user",
        )
        .fetch_one(&admin)
        .await
        .map_err(DomainError::internal)?
        .try_get::<i32, _>(0)
        .map_err(DomainError::internal)?;
        admin.close().await;

        return Ok(DbReport {
            engine: "postgres",
            server_version: version,
            contents: Contents::Missing,
            can_create: Some(can == 1),
            tables: 0,
        });
    }

    admin.close().await;
    let db = pool(url).await?;
    let tables: Vec<String> = sqlx::query(
        "SELECT table_name FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'",
    )
    .fetch_all(&db)
    .await
    .map_err(DomainError::internal)?
    .iter()
    .filter_map(|r| r.try_get::<String, _>(0).ok())
    .collect();
    let contents = classify(&db, &tables).await?;
    db.close().await;

    Ok(DbReport { engine: "postgres", server_version: version, contents, can_create: None, tables: tables.len() })
}

async fn check_sqlite(url: &str) -> Result<DbReport, DomainError> {
    let path = url.trim_start_matches("sqlite://").trim_start_matches("sqlite:");
    let path = path.split('?').next().unwrap_or_default();
    let exists = tokio::fs::try_exists(path).await.unwrap_or(false);
    if !exists {
        return Ok(DbReport {
            engine: "sqlite",
            server_version: None,
            contents: Contents::Missing,
            can_create: Some(true),
            tables: 0,
        });
    }

    let db = pool(url).await?;
    let tables: Vec<String> =
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
            .fetch_all(&db)
            .await
            .map_err(DomainError::internal)?
            .iter()
            .filter_map(|r| r.try_get::<String, _>(0).ok())
            .collect();
    let contents = classify(&db, &tables).await?;
    db.close().await;

    Ok(DbReport { engine: "sqlite", server_version: None, contents, can_create: None, tables: tables.len() })
}

async fn classify(db: &AnyPool, tables: &[String]) -> Result<Contents, DomainError> {
    if tables.is_empty() {
        return Ok(Contents::Empty);
    }

    if !tables.iter().any(|t| t == "_sqlx_migrations") {
        return Ok(Contents::Foreign);
    }

    let rows =
        sqlx::query("SELECT version FROM _sqlx_migrations").fetch_all(db).await.map_err(DomainError::internal)?;
    let unknown = unknown_versions(rows.iter().filter_map(|r| r.try_get::<i64, _>("version").ok()));
    if !unknown.is_empty() {
        return Ok(Contents::ForgeNewer);
    }

    // `_sqlx_migrations` alone does not prove the database is Forge's.
    Ok(if tables.iter().any(|t| t == "workspaces") { Contents::Forge } else { Contents::Foreign })
}

/// Creates the PostgreSQL database named in `url` when it is missing.
/// Returns `true` when it was created. SQLite files are created on connect.
pub async fn create_if_missing(url: &str) -> Result<bool, DomainError> {
    if !is_postgres(url) {
        return Ok(false);
    }

    let name = database_of(url)?;
    let admin = pool(&with_database(url, "postgres")?).await?;
    let exists = sqlx::query("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(&name)
        .fetch_optional(&admin)
        .await
        .map_err(DomainError::internal)?
        .is_some();

    if exists {
        admin.close().await;
        return Ok(false);
    }

    // The name was validated by `database_of` (letters, digits, _), so
    // quoting it as an identifier is safe; CREATE DATABASE takes no bind.
    sqlx::query(&format!("CREATE DATABASE \"{name}\" ENCODING 'UTF8'"))
        .execute(&admin)
        .await
        .map_err(DomainError::internal)?;
    admin.close().await;

    Ok(true)
}
