//! Dependency-free, wasm-safe HTML-to-text reduction.
//!
//! The offline reader path parses provider comment HTML in the webview, where
//! `ammonia` (native-only) is unavailable and injecting HTML would be unsafe.
//! [`strip_to_text`] flattens the markup to plain text with a single char scan:
//! no regex, no HTML parser, no non-wasm dependency.

/// Best-effort conversion of provider comment HTML to plain text: drop tags,
/// map `<p>`/`<br>` to newlines, decode the five predefined XML entities. Used
/// only on the offline path, which must never emit HTML into the webview.
pub fn strip_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                // Read the tag name to decide on a newline for <p>/<br>.
                let mut tag = String::new();
                for t in chars.by_ref() {
                    if t == '>' {
                        break;
                    }
                    tag.push(t);
                }
                let name = tag.trim_start_matches('/').trim().to_ascii_lowercase();
                if (name.starts_with('p') || name.starts_with("br"))
                    && !out.ends_with('\n')
                    && !out.is_empty()
                {
                    out.push('\n');
                }
            }
            '&' => {
                let mut ent = String::new();
                while let Some(&n) = chars.peek() {
                    if n == ';' {
                        chars.next();
                        break;
                    }
                    if ent.len() > 6 {
                        break;
                    }
                    ent.push(n);
                    chars.next();
                }
                out.push_str(match ent.as_str() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "#39" | "apos" => "'",
                    _ => "", // unknown entity dropped
                });
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_tags_keeps_text() {
        assert_eq!(strip_to_text("<p>hello <b>world</b></p>"), "hello world");
    }

    #[test]
    fn strip_decodes_basic_entities() {
        assert_eq!(strip_to_text("a &amp; b &lt;c&gt;"), "a & b <c>");
    }

    #[test]
    fn strip_linkifies_bare_urls_as_plain_text() {
        // URLs survive as readable text (not turned into HTML).
        assert_eq!(
            strip_to_text("see https://x.io/y here"),
            "see https://x.io/y here"
        );
    }

    #[test]
    fn strip_collapses_paragraph_breaks_to_newlines() {
        assert_eq!(strip_to_text("<p>a</p><p>b</p>"), "a\nb");
    }
}
