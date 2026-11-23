use async_trait::async_trait;
use forge_core::model::{OAuthCode, Session, SessionKind, User};
use forge_core::ports::{SessionRepository, UserRepository};
use forge_core::*;
use sqlx::any::AnyRow;

use crate::row::*;
use crate::{Store, db_err};

fn user_row(r: &AnyRow) -> DomainResult<User> {
    Ok(User {
        id: parsed(r, "id")?,
        username: opt_text(r, "username")?,
        email: opt_text(r, "email")?,
        display_name: text(r, "display_name")?,
        password_hash: opt_text(r, "password_hash")?,
        totp_secret_sealed: opt_text(r, "totp_secret_sealed")?,
        oidc_subject: opt_text(r, "oidc_subject")?,
        disabled: flag(r, "disabled")?,
        created_at: ts(r, "created_at")?,
    })
}

fn session_row(r: &AnyRow) -> DomainResult<Session> {
    Ok(Session {
        id_hash: text(r, "id_hash")?,
        user_id: parsed(r, "user_id")?,
        kind: if text(r, "kind")? == "bearer" { SessionKind::Bearer } else { SessionKind::Web },
        csrf_hash: opt_text(r, "csrf_hash")?,
        refresh_hash: opt_text(r, "refresh_hash")?,
        mfa: flag(r, "mfa")?,
        created_at: ts(r, "created_at")?,
        expires_at: ts(r, "expires_at")?,
        connection_id: opt_text(r, "connection_id")?,
    })
}

#[async_trait]
impl UserRepository for Store {
    async fn create_user(&self, u: &User) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO users (id, username, email, display_name, password_hash, totp_secret_sealed, oidc_subject, disabled, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(u.id.to_string())
        .bind(u.username.as_deref().map(str::to_lowercase))
        .bind(u.email.as_deref().map(str::to_lowercase))
        .bind(&u.display_name)
        .bind(u.password_hash.clone())
        .bind(u.totp_secret_sealed.clone())
        .bind(u.oidc_subject.clone())
        .bind(b(u.disabled))
        .bind(u.created_at.0)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_user(&self, id: UserId) -> DomainResult<User> {
        let r = sqlx::query("SELECT * FROM users WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(user_row).transpose()?.ok_or_else(|| DomainError::NotFound(format!("user {id}")))
    }

    async fn find_user_by_email(&self, email: &str) -> DomainResult<Option<User>> {
        let r = sqlx::query("SELECT * FROM users WHERE email = $1")
            .bind(email.to_lowercase())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(user_row).transpose()
    }

    async fn find_user_by_username(&self, username: &str) -> DomainResult<Option<User>> {
        let r = sqlx::query("SELECT * FROM users WHERE username = $1")
            .bind(username.to_lowercase())
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(user_row).transpose()
    }

    async fn update_user_profile(
        &self,
        id: UserId,
        username: Option<&str>,
        email: Option<&str>,
        display_name: &str,
    ) -> DomainResult<()> {
        sqlx::query("UPDATE users SET username = $1, email = $2, display_name = $3 WHERE id = $4")
            .bind(username.map(str::to_lowercase))
            .bind(email.map(str::to_lowercase))
            .bind(display_name)
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn set_password_hash(&self, id: UserId, hash: &str) -> DomainResult<()> {
        sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
            .bind(hash)
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn find_user_by_oidc_subject(&self, subject: &str) -> DomainResult<Option<User>> {
        let r = sqlx::query("SELECT * FROM users WHERE oidc_subject = $1")
            .bind(subject)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(user_row).transpose()
    }

    async fn list_users(&self) -> DomainResult<Vec<User>> {
        let rows = sqlx::query("SELECT * FROM users ORDER BY COALESCE(username, email)")
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        rows.iter().map(user_row).collect()
    }

    async fn get_users(&self, ids: &[UserId]) -> DomainResult<Vec<User>> {
        let mut out = Vec::with_capacity(ids.len());
        // One statement per 200 ids keeps the placeholder count small.
        for chunk in ids.chunks(200) {
            let marks: Vec<String> = (1..=chunk.len()).map(|i| format!("${i}")).collect();
            let sql = format!("SELECT * FROM users WHERE id IN ({})", marks.join(", "));
            let mut q = sqlx::query(&sql);
            for id in chunk {
                q = q.bind(id.to_string());
            }
            let rows = q.fetch_all(self.pool()).await.map_err(db_err)?;
            for r in &rows {
                out.push(user_row(r)?);
            }
        }
        Ok(out)
    }

    async fn set_totp_secret(&self, id: UserId, sealed: Option<&str>) -> DomainResult<()> {
        sqlx::query("UPDATE users SET totp_secret_sealed = $1 WHERE id = $2")
            .bind(sealed.map(str::to_owned))
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn set_oidc_subject(&self, id: UserId, subject: &str) -> DomainResult<()> {
        sqlx::query("UPDATE users SET oidc_subject = $1 WHERE id = $2")
            .bind(subject)
            .bind(id.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }
}

#[async_trait]
impl SessionRepository for Store {
    async fn create_session(&self, s: &Session) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO sessions (id_hash, user_id, kind, csrf_hash, refresh_hash, mfa, created_at, expires_at,
                                   connection_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&s.id_hash)
        .bind(s.user_id.to_string())
        .bind(s.kind.as_str())
        .bind(s.csrf_hash.clone())
        .bind(s.refresh_hash.clone())
        .bind(b(s.mfa))
        .bind(s.created_at.0)
        .bind(s.expires_at.0)
        .bind(s.connection_id.clone())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_session(&self, id_hash: &str) -> DomainResult<Option<Session>> {
        let r = sqlx::query("SELECT * FROM sessions WHERE id_hash = $1")
            .bind(id_hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(session_row).transpose()
    }

    async fn find_by_refresh(&self, refresh_hash: &str) -> DomainResult<Option<Session>> {
        let r = sqlx::query("SELECT * FROM sessions WHERE refresh_hash = $1")
            .bind(refresh_hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.as_ref().map(session_row).transpose()
    }

    async fn delete_session(&self, id_hash: &str) -> DomainResult<()> {
        sqlx::query("DELETE FROM sessions WHERE id_hash = $1")
            .bind(id_hash)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn delete_user_sessions(&self, user: UserId) -> DomainResult<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user.to_string())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn create_oauth_code(&self, c: &OAuthCode) -> DomainResult<()> {
        sqlx::query(
            "INSERT INTO oauth_codes (code_hash, user_id, client_id, redirect_uri, code_challenge, mfa, expires_at,
                                      connection_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&c.code_hash)
        .bind(c.user_id.to_string())
        .bind(&c.client_id)
        .bind(&c.redirect_uri)
        .bind(&c.code_challenge)
        .bind(b(c.mfa))
        .bind(c.expires_at.0)
        .bind(c.connection_id.clone())
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn take_oauth_code(&self, code_hash: &str) -> DomainResult<Option<OAuthCode>> {
        let r = sqlx::query("DELETE FROM oauth_codes WHERE code_hash = $1 RETURNING *")
            .bind(code_hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
        r.map(|r| {
            Ok(OAuthCode {
                code_hash: text(&r, "code_hash")?,
                user_id: parsed(&r, "user_id")?,
                client_id: text(&r, "client_id")?,
                redirect_uri: text(&r, "redirect_uri")?,
                code_challenge: text(&r, "code_challenge")?,
                mfa: flag(&r, "mfa")?,
                expires_at: ts(&r, "expires_at")?,
                connection_id: opt_text(&r, "connection_id")?,
            })
        })
        .transpose()
    }
}
