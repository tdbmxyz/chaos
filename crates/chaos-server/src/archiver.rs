//! Background archiver: turns pending links into single-file page snapshots.
//!
//! Shells out to `monolith` (process isolation: a hostile or huge page can
//! only kill the child), stores `<archive-dir>/<link-id>.html`, extracts the
//! plain text into the FTS index, and records the outcome on the link.
//! Single worker by design — archiving is bandwidth-bound and a queue depth
//! of one keeps SQLite write contention trivial.

use std::path::PathBuf;
use std::time::Duration;

use chaos_domain::Link;
use scraper::Html;
use uuid::Uuid;

use crate::db::ArchiveOutcome;
use crate::state::AppState;

/// Cap on text stored in the FTS index per page.
const MAX_FTS_TEXT_BYTES: usize = 512 * 1024;

/// Snapshots larger than this fail the archive instead of being read into
/// memory; monolith can emit hundreds of MB for pathological pages and an
/// unchecked read_to_string would balloon the server.
const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

pub fn spawn(state: AppState) {
    if state.config.archive.enabled {
        tokio::spawn(run(state));
    }
}

pub fn snapshot_path(state: &AppState, id: Uuid) -> PathBuf {
    state.config.archive.dir.join(format!("{id}.html"))
}

async fn run(state: AppState) {
    if let Err(err) = tokio::fs::create_dir_all(&state.config.archive.dir).await {
        tracing::error!(
            dir = %state.config.archive.dir.display(),
            %err,
            "cannot create archive dir; archiver disabled"
        );
        return;
    }

    loop {
        match state.db.next_pending_archive().await {
            Ok(Some(link)) => {
                let outcome = archive_one(&state, &link).await;
                match state.db.finish_archive(link.id, outcome).await {
                    // Drain the queue before sleeping.
                    Ok(()) => continue,
                    // Fall through to the idle sleep: if the outcome can't
                    // be recorded the link stays pending, and looping now
                    // would re-run monolith on it in a tight loop.
                    Err(err) => {
                        tracing::error!(link = %link.id, %err, "recording archive outcome");
                    }
                }
            }
            Ok(None) => {}
            Err(err) => tracing::error!(%err, "polling archive queue"),
        }

        // Idle: wake up on demand (new link / re-archive) or periodically as
        // a safety net.
        tokio::select! {
            _ = state.archive_notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
    }
}

async fn archive_one(state: &AppState, link: &Link) -> ArchiveOutcome {
    tracing::info!(link = %link.id, url = %link.url, "archiving");
    let final_path = snapshot_path(state, link.id);
    let tmp_path = final_path.with_extension("html.tmp");

    let result = tokio::time::timeout(
        Duration::from_secs(state.config.archive.timeout_secs),
        run_monolith(state, link, &tmp_path),
    )
    .await;

    let outcome = match result {
        Err(_) => Err(format!(
            "timed out after {}s",
            state.config.archive.timeout_secs
        )),
        Ok(Err(reason)) => Err(reason),
        Ok(Ok(())) => finalize(&tmp_path, &final_path).await,
    };

    match outcome {
        Ok((size_bytes, text)) => ArchiveOutcome::Success { size_bytes, text },
        Err(reason) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            tracing::warn!(link = %link.id, reason, "archive failed");
            ArchiveOutcome::Failure { reason }
        }
    }
}

async fn run_monolith(
    state: &AppState,
    link: &Link,
    tmp_path: &std::path::Path,
) -> Result<(), String> {
    let output = tokio::process::Command::new(&state.config.archive.monolith_bin)
        .arg(link.url.as_str())
        .arg("-o")
        .arg(tmp_path)
        .arg("-q") // quiet
        .arg("-j") // strip javascript: archives are for reading, not running
        .arg("-I") // isolate: block outbound requests when viewing
        .arg("-k") // tolerate self-signed certificates (LAN services)
        .kill_on_drop(true) // killed if the timeout drops the future
        .output()
        .await
        .map_err(|e| format!("spawning {}: {e}", state.config.archive.monolith_bin))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "monolith exited with {}: {}",
            output.status,
            stderr.trim().chars().take(300).collect::<String>()
        ));
    }
    Ok(())
}

/// Move the snapshot in place and extract its text for indexing.
async fn finalize(
    tmp_path: &std::path::Path,
    final_path: &std::path::Path,
) -> Result<(u64, String), String> {
    let size_bytes = tokio::fs::metadata(tmp_path)
        .await
        .map_err(|e| format!("inspecting snapshot: {e}"))?
        .len();
    if size_bytes > MAX_SNAPSHOT_BYTES {
        return Err(format!(
            "snapshot too large ({size_bytes} bytes, cap {MAX_SNAPSHOT_BYTES})"
        ));
    }
    let html = tokio::fs::read_to_string(tmp_path)
        .await
        .map_err(|e| format!("reading snapshot: {e}"))?;

    // HTML parsing of a multi-MB page is CPU work; keep it off the runtime.
    let text = tokio::task::spawn_blocking(move || extract_text(&html))
        .await
        .map_err(|e| format!("text extraction panicked: {e}"))?;

    tokio::fs::rename(tmp_path, final_path)
        .await
        .map_err(|e| format!("moving snapshot in place: {e}"))?;
    Ok((size_bytes, text))
}

/// Visible text of the page, whitespace-collapsed and size-capped.
/// Skips subtrees that hold code rather than prose.
fn extract_text(html: &str) -> String {
    const SKIP: &[&str] = &["script", "style", "noscript", "svg", "template", "head"];

    let doc = Html::parse_document(html);
    let mut out = String::new();
    let mut stack = vec![doc.tree.root()];
    while let Some(node) = stack.pop() {
        match node.value() {
            scraper::Node::Text(text) => {
                for word in text.split_whitespace() {
                    if out.len() + word.len() + 1 > MAX_FTS_TEXT_BYTES {
                        return out;
                    }
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(word);
                }
            }
            scraper::Node::Element(el) if SKIP.contains(&el.name()) => continue,
            _ => {}
        }
        // Reverse to visit children in document order despite the LIFO stack.
        let children: Vec<_> = node.children().collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_keeps_prose_and_skips_code() {
        let text = extract_text(
            "<html><head><title>T</title><style>.hidden{color:red}</style></head>
             <body><h1>Hello</h1><script>var secret=1;</script>
             <p>brown <b>fox</b>\n jumps</p></body></html>",
        );
        assert_eq!(text, "Hello brown fox jumps");
    }

    /// Fresh temp dir per test so parallel tests don't collide.
    async fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chaos-archiver-{name}-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.expect("temp dir");
        dir
    }

    #[tokio::test]
    async fn finalize_rejects_oversized_snapshots() {
        let dir = temp_dir("oversized").await;
        let tmp = dir.join("page.html.tmp");
        let final_path = dir.join("page.html");
        // Sparse file: instant to create, but metadata reports the full size.
        let file = tokio::fs::File::create(&tmp).await.expect("create tmp");
        file.set_len(MAX_SNAPSHOT_BYTES + 1).await.expect("set_len");
        drop(file);

        let err = finalize(&tmp, &final_path)
            .await
            .expect_err("oversized snapshot must fail the archive");
        assert!(err.contains("too large"), "unexpected reason: {err}");
        // The snapshot must not have been moved into place.
        assert!(tokio::fs::metadata(&final_path).await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn finalize_moves_small_snapshots_and_extracts_text() {
        let dir = temp_dir("small").await;
        let tmp = dir.join("page.html.tmp");
        let final_path = dir.join("page.html");
        let html = "<html><body><p>hello world</p></body></html>";
        tokio::fs::write(&tmp, html).await.expect("write tmp");

        let (size_bytes, text) = finalize(&tmp, &final_path).await.expect("finalize");
        assert_eq!(size_bytes, html.len() as u64);
        assert_eq!(text, "hello world");
        assert!(tokio::fs::metadata(&final_path).await.is_ok());
        assert!(tokio::fs::metadata(&tmp).await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
