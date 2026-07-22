//! System-wide post ingestion timestamps: the first time a (source, post_id)
//! entered our DB. Not user-scoped.

use chrono::{DateTime, Utc};

use crate::db::{Db, Result};

// Wired into the posts_list handler in Task A5; unused until then.
#[allow(dead_code)]
impl Db {
    /// Record first-seen for each `(source, post_id)`; existing rows are left
    /// untouched so `first_seen_at` stays the earliest sighting.
    pub async fn upsert_posts(
        &self,
        items: &[(String, String, String)], // (source, post_id, title)
        now: DateTime<Utc>,
    ) -> Result<()> {
        for (source, post_id, title) in items {
            sqlx::query(
                "INSERT INTO posts (source, post_id, title, first_seen_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source, post_id) DO NOTHING",
            )
            .bind(source)
            .bind(post_id)
            .bind(title)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    impl Db {
        /// Test-only: the stored `first_seen_at` (second granularity) for a post.
        async fn post_first_seen(&self, source: &str, post_id: &str) -> Result<Option<i64>> {
            let at: Option<DateTime<Utc>> = sqlx::query_scalar(
                "SELECT first_seen_at FROM posts WHERE source = ?1 AND post_id = ?2",
            )
            .bind(source)
            .bind(post_id)
            .fetch_optional(&self.pool)
            .await?;
            Ok(at.map(|t| t.timestamp()))
        }
    }

    #[tokio::test]
    async fn upsert_posts_keeps_first_seen() {
        let db = Db::in_memory().await.unwrap();
        let t1 = Utc::now();
        db.upsert_posts(&[("hackernews".into(), "1".into(), "Title".into())], t1)
            .await
            .unwrap();
        db.upsert_posts(
            &[("hackernews".into(), "1".into(), "Title changed".into())],
            t1 + Duration::hours(1),
        )
        .await
        .unwrap();
        let first = db.post_first_seen("hackernews", "1").await.unwrap();
        assert_eq!(first, Some(t1.timestamp()));
    }
}
