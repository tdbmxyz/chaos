//! Per-user post engagement. Every query is scoped by user id — another
//! user's view rows behave exactly like rows that do not exist.

use chaos_domain::{ViewEvent, ViewFlags, ViewedMap};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::{Db, Result};

/// A `post_views` row as read back: post_id plus the three nullable `*_at`
/// timestamps (presence = signal on).
type ViewRow = (String, Option<String>, Option<String>, Option<String>);

impl Db {
    /// Record one engagement event, first-timestamp-wins. Every event ensures
    /// `seen_at` is set; `opened_*` set their own column only when still null.
    pub async fn record_view(
        &self,
        user_id: Uuid,
        source: &str,
        post_id: &str,
        event: ViewEvent,
        at: DateTime<Utc>,
    ) -> Result<()> {
        // Columns to set-if-null for this event.
        let (set_comments, set_article) = match event {
            ViewEvent::Seen => (false, false),
            ViewEvent::OpenedComments => (true, false),
            ViewEvent::OpenedArticle => (false, true),
        };
        sqlx::query(
            "INSERT INTO post_views
                (user_id, source, post_id, seen_at, opened_comments_at, opened_article_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?4)
             ON CONFLICT(user_id, source, post_id) DO UPDATE SET
                seen_at            = COALESCE(post_views.seen_at, excluded.seen_at),
                opened_comments_at = COALESCE(post_views.opened_comments_at, excluded.opened_comments_at),
                opened_article_at  = COALESCE(post_views.opened_article_at, excluded.opened_article_at),
                updated_at         = excluded.updated_at",
        )
        .bind(user_id.to_string())
        .bind(source)
        .bind(post_id)
        .bind(at) // seen_at (+ updated_at via ?4)
        .bind(set_comments.then_some(at)) // opened_comments_at or NULL
        .bind(set_article.then_some(at)) // opened_article_at or NULL
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The user's flags for a source, keyed by post_id. A column being set
    /// (non-null) means that signal is on.
    pub async fn viewed_map(&self, user_id: Uuid, source: &str) -> Result<ViewedMap> {
        let rows: Vec<ViewRow> = sqlx::query_as(
            "SELECT post_id, seen_at, opened_comments_at, opened_article_at
             FROM post_views WHERE user_id = ?1 AND source = ?2",
        )
        .bind(user_id.to_string())
        .bind(source)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, s, c, a)| {
                (
                    id,
                    ViewFlags::from_times(s.is_some(), c.is_some(), a.is_some()),
                )
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    async fn test_user(db: &Db, username: &str) -> Uuid {
        db.create_user(username, username, "x")
            .await
            .expect("user")
            .id
    }

    #[tokio::test]
    async fn record_view_is_first_write_wins_and_implies_seen() {
        let db = Db::in_memory().await.unwrap();
        let uid = test_user(&db, "tibo").await;
        let t1 = Utc::now();
        db.record_view(uid, "hackernews", "1", ViewEvent::Seen, t1)
            .await
            .unwrap();
        // second Seen must NOT move seen_at
        db.record_view(
            uid,
            "hackernews",
            "1",
            ViewEvent::Seen,
            t1 + Duration::hours(1),
        )
        .await
        .unwrap();
        // OpenedComments sets comments + keeps seen; not article
        db.record_view(
            uid,
            "hackernews",
            "1",
            ViewEvent::OpenedComments,
            t1 + Duration::hours(2),
        )
        .await
        .unwrap();

        let map = db.viewed_map(uid, "hackernews").await.unwrap();
        let f = map.get("1").copied().unwrap();
        assert_eq!(
            f,
            ViewFlags {
                seen: true,
                comments: true,
                article: false
            }
        );
    }

    #[tokio::test]
    async fn viewed_map_is_user_scoped() {
        let db = Db::in_memory().await.unwrap();
        let a = test_user(&db, "a").await;
        let b = test_user(&db, "b").await;
        db.record_view(a, "lobsters", "x", ViewEvent::Seen, Utc::now())
            .await
            .unwrap();
        assert!(db.viewed_map(b, "lobsters").await.unwrap().is_empty());
        assert_eq!(db.viewed_map(a, "lobsters").await.unwrap().len(), 1);
    }
}
