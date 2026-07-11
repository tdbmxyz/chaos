//! User and session persistence (see migrations/0003_auth_calendar.sql).
//!
//! Passwords are stored as argon2id PHC strings, sessions as sha256 hashes
//! of the opaque token. Hashing/verification itself lives in `auth.rs`;
//! this module only moves rows.

use chaos_domain::User;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::{Db, DbError, Result};

impl Db {
    pub async fn create_user(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<User> {
        let username = username.trim().to_lowercase();
        if username.is_empty() {
            return Err(DbError::Constraint("username cannot be empty".into()));
        }
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(&username)
        .bind(display_name.trim())
        .bind(password_hash)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|err| match &err {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                DbError::Constraint(format!("user {username:?} already exists"))
            }
            _ => err.into(),
        })?;
        self.user_by_id(id).await
    }

    pub async fn user_by_id(&self, id: Uuid) -> Result<User> {
        let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(DbError::NotFound)?;
        row.try_into()
    }

    pub async fn user_by_username(&self, username: &str) -> Result<User> {
        // Case folding happens in Rust: SQLite's NOCASE is ASCII-only and
        // create_user stores Unicode-lowercased names. The collation stays
        // only to cover rows that predate that normalization.
        let row =
            sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE username = ? COLLATE NOCASE")
                .bind(username.trim().to_lowercase())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(DbError::NotFound)?;
        row.try_into()
    }

    /// User plus stored password hash, for login verification.
    /// Folds case in Rust like `user_by_username` (NOCASE is ASCII-only).
    pub async fn user_with_password(&self, username: &str) -> Result<(User, String)> {
        let row =
            sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE username = ? COLLATE NOCASE")
                .bind(username.trim().to_lowercase())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(DbError::NotFound)?;
        let hash = row.password_hash.clone();
        Ok((row.try_into()?, hash))
    }

    pub async fn create_session(
        &self,
        token_hash: &str,
        user_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, expires_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(user_id.to_string())
        .bind(Utc::now())
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        // Opportunistic cleanup; logins are rare enough that this is free.
        sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
            .bind(Utc::now())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Resolve a session token hash to its (non-expired) user.
    pub async fn user_by_session(&self, token_hash: &str) -> Result<User> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT u.* FROM users u
             JOIN sessions s ON s.user_id = u.id
             WHERE s.token_hash = ? AND s.expires_at >= ?",
        )
        .bind(token_hash)
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(DbError::NotFound)?;
        row.try_into()
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>("SELECT * FROM users ORDER BY username")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(UserRow::try_into).collect()
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    username: String,
    display_name: String,
    password_hash: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<UserRow> for User {
    type Error = DbError;

    fn try_from(row: UserRow) -> Result<User> {
        Ok(User {
            id: parse_uuid(&row.id)?,
            username: row.username,
            display_name: row.display_name,
            created_at: row.created_at,
        })
    }
}

pub(crate) fn parse_uuid(s: &str) -> Result<Uuid> {
    s.parse()
        .map_err(|_| DbError::Corrupt(format!("invalid uuid {s:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn user_and_session_lifecycle() {
        let db = Db::in_memory().await.expect("db");
        let user = db
            .create_user("Tibo", "Tibo", "phc-string")
            .await
            .expect("user");
        assert_eq!(user.username, "tibo", "usernames normalize to lowercase");

        // Duplicate (case-insensitive) is refused.
        assert!(matches!(
            db.create_user("tibo", "Dup", "x").await,
            Err(DbError::Constraint(_))
        ));

        let (found, hash) = db.user_with_password("TIBO").await.expect("lookup");
        assert_eq!(found.id, user.id);
        assert_eq!(hash, "phc-string");

        db.create_session("hash1", user.id, Utc::now() + chrono::Duration::days(1))
            .await
            .expect("session");
        assert_eq!(
            db.user_by_session("hash1").await.expect("resolve").id,
            user.id
        );

        // Expired sessions do not resolve.
        db.create_session("hash2", user.id, Utc::now() - chrono::Duration::hours(1))
            .await
            .expect("expired session");
        assert!(matches!(
            db.user_by_session("hash2").await,
            Err(DbError::NotFound)
        ));

        db.delete_session("hash1").await.expect("logout");
        assert!(matches!(
            db.user_by_session("hash1").await,
            Err(DbError::NotFound)
        ));
    }

    /// SQLite's COLLATE NOCASE only folds ASCII, but create_user
    /// normalizes with Rust's Unicode to_lowercase(): "Émile" is stored
    /// as "émile". Lookups must fold in Rust too, or accented usernames
    /// can never log back in.
    #[tokio::test]
    async fn non_ascii_usernames_are_looked_up_case_insensitively() {
        let db = Db::in_memory().await.expect("db");
        let user = db
            .create_user("Émile", "Émile", "phc-string")
            .await
            .expect("user");
        assert_eq!(user.username, "émile");

        let found = db.user_by_username("ÉMILE").await.expect("lookup by name");
        assert_eq!(found.id, user.id);

        let (found, hash) = db.user_with_password("Émile").await.expect("login lookup");
        assert_eq!(found.id, user.id);
        assert_eq!(hash, "phc-string");
    }
}
