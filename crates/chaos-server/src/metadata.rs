//! Page metadata extraction for saved links.
//!
//! Fills in title/description when the user pastes a bare URL. Best-effort
//! by design: any failure (timeout, non-HTML, parse error) just yields empty
//! metadata and the caller keeps its fallbacks — saving a link must never
//! fail because the page couldn't be read.

use futures::StreamExt;
use scraper::{Html, Selector};
use url::Url;

/// Hard cap on the HTML we download; metadata lives in <head>, so anything
/// beyond this is wasted bandwidth.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_TITLE_CHARS: usize = 300;
const MAX_DESCRIPTION_CHARS: usize = 1000;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PageMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Client tuned for metadata fetching; built once and stored in AppState.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .user_agent(concat!("chaos/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("building metadata http client")
}

pub async fn fetch(client: &reqwest::Client, url: &Url) -> PageMetadata {
    match try_fetch(client, url).await {
        Ok(meta) => meta,
        Err(reason) => {
            tracing::debug!(%url, reason, "metadata fetch failed");
            PageMetadata::default()
        }
    }
}

async fn try_fetch(client: &reqwest::Client, url: &Url) -> Result<PageMetadata, String> {
    let resp = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "text/html")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("html") {
        return Err(format!("content-type {content_type:?}"));
    }

    // Stream the body so a huge page cannot balloon memory; stop at the cap.
    // Deliberately not http_util::get_body_capped: that helper *fails* past
    // the cap, while metadata lives in <head> so truncating and using what
    // arrived is the right call here.
    let mut body: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        body.extend_from_slice(&chunk);
        if body.len() >= MAX_BODY_BYTES {
            body.truncate(MAX_BODY_BYTES);
            break;
        }
    }

    Ok(extract(&String::from_utf8_lossy(&body)))
}

/// Pull title/description out of an HTML document.
/// Priority: OpenGraph tags, then <title> / meta[name=description].
fn extract(html: &str) -> PageMetadata {
    let doc = Html::parse_document(html);

    let meta_content = |selector: &str| -> Option<String> {
        let sel = Selector::parse(selector).expect("valid selector");
        doc.select(&sel)
            .filter_map(|el| el.value().attr("content"))
            .map(clean)
            .find(|s| !s.is_empty())
    };

    let title = meta_content(r#"meta[property="og:title"]"#)
        .or_else(|| {
            let sel = Selector::parse("title").expect("valid selector");
            doc.select(&sel)
                .map(|el| clean(&el.text().collect::<String>()))
                .find(|s| !s.is_empty())
        })
        .map(|t| truncate_chars(t, MAX_TITLE_CHARS));

    let description = meta_content(r#"meta[property="og:description"]"#)
        .or_else(|| meta_content(r#"meta[name="description"]"#))
        .map(|d| truncate_chars(d, MAX_DESCRIPTION_CHARS));

    PageMetadata { title, description }
}

/// Collapse all whitespace runs (newlines in <title> are common) and trim.
fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(s: String, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => {
            let mut out = s[..idx].trim_end().to_string();
            out.push('…');
            out
        }
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_title_and_meta_description() {
        let meta = extract(
            r#"<html><head>
                 <title>  Example
                    Domain </title>
                 <meta name="description" content="A page &amp; more">
               </head><body><h1>ignored</h1></body></html>"#,
        );
        assert_eq!(meta.title.as_deref(), Some("Example Domain"));
        assert_eq!(meta.description.as_deref(), Some("A page & more"));
    }

    #[test]
    fn opengraph_wins_over_plain_tags() {
        let meta = extract(
            r#"<head>
                 <meta property="og:title" content="OG title">
                 <meta property="og:description" content="OG desc">
                 <title>plain title</title>
                 <meta name="description" content="plain desc">
               </head>"#,
        );
        assert_eq!(meta.title.as_deref(), Some("OG title"));
        assert_eq!(meta.description.as_deref(), Some("OG desc"));
    }

    #[test]
    fn empty_og_falls_back() {
        let meta = extract(
            r#"<head>
                 <meta property="og:title" content="  ">
                 <title>fallback</title>
               </head>"#,
        );
        assert_eq!(meta.title.as_deref(), Some("fallback"));
        assert_eq!(meta.description, None);
    }

    #[test]
    fn no_metadata_yields_empty() {
        assert_eq!(extract("<p>hello</p>"), PageMetadata::default());
    }

    #[test]
    fn long_values_truncate_on_char_boundary() {
        let long = "é".repeat(400);
        let meta = extract(&format!("<head><title>{long}</title></head>"));
        let title = meta.title.unwrap();
        assert!(title.chars().count() <= MAX_TITLE_CHARS + 1);
        assert!(title.ends_with('…'));
    }
}
