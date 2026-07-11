//! GitHub releases watcher. Reads each repository's public
//! `releases.atom` feed instead of the REST API: same data, no
//! authentication and no 60-requests-per-hour rate limit to manage.

use chaos_domain::{ReleaseItem, WidgetData};

/// Cap on a fetched releases.atom body; GitHub's feeds are a few KB, so
/// anything near this is a misbehaving response.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

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
    let body = crate::http_util::get_body_capped(http, &url, MAX_BODY_BYTES).await?;
    let feed = feed_rs::parser::parse(body.as_slice()).map_err(|e| e.to_string())?;

    let Some(entry) = feed.entries.into_iter().next() else {
        return Ok(None);
    };
    let link = entry.links.first().map(|l| l.href.clone());
    let tag = release_tag(
        link.as_deref(),
        entry.title.as_ref().map(|t| t.content.as_str()),
    );

    Ok(Some(ReleaseItem {
        repo: repo.to_string(),
        tag,
        url: link.and_then(|href| href.parse().ok()),
        published: entry.published.or(entry.updated),
    }))
}

/// Release links look like …/releases/tag/<tag>; the tag is the cleanest
/// short label, falling back to the release title, then "?".
fn release_tag(link: Option<&str>, title: Option<&str>) -> String {
    link.and_then(|href| href.split("/tag/").nth(1))
        .map(str::to_string)
        .or_else(|| title.map(str::to_string))
        .unwrap_or_else(|| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tag_prefers_the_tag_url_segment() {
        assert_eq!(
            release_tag(
                Some("https://github.com/leptos-rs/leptos/releases/tag/v0.8.2"),
                Some("v0.8.2 title"),
            ),
            "v0.8.2"
        );
        // No /tag/ segment: fall back to the entry title.
        assert_eq!(
            release_tag(Some("https://github.com/x/y/releases"), Some("v1.0")),
            "v1.0"
        );
        assert_eq!(release_tag(None, Some("v1.0")), "v1.0");
        assert_eq!(release_tag(None, None), "?");
    }
}
