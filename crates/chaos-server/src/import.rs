//! One-shot importer for Linkwarden JSON backups
//! (`chaos-server import-linkwarden <export.json>`).
//!
//! Format: the export produced by Linkwarden's "Export Data" — a user object
//! with `collections[]`, each carrying its `links[]` (with `tags[]`) plus
//! `parentId` for sub-collections (see
//! inspirations/linkwarden/.../migration/exportData.ts). Unknown fields are
//! ignored so minor Linkwarden version drift stays harmless.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use chaos_domain::{CollectionRequest, CreateLinkRequest};
use serde::Deserialize;

use crate::state::AppState;

#[derive(Deserialize)]
struct Backup {
    #[serde(default)]
    collections: Vec<BackupCollection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupCollection {
    #[serde(default)]
    id: Option<i64>,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    parent_id: Option<i64>,
    #[serde(default)]
    links: Vec<BackupLink>,
}

#[derive(Deserialize)]
struct BackupLink {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<BackupTag>,
}

#[derive(Deserialize)]
struct BackupTag {
    name: String,
}

pub async fn linkwarden(state: &AppState, path: &Path) -> anyhow::Result<()> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let backup: Backup = serde_json::from_str(&raw).context("parsing Linkwarden export")?;

    let archive = state.config.archive.enabled && state.config.archive.auto;
    let mut collections = 0usize;
    let mut links = 0usize;
    let mut skipped = 0usize;

    // Pass 1: create collections, remembering Linkwarden id -> chaos id.
    let mut id_map = HashMap::new();
    for c in &backup.collections {
        let created = state
            .db
            .create_collection(&CollectionRequest {
                name: c.name.clone(),
                description: c.description.clone().filter(|d| !d.trim().is_empty()),
                color: c.color.clone().filter(|c| !c.trim().is_empty()),
                parent_id: None, // wired in pass 2
            })
            .await
            .with_context(|| format!("creating collection {:?}", c.name))?;
        collections += 1;
        if let Some(old_id) = c.id {
            id_map.insert(old_id, created.id);
        }

        // Pass 1.5: links of this collection.
        for l in &c.links {
            let Some(url) = l.url.as_deref().map(str::trim).and_then(|u| u.parse().ok()) else {
                skipped += 1;
                continue;
            };
            state
                .db
                .create_link(
                    &CreateLinkRequest {
                        url,
                        title: l.name.clone().filter(|n| !n.trim().is_empty()),
                        description: l.description.clone(),
                        collection_id: Some(created.id),
                        tags: l.tags.iter().map(|t| t.name.clone()).collect(),
                    },
                    archive,
                )
                .await
                .with_context(|| format!("importing link {:?}", l.url))?;
            links += 1;
        }
    }

    // Pass 2: restore the collection hierarchy.
    for c in &backup.collections {
        let (Some(old_id), Some(old_parent)) = (c.id, c.parent_id) else {
            continue;
        };
        let (Some(&new_id), Some(&new_parent)) = (id_map.get(&old_id), id_map.get(&old_parent))
        else {
            continue;
        };
        let current = state
            .db
            .list_collections()
            .await?
            .into_iter()
            .find(|col| col.id == new_id)
            .context("imported collection vanished")?;
        state
            .db
            .update_collection(
                new_id,
                &CollectionRequest {
                    name: current.name,
                    description: current.description,
                    color: current.color,
                    parent_id: Some(new_parent),
                },
            )
            .await?;
    }

    println!(
        "imported {collections} collections and {links} links ({skipped} without valid URL skipped)"
    );
    if archive && links > 0 {
        println!("{links} links queued for archiving; they will be processed on next server start");
    }
    Ok(())
}
