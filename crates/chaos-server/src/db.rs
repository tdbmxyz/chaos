//! SQLite persistence for the links domain.
//!
//! Storage conventions (see migrations/0001_links.sql): UUIDs as hyphenated
//! TEXT, timestamps as RFC3339 TEXT, `ArchiveState` flattened into columns.
//! All mapping between rows and `chaos-domain` types happens here and only
//! here — handlers never see SQL types.

use std::collections::HashMap;
use std::path::Path;

use chaos_domain::{
    ArchiveState, Collection, CollectionRequest, CreateLinkRequest, Link, LinkPage, LinkQuery, Tag,
    TagWithCount, UpdateLinkRequest,
};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{QueryBuilder, Row, SqlitePool};
use uuid::Uuid;

const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("not found")]
    NotFound,
    /// Integrity violations the caller can act on (bad reference, cycle…).
    #[error("{0}")]
    Constraint(String),
    #[error("invalid stored data: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Clone)]
pub struct Db {
    // pub(crate) so db_auth.rs / db_calendar.rs can add impl blocks.
    pub(crate) pool: SqlitePool,
}

impl Db {
    pub async fn connect(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| DbError::Constraint(format!("creating {}: {e}", parent.display())))?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        Self::with_options(options).await
    }

    /// In-memory database for tests.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self> {
        use std::str::FromStr;
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .expect("valid memory dsn")
            .foreign_keys(true);
        Self::with_options(options).await
    }

    async fn with_options(options: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            // A single writer avoids SQLITE_BUSY surprises; reads are cheap.
            .max_connections(4)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    // ---- collections ----

    pub async fn list_collections(&self) -> Result<Vec<Collection>> {
        let rows = sqlx::query_as::<_, CollectionRow>(
            "SELECT * FROM collections ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Collection::try_from).collect()
    }

    pub async fn create_collection(&self, req: &CollectionRequest) -> Result<Collection> {
        validate_name(&req.name)?;
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO collections (id, name, description, color, parent_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(req.name.trim())
        .bind(
            req.description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .bind(&req.color)
        .bind(req.parent_id.map(|p| p.to_string()))
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(map_reference_err)?;
        self.get_collection(id).await
    }

    pub async fn update_collection(&self, id: Uuid, req: &CollectionRequest) -> Result<Collection> {
        validate_name(&req.name)?;
        if let Some(parent_id) = req.parent_id {
            self.ensure_no_collection_cycle(id, parent_id).await?;
        }
        let result = sqlx::query(
            "UPDATE collections SET name = ?, description = ?, color = ?, parent_id = ?
             WHERE id = ?",
        )
        .bind(req.name.trim())
        .bind(
            req.description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .bind(&req.color)
        .bind(req.parent_id.map(|p| p.to_string()))
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_reference_err)?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.get_collection(id).await
    }

    pub async fn delete_collection(&self, id: Uuid) -> Result<()> {
        // ON DELETE SET NULL: children become roots, links become unsorted.
        let result = sqlx::query("DELETE FROM collections WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn get_collection(&self, id: Uuid) -> Result<Collection> {
        let row = sqlx::query_as::<_, CollectionRow>("SELECT * FROM collections WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(DbError::NotFound)?;
        Collection::try_from(row)
    }

    /// Reject a `parent_id` that would make `id` an ancestor of itself.
    async fn ensure_no_collection_cycle(&self, id: Uuid, new_parent: Uuid) -> Result<()> {
        let mut cursor = Some(new_parent.to_string());
        let id = id.to_string();
        // Bounded walk: deeper nesting than this is certainly a data bug.
        for _ in 0..64 {
            let Some(current) = cursor else {
                return Ok(());
            };
            if current == id {
                return Err(DbError::Constraint(
                    "collection cannot be its own ancestor".into(),
                ));
            }
            cursor = sqlx::query_scalar::<_, Option<String>>(
                "SELECT parent_id FROM collections WHERE id = ?",
            )
            .bind(current)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        }
        Err(DbError::Constraint("collection nesting too deep".into()))
    }

    // ---- tags ----

    pub async fn list_tags(&self) -> Result<Vec<TagWithCount>> {
        let rows = sqlx::query(
            "SELECT t.id, t.name, COUNT(lt.link_id) AS link_count
             FROM tags t
             LEFT JOIN link_tags lt ON lt.tag_id = t.id
             GROUP BY t.id
             ORDER BY t.name COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(TagWithCount {
                    tag: Tag {
                        id: parse_uuid(row.get::<String, _>("id"))?,
                        name: row.get("name"),
                    },
                    link_count: row.get::<i64, _>("link_count") as u64,
                })
            })
            .collect()
    }

    // ---- links ----

    /// `archive` decides the initial archive state: `true` enqueues the link
    /// for the archiver (the caller must wake it up afterwards). `created_by`
    /// is attribution only (see migrations/0004_links_created_by.sql) — it
    /// never restricts who can see or edit the link.
    pub async fn create_link(
        &self,
        req: &CreateLinkRequest,
        archive: bool,
        created_by: Option<Uuid>,
    ) -> Result<Link> {
        let id = Uuid::now_v7();
        let now = Utc::now();
        let title = req
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            // Metadata fetch happens in the API layer; this is the last resort.
            .unwrap_or_else(|| req.url.host_str().unwrap_or("untitled").to_string());

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO links (id, url, title, description, collection_id,
                                archive_state, created_by, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(req.url.as_str())
        .bind(&title)
        .bind(
            req.description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .bind(req.collection_id.map(|c| c.to_string()))
        .bind(if archive { "pending" } else { "none" })
        .bind(created_by.map(|u| u.to_string()))
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(map_reference_err)?;
        set_link_tags(&mut tx, id, &req.tags).await?;
        tx.commit().await?;

        self.get_link(id).await
    }

    pub async fn update_link(&self, id: Uuid, req: &UpdateLinkRequest) -> Result<Link> {
        validate_name(&req.title)?;
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE links SET url = ?, title = ?, description = ?, collection_id = ?,
                              updated_at = ?
             WHERE id = ?",
        )
        .bind(req.url.as_str())
        .bind(req.title.trim())
        .bind(
            req.description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .bind(req.collection_id.map(|c| c.to_string()))
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(map_reference_err)?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        set_link_tags(&mut tx, id, &req.tags).await?;
        tx.commit().await?;

        self.get_link(id).await
    }

    pub async fn delete_link(&self, id: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("DELETE FROM links WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        sqlx::query("DELETE FROM archive_fts WHERE link_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        collect_orphan_tags(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    // ---- archiving ----

    /// (Re-)enqueue a link for archiving.
    pub async fn set_archive_pending(&self, id: Uuid) -> Result<Link> {
        let result = sqlx::query(
            "UPDATE links SET archive_state = 'pending', archived_at = NULL,
                              archive_size_bytes = NULL, archive_error = NULL
             WHERE id = ?",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.get_link(id).await
    }

    /// Oldest link waiting for the archiver, if any. Single-worker setup:
    /// no claim marking needed, the worker owns all pending rows.
    pub async fn next_pending_archive(&self) -> Result<Option<Link>> {
        let row = sqlx::query_as::<_, LinkRow>(
            "SELECT * FROM links WHERE archive_state = 'pending' ORDER BY updated_at LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(self.get_link(parse_uuid(row.id)?).await?)),
            None => Ok(None),
        }
    }

    /// Record the outcome of an archive attempt; on success the extracted
    /// text replaces the link's full-text index entry.
    pub async fn finish_archive(&self, id: Uuid, outcome: ArchiveOutcome) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();
        let result = match &outcome {
            ArchiveOutcome::Success { size_bytes, .. } => {
                sqlx::query(
                    "UPDATE links SET archive_state = 'archived', archived_at = ?,
                                  archive_size_bytes = ?, archive_error = NULL
                 WHERE id = ?",
                )
                .bind(now)
                .bind(*size_bytes as i64)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?
            }
            ArchiveOutcome::Failure { reason } => {
                sqlx::query(
                    "UPDATE links SET archive_state = 'failed', archived_at = ?,
                                  archive_size_bytes = NULL, archive_error = ?
                 WHERE id = ?",
                )
                .bind(now)
                .bind(reason)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?
            }
        };
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }

        sqlx::query("DELETE FROM archive_fts WHERE link_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        if let ArchiveOutcome::Success { text, .. } = &outcome
            && !text.is_empty()
        {
            sqlx::query("INSERT INTO archive_fts (link_id, content) VALUES (?, ?)")
                .bind(id.to_string())
                .bind(text)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_link(&self, id: Uuid) -> Result<Link> {
        let row = sqlx::query_as::<_, LinkRow>("SELECT * FROM links WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or(DbError::NotFound)?;
        let mut tags = self.tags_for_links(std::slice::from_ref(&row.id)).await?;
        let mut link = Link::try_from(row)?;
        link.tags = tags.remove(&link.id.to_string()).unwrap_or_default();
        Ok(link)
    }

    pub async fn list_links(&self, query: &LinkQuery) -> Result<LinkPage> {
        let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE).min(MAX_PAGE_SIZE);
        let offset = query.offset.unwrap_or(0);

        // WHERE clause shared by the page and count queries.
        let push_filters = |qb: &mut QueryBuilder<sqlx::Sqlite>| {
            if query.tag.is_some() {
                qb.push(
                    " JOIN link_tags flt ON flt.link_id = l.id
                      JOIN tags ft ON ft.id = flt.tag_id",
                );
            }
            qb.push(" WHERE 1 = 1");
            if let Some(collection_id) = query.collection_id {
                qb.push(" AND l.collection_id = ");
                qb.push_bind(collection_id.to_string());
            }
            if let Some(tag) = &query.tag {
                qb.push(" AND ft.name = ");
                qb.push_bind(tag.clone());
                qb.push(" COLLATE NOCASE");
            }
            if let Some(q) = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
                // Substring match over the link fields, plus full-text match
                // over archived page content.
                let pattern = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
                qb.push(" AND (l.title LIKE ");
                qb.push_bind(pattern.clone());
                qb.push(" ESCAPE '\\' OR l.description LIKE ");
                qb.push_bind(pattern.clone());
                qb.push(" ESCAPE '\\' OR l.url LIKE ");
                qb.push_bind(pattern);
                qb.push(" ESCAPE '\\'");
                let fts = fts_match_query(q);
                if !fts.is_empty() {
                    qb.push(
                        " OR l.id IN (SELECT link_id FROM archive_fts WHERE archive_fts MATCH ",
                    );
                    qb.push_bind(fts);
                    qb.push(")");
                }
                qb.push(")");
            }
        };

        let mut count_qb = QueryBuilder::new("SELECT COUNT(DISTINCT l.id) FROM links l");
        push_filters(&mut count_qb);
        let total: i64 = count_qb.build_query_scalar().fetch_one(&self.pool).await?;

        let mut page_qb = QueryBuilder::new("SELECT DISTINCT l.* FROM links l");
        push_filters(&mut page_qb);
        page_qb.push(" ORDER BY l.created_at DESC LIMIT ");
        page_qb.push_bind(limit as i64);
        page_qb.push(" OFFSET ");
        page_qb.push_bind(offset as i64);
        let rows: Vec<LinkRow> = page_qb.build_query_as().fetch_all(&self.pool).await?;

        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let mut tags = self.tags_for_links(&ids).await?;
        let items = rows
            .into_iter()
            .map(|row| {
                let key = row.id.clone();
                let mut link = Link::try_from(row)?;
                link.tags = tags.remove(&key).unwrap_or_default();
                Ok(link)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(LinkPage {
            items,
            total: total as u64,
        })
    }

    /// Tags for a batch of links in one query, keyed by link id.
    async fn tags_for_links(&self, link_ids: &[String]) -> Result<HashMap<String, Vec<Tag>>> {
        let mut map: HashMap<String, Vec<Tag>> = HashMap::new();
        if link_ids.is_empty() {
            return Ok(map);
        }
        let mut qb = QueryBuilder::new(
            "SELECT lt.link_id, t.id, t.name
             FROM link_tags lt JOIN tags t ON t.id = lt.tag_id
             WHERE lt.link_id IN (",
        );
        let mut separated = qb.separated(", ");
        for id in link_ids {
            separated.push_bind(id);
        }
        qb.push(") ORDER BY t.name COLLATE NOCASE");

        for row in qb.build().fetch_all(&self.pool).await? {
            map.entry(row.get("link_id")).or_default().push(Tag {
                id: parse_uuid(row.get::<String, _>("id"))?,
                name: row.get("name"),
            });
        }
        Ok(map)
    }
}

/// Replace the tag set of a link: upsert tag names, rewrite the junction
/// rows, drop tags that no longer tag anything.
async fn set_link_tags(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    link_id: Uuid,
    names: &[String],
) -> Result<()> {
    sqlx::query("DELETE FROM link_tags WHERE link_id = ?")
        .bind(link_id.to_string())
        .execute(&mut **tx)
        .await?;

    let mut seen: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() || seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            continue;
        }
        seen.push(name.to_string());

        sqlx::query("INSERT INTO tags (id, name) VALUES (?, ?) ON CONFLICT(name) DO NOTHING")
            .bind(Uuid::now_v7().to_string())
            .bind(name)
            .execute(&mut **tx)
            .await?;
        let tag_id: String =
            sqlx::query_scalar("SELECT id FROM tags WHERE name = ? COLLATE NOCASE")
                .bind(name)
                .fetch_one(&mut **tx)
                .await?;
        sqlx::query("INSERT INTO link_tags (link_id, tag_id) VALUES (?, ?)")
            .bind(link_id.to_string())
            .bind(tag_id)
            .execute(&mut **tx)
            .await?;
    }

    collect_orphan_tags(tx).await
}

async fn collect_orphan_tags(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    sqlx::query("DELETE FROM tags WHERE id NOT IN (SELECT DISTINCT tag_id FROM link_tags)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Outcome of an archive attempt, reported by the archiver worker.
#[derive(Debug)]
pub enum ArchiveOutcome {
    Success {
        size_bytes: u64,
        /// Plain text extracted from the snapshot, for full-text search.
        text: String,
    },
    Failure {
        reason: String,
    },
}

/// Turn free-form user input into a safe FTS5 MATCH expression: every token
/// is double-quoted (phrase syntax), so operators and punctuation in the
/// input cannot break the query.
fn fts_match_query(q: &str) -> String {
    q.split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "")))
        .filter(|t| t.len() > 2) // drop empty quotes
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(DbError::Constraint("name must not be empty".into()));
    }
    Ok(())
}

/// Surface foreign-key violations as constraint errors the API can turn
/// into a 4xx instead of a 500.
fn map_reference_err(err: sqlx::Error) -> DbError {
    match &err {
        sqlx::Error::Database(db) if db.is_foreign_key_violation() => {
            DbError::Constraint("referenced entity does not exist".into())
        }
        _ => DbError::Sqlx(err),
    }
}

fn parse_uuid(s: String) -> Result<Uuid> {
    Uuid::parse_str(&s).map_err(|_| DbError::Corrupt(format!("invalid uuid {s:?}")))
}

fn parse_url(s: String) -> Result<url::Url> {
    url::Url::parse(&s).map_err(|_| DbError::Corrupt(format!("invalid url {s:?}")))
}

// ---- row types ----

#[derive(sqlx::FromRow)]
struct CollectionRow {
    id: String,
    name: String,
    description: Option<String>,
    color: Option<String>,
    parent_id: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<CollectionRow> for Collection {
    type Error = DbError;

    fn try_from(row: CollectionRow) -> Result<Self> {
        Ok(Collection {
            id: parse_uuid(row.id)?,
            name: row.name,
            description: row.description,
            color: row.color,
            parent_id: row.parent_id.map(parse_uuid).transpose()?,
            created_at: row.created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    id: String,
    url: String,
    title: String,
    description: Option<String>,
    collection_id: Option<String>,
    created_by: Option<String>,
    archive_state: String,
    archived_at: Option<DateTime<Utc>>,
    archive_size_bytes: Option<i64>,
    archive_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<LinkRow> for Link {
    type Error = DbError;

    fn try_from(row: LinkRow) -> Result<Self> {
        let archive = match row.archive_state.as_str() {
            "none" => ArchiveState::None,
            "pending" => ArchiveState::Pending,
            "archived" => ArchiveState::Archived {
                at: row
                    .archived_at
                    .ok_or_else(|| DbError::Corrupt("archived without archived_at".into()))?,
                size_bytes: row.archive_size_bytes.unwrap_or(0) as u64,
            },
            "failed" => ArchiveState::Failed {
                at: row
                    .archived_at
                    .ok_or_else(|| DbError::Corrupt("failed without archived_at".into()))?,
                reason: row.archive_error.unwrap_or_default(),
            },
            other => return Err(DbError::Corrupt(format!("archive_state {other:?}"))),
        };
        Ok(Link {
            id: parse_uuid(row.id)?,
            url: parse_url(row.url)?,
            title: row.title,
            description: row.description,
            collection_id: row.collection_id.map(parse_uuid).transpose()?,
            created_by: row.created_by.map(parse_uuid).transpose()?,
            tags: Vec::new(), // filled by the caller
            archive,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_req(url: &str, tags: &[&str]) -> CreateLinkRequest {
        CreateLinkRequest {
            url: url.parse().unwrap(),
            title: None,
            description: None,
            collection_id: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn collection_crud_and_cycle_guard() {
        let db = Db::in_memory().await.unwrap();

        let root = db
            .create_collection(&CollectionRequest {
                name: "Dev".into(),
                description: None,
                color: Some("#7c9aff".into()),
                parent_id: None,
            })
            .await
            .unwrap();
        let child = db
            .create_collection(&CollectionRequest {
                name: "Rust".into(),
                description: None,
                color: None,
                parent_id: Some(root.id),
            })
            .await
            .unwrap();
        assert_eq!(child.parent_id, Some(root.id));
        assert_eq!(db.list_collections().await.unwrap().len(), 2);

        // Making the root a child of its own child must fail.
        let cycle = db
            .update_collection(
                root.id,
                &CollectionRequest {
                    name: "Dev".into(),
                    description: None,
                    color: None,
                    parent_id: Some(child.id),
                },
            )
            .await;
        assert!(matches!(cycle, Err(DbError::Constraint(_))));

        // Unknown parent is a constraint error, not a 500.
        let bad_ref = db
            .create_collection(&CollectionRequest {
                name: "Orphan".into(),
                description: None,
                color: None,
                parent_id: Some(Uuid::now_v7()),
            })
            .await;
        assert!(matches!(bad_ref, Err(DbError::Constraint(_))));

        // Deleting the root leaves the child as a new root.
        db.delete_collection(root.id).await.unwrap();
        let remaining = db.list_collections().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].parent_id, None);
    }

    #[tokio::test]
    async fn link_crud_with_tags_and_orphan_gc() {
        let db = Db::in_memory().await.unwrap();

        let link = db
            .create_link(
                &link_req("https://blog.rust-lang.org/post", &["rust", "blog"]),
                false,
                None,
            )
            .await
            .unwrap();
        // Title falls back to the host until metadata fetch lands.
        assert_eq!(link.title, "blog.rust-lang.org");
        assert_eq!(link.archive, ArchiveState::None);
        let names: Vec<_> = link.tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["blog", "rust"]);

        // Duplicate/whitespace tag names collapse; matching is case-insensitive.
        let other = db
            .create_link(
                &link_req("https://example.com", &["Rust", " rust ", ""]),
                false,
                None,
            )
            .await
            .unwrap();
        assert_eq!(other.tags.len(), 1);
        assert_eq!(db.list_tags().await.unwrap().len(), 2);
        let rust_count = db
            .list_tags()
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.tag.name == "rust")
            .unwrap()
            .link_count;
        assert_eq!(rust_count, 2);

        // Full-replacement update rewrites the tag set and GCs "blog".
        let updated = db
            .update_link(
                link.id,
                &UpdateLinkRequest {
                    url: link.url.clone(),
                    title: "Rust blog".into(),
                    description: Some("official".into()),
                    collection_id: None,
                    tags: vec!["rust".into()],
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.title, "Rust blog");
        assert_eq!(updated.tags.len(), 1);
        assert!(updated.updated_at > updated.created_at);
        assert!(
            db.list_tags()
                .await
                .unwrap()
                .iter()
                .all(|t| t.tag.name != "blog")
        );

        // Deleting the last "rust"-tagged links GCs the tag entirely.
        db.delete_link(link.id).await.unwrap();
        db.delete_link(other.id).await.unwrap();
        assert!(db.list_tags().await.unwrap().is_empty());
        assert!(matches!(db.get_link(link.id).await, Err(DbError::NotFound)));
        assert!(matches!(
            db.delete_link(link.id).await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn archive_lifecycle_and_fts_search() {
        let db = Db::in_memory().await.unwrap();

        // Created with archiving enabled -> pending.
        let link = db
            .create_link(&link_req("https://example.com/article", &[]), true, None)
            .await
            .unwrap();
        assert_eq!(link.archive, ArchiveState::Pending);
        assert_eq!(
            db.next_pending_archive().await.unwrap().unwrap().id,
            link.id
        );

        // Successful archive stores state + text; FTS search finds it even
        // though title/description/url contain none of the words.
        db.finish_archive(
            link.id,
            ArchiveOutcome::Success {
                size_bytes: 1234,
                text: "the quick brown fox jumped over the lazy dog".into(),
            },
        )
        .await
        .unwrap();
        assert!(db.next_pending_archive().await.unwrap().is_none());
        let archived = db.get_link(link.id).await.unwrap();
        assert!(matches!(
            archived.archive,
            ArchiveState::Archived {
                size_bytes: 1234,
                ..
            }
        ));
        let hits = db
            .list_links(&LinkQuery {
                q: Some("brown fox".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(hits.total, 1);
        // Porter stemming: "jumping" matches "jumped".
        let stemmed = db
            .list_links(&LinkQuery {
                q: Some("jumping".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(stemmed.total, 1);
        // Hostile FTS input must not error out (operators become literal
        // quoted tokens; "AND" isn't in the text, so no match — that's fine).
        let hostile = db
            .list_links(&LinkQuery {
                q: Some("\"fox* AND (dog OR".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(hostile.total, 0);
        // Sanity: punctuation-stripped tokens still match on their own.
        let punctuated = db
            .list_links(&LinkQuery {
                q: Some("fox* dog!".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(punctuated.total, 1);

        // Re-archive resets to pending; failure records the reason.
        let pending = db.set_archive_pending(link.id).await.unwrap();
        assert_eq!(pending.archive, ArchiveState::Pending);
        db.finish_archive(
            link.id,
            ArchiveOutcome::Failure {
                reason: "timeout".into(),
            },
        )
        .await
        .unwrap();
        let failed = db.get_link(link.id).await.unwrap();
        assert!(
            matches!(failed.archive, ArchiveState::Failed { ref reason, .. } if reason == "timeout")
        );
        // Failed archive removed its FTS entry.
        let gone = db
            .list_links(&LinkQuery {
                q: Some("brown fox".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(gone.total, 0);

        // Deleting a link cleans its FTS row (would otherwise orphan).
        db.finish_archive(
            link.id,
            ArchiveOutcome::Success {
                size_bytes: 10,
                text: "unique zanzibar content".into(),
            },
        )
        .await
        .unwrap();
        db.delete_link(link.id).await.unwrap();
        let after_delete = db
            .list_links(&LinkQuery {
                q: Some("zanzibar".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(after_delete.total, 0);
    }

    #[tokio::test]
    async fn list_links_filters_and_pagination() {
        let db = Db::in_memory().await.unwrap();
        let dev = db
            .create_collection(&CollectionRequest {
                name: "Dev".into(),
                description: None,
                color: None,
                parent_id: None,
            })
            .await
            .unwrap();

        db.create_link(
            &CreateLinkRequest {
                url: "https://docs.rs/axum".parse().unwrap(),
                title: Some("axum docs".into()),
                description: None,
                collection_id: Some(dev.id),
                tags: vec!["rust".into(), "web".into()],
            },
            false,
            None,
        )
        .await
        .unwrap();
        db.create_link(
            &CreateLinkRequest {
                url: "https://leptos.dev".parse().unwrap(),
                title: Some("leptos".into()),
                description: Some("reactive UI".into()),
                collection_id: Some(dev.id),
                tags: vec!["rust".into()],
            },
            false,
            None,
        )
        .await
        .unwrap();
        db.create_link(
            &link_req("https://news.ycombinator.com", &["news"]),
            false,
            None,
        )
        .await
        .unwrap();

        let all = db.list_links(&LinkQuery::default()).await.unwrap();
        assert_eq!(all.total, 3);
        assert_eq!(all.items.len(), 3);

        let by_collection = db
            .list_links(&LinkQuery {
                collection_id: Some(dev.id),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_collection.total, 2);

        let by_tag = db
            .list_links(&LinkQuery {
                tag: Some("RUST".into()), // case-insensitive
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_tag.total, 2);

        let search = db
            .list_links(&LinkQuery {
                q: Some("reactive".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(search.total, 1);
        assert_eq!(search.items[0].title, "leptos");

        let page = db
            .list_links(&LinkQuery {
                limit: Some(2),
                offset: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.items.len(), 1);
    }
}
