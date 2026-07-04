//! Link management domain (the Linkwarden replacement).
//!
//! Defined up-front (Phase 2 implements storage/API/UI) because these types
//! shape the database schema and the API surface.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

/// A saved link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub id: Uuid,
    pub url: Url,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Collection the link belongs to; `None` means "unsorted".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection_id: Option<Uuid>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    pub archive: ArchiveState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Hierarchical grouping of links (Linkwarden "collections" / categories).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Hex color used as the collection accent, e.g. `"#7c3aed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: Uuid,
    pub name: String,
}

/// Lifecycle of the single-file page snapshot kept for a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ArchiveState {
    /// Archiving disabled or not requested for this link.
    None,
    /// Queued; the archiver will process it.
    Pending,
    Archived {
        at: DateTime<Utc>,
        /// Size of the snapshot in bytes, for display purposes.
        size_bytes: u64,
    },
    Failed {
        at: DateTime<Utc>,
        reason: String,
    },
}
