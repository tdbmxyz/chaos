//! Static frontend serving: precompressed assets and their cache policy.

/// A year — the longest max-age browsers honour in practice. Safe only for
/// content-hashed filenames, where a change means a new URL.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Revalidate every time (a 304 is cheap; a stale `index.html` is not).
const REVALIDATE: &str = "no-cache";

/// Cache policy for a request path.
pub(crate) fn cache_control_for(_path: &str) -> &'static str {
    unimplemented!("cache_control_for")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trunk_fingerprinted_assets_are_immutable() {
        assert_eq!(
            cache_control_for("/chaos-web-1243ba43bf8faa7b_bg.wasm"),
            IMMUTABLE
        );
        assert_eq!(cache_control_for("/chaos-web-1243ba43bf8faa7b.js"), IMMUTABLE);
        assert_eq!(cache_control_for("/styles-3e1677dddc3dd7f1.css"), IMMUTABLE);
    }

    #[test]
    fn unhashed_assets_and_spa_routes_revalidate() {
        assert_eq!(cache_control_for("/index.html"), REVALIDATE);
        assert_eq!(cache_control_for("/"), REVALIDATE);
        assert_eq!(cache_control_for("/links"), REVALIDATE);
        assert_eq!(cache_control_for("/vendor/echarts.min.js"), REVALIDATE);
        assert_eq!(cache_control_for("/assets/logo.svg"), REVALIDATE);
        assert_eq!(cache_control_for("/assets/favicon-32.png"), REVALIDATE);
        assert_eq!(cache_control_for("/assets/manifest.json"), REVALIDATE);
    }

    /// A uuid in a path must not read as a fingerprint: its groups are 8, 4, 4,
    /// 4 and 12 hex digits, never 16.
    #[test]
    fn uuid_paths_are_not_mistaken_for_fingerprints() {
        assert_eq!(
            cache_control_for("/api/v1/links/019f388b-4c21-7b3a-9f10-2d4e6a8c1b55"),
            REVALIDATE
        );
    }
}
