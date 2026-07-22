//! Generic append-only analytics log (`analytics_events` table). The calendar
//! owns the `events` table, so the analytics log is separately named.

use chaos_domain::EventItem;
use uuid::Uuid;

use crate::db::{Db, Result};

// Wired into handlers in Tasks A5/A6; unused until then.
#[allow(dead_code)]
impl Db {
    /// Batch-insert analytics events. `user_id` is `None` for anonymous events.
    pub async fn record_events(&self, user_id: Option<Uuid>, items: &[EventItem]) -> Result<()> {
        let uid = user_id.map(|u| u.to_string());
        for it in items {
            sqlx::query(
                "INSERT INTO analytics_events (id, user_id, kind, at, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(Uuid::now_v7().to_string())
            .bind(uid.as_deref())
            .bind(&it.kind)
            .bind(it.at)
            .bind(it.detail.as_deref())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Convenience for server-side single events (login/search).
    pub async fn record_event(
        &self,
        user_id: Option<Uuid>,
        kind: &str,
        at: chrono::DateTime<chrono::Utc>,
        detail: Option<&str>,
    ) -> Result<()> {
        self.record_events(
            user_id,
            &[EventItem {
                kind: kind.into(),
                detail: detail.map(str::to_owned),
                at,
            }],
        )
        .await
    }
}

#[cfg(test)]
impl Db {
    /// Test-only: number of analytics events with the given kind. Shared by
    /// this module's tests and the login-logging test in `api/auth.rs`.
    pub(crate) async fn count_events(&self, kind: &str) -> Result<i64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM analytics_events WHERE kind = ?1")
            .bind(kind)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn record_events_inserts_rows() {
        let db = Db::in_memory().await.unwrap();
        let uid = db.create_user("tibo", "Tibo", "x").await.unwrap().id;
        db.record_events(
            Some(uid),
            &[
                EventItem {
                    kind: "app_open".into(),
                    detail: None,
                    at: Utc::now(),
                },
                EventItem {
                    kind: "reader_open".into(),
                    detail: Some("hackernews:1".into()),
                    at: Utc::now(),
                },
            ],
        )
        .await
        .unwrap();
        assert_eq!(db.count_events("app_open").await.unwrap(), 1);
    }
}
