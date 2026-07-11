//! Scheduled SQLite backups: every `interval_hours`, `VACUUM INTO` a
//! timestamped copy of the database, then prune to the `keep` newest
//! files. `VACUUM INTO` produces a consistent, defragmented snapshot
//! without blocking writers (works under WAL), so no downtime is needed.
//! Failures are logged and retried next cycle — never fatal.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;

use crate::db::Db;
use crate::state::AppState;

pub fn spawn(state: AppState) {
    if state.config.backup.enabled {
        tokio::spawn(run(state));
    }
}

async fn run(state: AppState) {
    let dir = state.config.backup.dir.clone();
    let interval = Duration::from_secs(state.config.backup.interval_hours.max(1) * 3600);
    let keep = state.config.backup.keep.max(1);

    loop {
        // Re-attempted every cycle: a backup volume that wasn't mounted yet
        // at boot must not silently disable backups for the process's life.
        if let Err(err) = tokio::fs::create_dir_all(&dir).await {
            tracing::error!(
                dir = %dir.display(),
                %err,
                "cannot create backup dir; retrying next cycle"
            );
        } else {
            match backup_once(&state.db, &dir).await {
                Ok(path) => tracing::info!(path = %path.display(), "database backed up"),
                Err(err) => tracing::error!(%err, "database backup failed"),
            }
            if let Err(err) = prune(&dir, keep).await {
                tracing::warn!(%err, "pruning old backups failed");
            }
        }
        tokio::time::sleep(interval).await;
    }
}

/// Write one consistent snapshot of the live database into `dir` and
/// return its path. `VACUUM INTO` takes a filename literal, not a bind
/// parameter, so single quotes in the path are SQL-escaped.
pub async fn backup_once(db: &Db, dir: &Path) -> anyhow::Result<PathBuf> {
    let name = format!("chaos-{}.db", Utc::now().format("%Y%m%d-%H%M%S"));
    let path = dir.join(name);
    let escaped = path.display().to_string().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(&db.pool)
        .await?;
    Ok(path)
}

/// Delete everything beyond the `keep` newest backups.
async fn prune(dir: &Path, keep: usize) -> anyhow::Result<()> {
    let mut names = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    for name in to_delete(names, keep) {
        tracing::info!(file = name, "pruning old backup");
        tokio::fs::remove_file(dir.join(name)).await?;
    }
    Ok(())
}

/// The backup files to delete: everything except the `keep` newest.
/// Lexicographic order == chronological for `chaos-<timestamp>.db`
/// names; files not matching the pattern are never touched.
fn to_delete(mut names: Vec<String>, keep: usize) -> Vec<String> {
    names.retain(|n| n.starts_with("chaos-") && n.ends_with(".db"));
    names.sort();
    let cut = names.len().saturating_sub(keep);
    names.truncate(cut);
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use chaos_domain::CreateLinkRequest;

    #[test]
    fn to_delete_keeps_the_newest_and_ignores_foreign_files() {
        let names = vec![
            "chaos-20260701-000000.db".to_string(),
            "chaos-20260703-000000.db".to_string(),
            "chaos-20260702-000000.db".to_string(),
            "notes.txt".to_string(),
            "live.db".to_string(),
        ];
        assert_eq!(
            to_delete(names.clone(), 2),
            vec!["chaos-20260701-000000.db"]
        );
        assert!(to_delete(names.clone(), 3).is_empty());
        assert!(to_delete(names, 10).is_empty());
        assert!(to_delete(vec![], 5).is_empty());
    }

    #[tokio::test]
    async fn backup_once_produces_an_openable_consistent_copy() {
        // File-backed live db in a scratch dir; VACUUM INTO lands beside it.
        let dir = std::env::temp_dir().join(format!("chaos-backup-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::connect(&dir.join("live.db")).await.unwrap();

        let link = db
            .create_link(
                &CreateLinkRequest {
                    url: "https://example.com/backup".parse().unwrap(),
                    title: Some("kept across backup".into()),
                    description: None,
                    collection_id: None,
                    tags: vec![],
                },
                false,
                None,
            )
            .await
            .unwrap();

        let path = backup_once(&db, &dir).await.unwrap();
        assert!(path.exists());
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("chaos-")
        );

        // The snapshot is a complete database: opening it (re-runs the
        // migration check) and reading the link back must work.
        let restored = Db::connect(&path).await.unwrap();
        assert_eq!(
            restored.get_link(link.id).await.unwrap().title,
            "kept across backup"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
