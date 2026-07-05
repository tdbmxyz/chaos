//! GitHub releases watcher. Reads each repository's public
//! `releases.atom` feed instead of the REST API: same data, no
//! authentication and no 60-requests-per-hour rate limit to manage.

use chaos_domain::{ReleaseItem, WidgetData};

pub async fn fetch(
    http: &reqwest::Client,
    repos: &[String],
    limit: u32,
) -> Result<WidgetData, String> {
    let results =
        futures::future::join_all(repos.iter().map(|repo| latest_release(http, repo))).await;

    let mut items = Vec::new();
    let mut errors = Vec::new();
    for (repo, result) in repos.iter().zip(results) {
        match result {
            Ok(Some(item)) => items.push(item),
            Ok(None) => tracing::debug!(repo, "repository has no releases"),
            Err(reason) => {
                tracing::warn!(repo, reason, "release fetch failed");
                errors.push(format!("{repo}: {reason}"));
            }
        }
    }
    if items.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }

    items.sort_by_key(|item| std::cmp::Reverse(item.published));
    items.truncate(limit as usize);
    Ok(WidgetData::Releases { items })
}

async fn latest_release(http: &reqwest::Client, repo: &str) -> Result<Option<ReleaseItem>, String> {
    if repo.split('/').filter(|part| !part.is_empty()).count() != 2 {
        return Err("expected owner/name".into());
    }

    let url = format!("https://github.com/{repo}/releases.atom");
    let resp = http.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    let feed = feed_rs::parser::parse(body.as_ref()).map_err(|e| e.to_string())?;

    let Some(entry) = feed.entries.into_iter().next() else {
        return Ok(None);
    };
    let link = entry.links.first().map(|l| l.href.clone());
    // Release links look like …/releases/tag/<tag>; the tag is the cleanest
    // short label, falling back to the release title.
    let tag = link
        .as_deref()
        .and_then(|href| href.split("/tag/").nth(1))
        .map(|tag| tag.to_string())
        .or_else(|| entry.title.as_ref().map(|t| t.content.clone()))
        .unwrap_or_else(|| "?".into());

    Ok(Some(ReleaseItem {
        repo: repo.to_string(),
        tag,
        url: link.and_then(|href| href.parse().ok()),
        published: entry.published.or(entry.updated),
    }))
}
