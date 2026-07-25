# Frontend Payload Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut the dashboard's cold-load transfer from ~5.9 MB to under 1 MB and paint immediately, without growing the desktop binary or touching `/etc/nixos`.

**Architecture:** Compression happens once at build time (a new `chaos-web-static` flake package adds `.br`/`.gz` siblings) and `ServeDir` serves them precompressed with a path-aware `Cache-Control`; the wasm itself shrinks via trunk's `wasm-opt` attributes plus the existing-but-unwired `wasm-release` cargo profile; the 1 MB ECharts bundle moves from a blocking `<head>` script to a memoized on-demand loader; and an inline boot skeleton paints before the wasm instantiates.

**Tech Stack:** Rust 2024, axum 0.8, tower-http 0.6 (`ServeDir::precompressed_br`), Leptos 0.8 CSR, trunk 0.21, binaryen `wasm-opt`, nix flake, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-07-25-frontend-payload-optimization-design.md`

---

## Context for every task

- Repo root is `/projects/rust/chaos`. All commands run from there unless stated.
- Work inside `nix develop` (or with direnv active); `just` recipes assume it.
- **Commits must be unsigned and carry the repo trailers.** Use exactly:
  ```bash
  git -c commit.gpgsign=false commit -m "type: subject" \
    -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
    -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
  ```
  Do **not** push. Do **not** commit anything under `/etc/nixos`.
- **Never touch** `crates/chaos-desktop/gen/schemas/android-schema.json` or
  `crates/chaos-desktop/gen/schemas/mobile-schema.json`.
- `nix build` only sees files git knows about — `git add` new files before any
  `nix build`.
- Full check before finishing a task that touched Rust: `just check` (fmt +
  clippy `-D warnings` + wasm check) and `just test` (nextest).

## File structure

| File | Responsibility |
| --- | --- |
| `crates/chaos-server/src/api/static_assets.rs` (new) | Everything about serving the frontend: precompressed `ServeDir`, the `Cache-Control`/`Vary` middleware, and the pure path→policy classifier. Keeps `api/mod.rs` about routes. |
| `crates/chaos-server/src/api/mod.rs` | Loses its inline static-serving block; merges `static_assets::router`. |
| `crates/chaos-server/Cargo.toml` | Adds `tower` (needed for `ServiceBuilder` and, in tests, `ServiceExt::oneshot`). |
| `flake.nix` | Adds `chaos-web-static` (dist + `.br`/`.gz`); exports it. |
| `nix/module.nix` | `webPackage` defaults to `chaos-web-static`. |
| `crates/chaos-web/index.html` | Rust link with wasm-opt/profile attributes; ECharts loader instead of a blocking script; boot skeleton + inline style. |
| `crates/chaos-web/src/main.rs` | Removes the boot skeleton after mount. |
| `crates/chaos-web/Cargo.toml` | Adds the `Document`/`Element` web-sys features that removal needs. |
| `crates/chaos-ui/src/echarts.rs` | `ChartCanvas` waits for the on-demand bundle before `init`. |
| `docs/ARCHITECTURE.md` | Records the delivery decisions. |

---

### Task 1: Branch and land the spec

**Files:**
- Commit: `docs/superpowers/specs/2026-07-25-frontend-payload-optimization-design.md`, `docs/superpowers/plans/2026-07-25-frontend-payload-optimization.md`

- [ ] **Step 1: Create the working branch**

```bash
git checkout -b perf/frontend-payload
```

Expected: `Switched to a new branch 'perf/frontend-payload'`

- [ ] **Step 2: Record the pre-change baseline**

```bash
ls -l crates/chaos-web/dist/*_bg.wasm crates/chaos-web/dist/*.js
```

Expected: a `*_bg.wasm` of roughly 4 769 951 bytes. Write the exact number down;
Task 6 compares against it. (If `dist/` is absent, run `just build-web` first
and use that size as the baseline.)

- [ ] **Step 3: Commit the spec and plan**

```bash
git add docs/superpowers/specs/2026-07-25-frontend-payload-optimization-design.md \
        docs/superpowers/plans/2026-07-25-frontend-payload-optimization.md
git -c commit.gpgsign=false commit -m "docs: spec and plan for frontend payload optimization" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 2: `cache_control_for` — the path → cache policy classifier

Trunk emits content-hashed names (`chaos-web-1243ba43bf8faa7b_bg.wasm`,
`styles-3e1677dddc3dd7f1.css`): 16 lowercase hex characters as one
`-`/`_`/`.`-delimited segment of the file name. Those are safe to cache for a
year. Everything else — `index.html`, SPA routes, `/vendor/echarts.min.js`,
`/assets/*` — must revalidate.

**Files:**
- Create: `crates/chaos-server/src/api/static_assets.rs`
- Modify: `crates/chaos-server/src/api/mod.rs:3-13` (module list)

- [ ] **Step 1: Create the module with the failing test**

Create `crates/chaos-server/src/api/static_assets.rs` — the signature and the
tests, with the body left unimplemented so the tests genuinely fail:

```rust
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
```

Register the module in `crates/chaos-server/src/api/mod.rs` — the `mod`
declarations are alphabetical, so insert between `services` and `views`:

```rust
mod services;
mod static_assets;
mod views;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p chaos-server static_assets`

Expected: 3 tests run and all FAIL with
`not implemented: cache_control_for`. If nextest reports "0 tests run", the `mod
static_assets;` line is missing.

- [ ] **Step 3: Implement the classifier**

In `crates/chaos-server/src/api/static_assets.rs`, replace the
`unimplemented!()` body:

```rust
/// Cache policy for a request path.
///
/// Trunk fingerprints the assets it builds — `styles-<16 hex>.css`,
/// `chaos-web-<16 hex>_bg.wasm` — so a changed file always arrives under a new
/// URL and can be pinned forever. Hand-vendored files (`/vendor/echarts.min.js`),
/// `/assets/*`, `index.html` and SPA routes carry no hash and must revalidate.
///
/// The test is "some `-`/`_`/`.`-delimited segment of the file name is exactly
/// 16 hex digits". A hand-written file that happens to match would be pinned
/// for a year; nothing in `crates/chaos-web` does.
pub(crate) fn cache_control_for(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let hashed = name
        .split(['-', '_', '.'])
        .any(|seg| seg.len() == 16 && seg.chars().all(|c| c.is_ascii_hexdigit()));
    if hashed { IMMUTABLE } else { REVALIDATE }
}
```

- [ ] **Step 4: Verify the tests pass and the crate is clean**

Run: `cargo nextest run -p chaos-server static_assets`
Expected: 3 tests pass.

Run: `cargo clippy -p chaos-server --all-targets -- -D warnings`
Expected: clean (no dead-code warning — `cache_control_for` is used by its
tests).

- [ ] **Step 5: Commit**

```bash
git add crates/chaos-server/src/api/static_assets.rs crates/chaos-server/src/api/mod.rs
git -c commit.gpgsign=false commit -m "feat(server): classify static asset cache policy by path" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 3: Serve precompressed assets with cache headers

`ServeDir::precompressed_br()` makes tower-http look for `<file>.br` when the
request carries `Accept-Encoding: br`, serve it with `Content-Encoding: br`, and
silently fall back to the identity file when the sibling is missing — so a plain
local `just build-web` dist keeps working.

**Files:**
- Modify: `crates/chaos-server/Cargo.toml`
- Modify: `crates/chaos-server/src/api/static_assets.rs`
- Modify: `crates/chaos-server/src/api/mod.rs:79-94`

- [ ] **Step 1: Add the `tower` dependency**

In `crates/chaos-server/Cargo.toml`, add after the `thiserror.workspace = true`
line:

```toml
tower = { version = "0.5", features = ["util"] }
```

(`ServiceBuilder` for layering the header middleware onto `ServeDir`;
`ServiceExt::oneshot` in the tests. Version 0.5.3 is already in `Cargo.lock`
via axum, so no lockfile churn beyond the new direct entry.)

Run: `cargo check -p chaos-server`
Expected: compiles; `Cargo.lock` gains `tower` under `chaos-server`'s deps.

- [ ] **Step 2: Write the failing service tests**

Append to `crates/chaos-server/src/api/static_assets.rs` — replace the existing
`#[cfg(test)] mod tests { ... }` opening lines' `use super::*;` block by adding
these imports and tests inside the same module:

```rust
#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use super::*;

    /// A throwaway dist directory: one fingerprinted wasm with a `.br`
    /// sibling, one without, and an index. Named after the calling test so
    /// tests never share a directory.
    fn dist(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chaos-static-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create dist dir");
        fs::write(dir.join("index.html"), "<html>index</html>").unwrap();
        fs::write(dir.join("app-0123456789abcdef_bg.wasm"), "identity-wasm").unwrap();
        fs::write(
            dir.join("app-0123456789abcdef_bg.wasm.br"),
            "brotli-wasm-bytes",
        )
        .unwrap();
        fs::write(dir.join("styles-fedcba9876543210.css"), "body{}").unwrap();
        dir
    }

    async fn get(dir: &PathBuf, path: &str, accept_encoding: Option<&str>) -> axum::response::Response {
        let mut req = Request::builder().uri(path);
        if let Some(encoding) = accept_encoding {
            req = req.header(header::ACCEPT_ENCODING, encoding);
        }
        router(dir)
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .expect("router is infallible")
    }

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn brotli_capable_clients_get_the_precompressed_sibling() {
        let dir = dist("br-sibling");
        let response = get(&dir, "/app-0123456789abcdef_bg.wasm", Some("br")).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(
            response.headers()[header::VARY]
                .to_str()
                .unwrap()
                .to_ascii_lowercase(),
            "accept-encoding"
        );
        assert_eq!(body_string(response).await, "brotli-wasm-bytes");
    }

    #[tokio::test]
    async fn clients_without_brotli_get_the_identity_file() {
        let dir = dist("identity");
        let response = get(&dir, "/app-0123456789abcdef_bg.wasm", None).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(body_string(response).await, "identity-wasm");
    }

    /// No `.br` sibling (a plain local `trunk build` dist): serve the original
    /// rather than 404.
    #[tokio::test]
    async fn missing_sibling_falls_back_to_the_identity_file() {
        let dir = dist("no-sibling");
        let response = get(&dir, "/styles-fedcba9876543210.css", Some("br, gzip")).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(body_string(response).await, "body{}");
    }

    #[tokio::test]
    async fn fingerprinted_assets_are_served_immutable() {
        let dir = dist("immutable-header");
        let response = get(&dir, "/app-0123456789abcdef_bg.wasm", Some("br")).await;

        assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE);
    }

    /// SPA routes fall through to index.html, which must never be pinned.
    #[tokio::test]
    async fn spa_routes_serve_index_and_revalidate() {
        let dir = dist("spa-fallback");
        let response = get(&dir, "/links", None).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], REVALIDATE);
        assert_eq!(body_string(response).await, "<html>index</html>");
    }
```

Keep the three `cache_control_for` tests from Task 2 in this same module (they
already `use super::*;`), and close the module with `}`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo nextest run -p chaos-server static_assets`

Expected: FAIL to compile — `cannot find function 'router' in this scope`.

- [ ] **Step 4: Implement the static router**

Add to the top of `crates/chaos-server/src/api/static_assets.rs`, above
`IMMUTABLE`:

```rust
use std::path::Path;

use axum::Router;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, VARY};
use axum::middleware::Next;
use axum::response::Response;
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
```

And below `cache_control_for`, add:

```rust
/// Stamp the cache policy for this path, plus the `Vary` that keeps a shared
/// cache from handing a brotli body to a client that never asked for one
/// (`ServeDir` sets `Content-Encoding` but not `Vary`).
async fn cache_headers(request: Request, next: Next) -> Response {
    let policy = cache_control_for(request.uri().path());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(policy));
    headers.insert(VARY, HeaderValue::from_static("accept-encoding"));
    response
}

/// Serves the built frontend out of `dir`, preferring the `.br`/`.gz` siblings
/// that `packages.chaos-web-static` generates at build time (see flake.nix) and
/// falling back to the identity file when they are absent — which is the case
/// for a local `just build-web` dist. Unknown paths get `index.html` so the
/// client-side router owns deep links.
pub(crate) fn router(dir: &Path) -> Router {
    let index = ServeFile::new(dir.join("index.html"))
        .precompressed_br()
        .precompressed_gzip();
    let files = ServeDir::new(dir)
        .precompressed_br()
        .precompressed_gzip()
        .fallback(index);

    Router::new().fallback_service(
        ServiceBuilder::new()
            .layer(axum::middleware::from_fn(cache_headers))
            .service(files),
    )
}
```

- [ ] **Step 5: Wire it into the API router**

In `crates/chaos-server/src/api/mod.rs`, replace the static-serving block
(currently lines 81-86):

```rust
    // Serve the built web frontend when configured (production mode). During
    // development trunk serves it instead and proxies /api here.
    if let Some(dir) = &state.config.static_dir {
        let index = dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
    }
```

with:

```rust
    // Serve the built web frontend when configured (production mode). During
    // development trunk serves it instead and proxies /api here.
    if let Some(dir) = &state.config.static_dir {
        app = app.merge(static_assets::router(dir));
    }
```

Then drop the now-unused import on line 18:

```rust
use tower_http::services::{ServeDir, ServeFile};
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo nextest run -p chaos-server static_assets`
Expected: 8 tests pass.

Run: `just check && just test`
Expected: fmt clean, clippy clean, wasm check clean, full suite green.

- [ ] **Step 7: Manual smoke test against a real dist**

```bash
just build-web
printf 'x' | gzip -9 -c > /dev/null   # sanity: gzip exists
brotli -q 11 -k -f crates/chaos-web/dist/*_bg.wasm
CHAOS_CONFIG=crates/chaos-server/chaos.example.toml \
  cargo run -p chaos-server &
sleep 5
WASM=$(basename crates/chaos-web/dist/*_bg.wasm)
curl -sI -H 'Accept-Encoding: br' "http://127.0.0.1:4600/$WASM" | grep -iE 'content-encoding|content-length|cache-control|vary'
kill %1
```

Expected: `content-encoding: br`, a `content-length` near 866 000 (not
4 761 056), `cache-control: public, max-age=31536000, immutable`, and a `vary`
containing `accept-encoding`.

Note: `chaos.example.toml` must have `static_dir` pointing at
`crates/chaos-web/dist`; if it does not, run with
`CHAOS_STATIC_DIR=crates/chaos-web/dist` instead (figment reads `CHAOS_`-prefixed
env overrides).

Then clean up the hand-made sibling so it does not get committed:

```bash
rm -f crates/chaos-web/dist/*_bg.wasm.br
```

- [ ] **Step 8: Commit**

```bash
git add crates/chaos-server/Cargo.toml crates/chaos-server/src/api/static_assets.rs \
        crates/chaos-server/src/api/mod.rs Cargo.lock
git -c commit.gpgsign=false commit -m "perf(server): serve precompressed assets and cache fingerprinted files" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 4: `chaos-web-static` — generate the compressed siblings at build time

Kept separate from `chaos-web` on purpose: `chaos-desktop` copies that dist into
its binary via `generate_context!` (`flake.nix:157-159`), where `.br`/`.gz`
files would add ~1 MB the Tauri asset protocol never serves.

**Files:**
- Modify: `flake.nix:109-139` (after the `chaos-web` derivation) and the
  `packages` set at `flake.nix:186`
- Modify: `nix/module.nix:37-42`

- [ ] **Step 1: Add the derivation**

In `flake.nix`, immediately after the closing `};` of the `chaos-web`
derivation (the line after `meta.description = "chaos web frontend (static
trunk dist)";`), insert:

```nix
    # The dist as chaos-server serves it: brotli/gzip siblings generated once
    # here so ServeDir answers compressed requests without re-compressing
    # megabytes of wasm per request (see api/static_assets.rs). Separate from
    # chaos-web because chaos-desktop bakes that dist into its binary and the
    # APK, where these files would be dead weight.
    chaos-web-static = pkgs.runCommand "chaos-web-static-${version}" {
      nativeBuildInputs = [pkgs.brotli pkgs.gzip];
      meta.description = "chaos web frontend, with precompressed assets";
    } ''
      cp -r ${chaos-web} $out
      chmod -R u+w $out
      find $out -type f \( -name '*.wasm' -o -name '*.js' -o -name '*.css' \
        -o -name '*.html' -o -name '*.json' -o -name '*.svg' -o -name '*.map' \) \
        -print0 | while IFS= read -r -d "" f; do
        brotli -q 11 -f -o "$f.br" "$f"
        gzip -9 -c "$f" > "$f.gz"
      done
    '';
```

- [ ] **Step 2: Export the package**

`flake.nix:186` currently reads:

```nix
      inherit chaos-server chaos-web chaos-desktop;
```

Change it to:

```nix
      inherit chaos-server chaos-web chaos-web-static chaos-desktop;
```

- [ ] **Step 3: Point the NixOS module at it**

In `nix/module.nix`, replace the `webPackage` option (lines 37-42):

```nix
    webPackage = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.chaos-web-static;
      defaultText = lib.literalExpression "chaos.packages.\${system}.chaos-web-static";
      description = ''
        Built web frontend served by the server (null to disable). Defaults to
        the variant carrying .br/.gz siblings, which the server prefers over
        recompressing on every request.
      '';
    };
```

- [ ] **Step 4: Verify the flake evaluates and builds**

```bash
git add flake.nix nix/module.nix
nix flake check --no-build 2>&1 | tail -5
nix build .#chaos-web-static
ls -l result/ | head -20
```

Expected: `nix flake check --no-build` reports no errors; `result/` contains the
dist plus `*_bg.wasm.br`, `*_bg.wasm.gz`, `*.js.br`, `*.css.br` and friends. The
`.br` for the wasm should be roughly 866 000 bytes (before Task 6 shrinks the
wasm itself).

- [ ] **Step 5: Verify the desktop dist stays clean**

```bash
nix build .#chaos-desktop
```

Expected: build succeeds. (The desktop derivation copies `${chaos-web}`, not the
compressed variant — confirm by re-reading `flake.nix:157-159`; it must still
say `cp -r ${chaos-web} crates/chaos-web/dist`.)

- [ ] **Step 6: Verify the system config still evaluates**

```bash
nix build /etc/nixos#nixosConfigurations.zeus.config.system.build.toplevel --no-link 2>&1 | tail -3
```

Expected: builds. This is the consumer of `webPackage`; the flake input there is
pinned, so the check proves the module change is at worst inert until the input
is bumped. Do **not** commit or rebuild anything in `/etc/nixos`.

- [ ] **Step 7: Commit**

```bash
git add flake.nix nix/module.nix
git -c commit.gpgsign=false commit -m "perf(nix): precompress the served web dist into chaos-web-static" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 5: Lazy-load the ECharts bundle

Today `<script src="/vendor/echarts.min.js">` blocks parsing on every page for
1 010 KB that only the Home and Weather tabs use. It becomes a memoized loader
that `ChartCanvas` awaits.

**Files:**
- Modify: `crates/chaos-web/index.html:15-16`
- Modify: `crates/chaos-ui/src/echarts.rs`

- [ ] **Step 1: Replace the blocking script with a loader**

In `crates/chaos-web/index.html`, delete these two lines:

```html
    <!-- Self-hosted ECharts (Home tab chart); pinned, no CDN. -->
    <script src="/vendor/echarts.min.js"></script>
```

and add, as the first thing inside the existing `<script>` block (immediately
after `<script>`, before `// Shared tooltip header for time-axis charts:`):

```js
      // Self-hosted ECharts (Home and Weather charts); pinned, no CDN. It is
      // ~1 MB, so it loads on demand instead of blocking every page — the
      // dashboard never pays for it. The promise is memoized, so N charts
      // mounting together share one request. A rejection is sticky: a failed
      // load stays failed for the session and ChartCanvas shows its
      // "Chart failed to load" message.
      window.chaosLoadECharts = function () {
        if (!window.chaosEChartsPromise) {
          window.chaosEChartsPromise = new Promise(function (resolve, reject) {
            var script = document.createElement("script");
            script.src = "/vendor/echarts.min.js";
            script.onload = function () { resolve(true); };
            script.onerror = function () { reject(new Error("echarts bundle failed to load")); };
            document.head.appendChild(script);
          });
        }
        return window.chaosEChartsPromise;
      };
```

- [ ] **Step 2: Bind the loader in Rust**

In `crates/chaos-ui/src/echarts.rs`, update the doc comment at the top of the
file:

```rust
//! Minimal bindings to the vendored Apache ECharts bundle (loaded on demand by
//! `chaosLoadECharts` in index.html). Provides reusable chart bindings plus a
//! `ChartCanvas` component used by both the Home and Weather tabs — options are
//! passed as JSON built with serde_json and parsed on the JS side.
```

and add this import declaration inside the existing
`#[wasm_bindgen] extern "C" { ... }` block, directly above `pub type EChart;`:

```rust
    /// `window.chaosLoadECharts()` — fetch the vendored bundle, memoized in JS
    /// so concurrent charts share one request. `catch` also covers the function
    /// being absent (a shell serving a stale index.html), which surfaces as a
    /// load failure rather than a panic.
    #[wasm_bindgen(js_name = chaosLoadECharts, catch)]
    async fn load_echarts() -> Result<JsValue, JsValue>;
```

- [ ] **Step 3: Gate `ChartCanvas` on the loaded bundle**

In `crates/chaos-ui/src/echarts.rs`, inside `ChartCanvas`, the declarations
currently end with:

```rust
    let zoomed = StoredValue::new_local(false);
    let failed = RwSignal::new(false);
```

Add after them:

```rust
    // The bundle arrives on demand; until it does there is nothing to init
    // against. One await per chart, one request per session (memoized in JS).
    let ready = RwSignal::new(false);
    leptos::task::spawn_local(async move {
        match load_echarts().await {
            Ok(_) => ready.set(true),
            Err(_) => failed.set(true),
        }
    });
```

Then make the mount effect wait for it — the effect currently starts:

```rust
    Effect::new(move |_| {
        let Some(el) = node.get() else {
            return;
        };
```

Change it to:

```rust
    Effect::new(move |_| {
        // Tracked: the effect re-runs once the bundle has loaded.
        if !ready.get() {
            return;
        }
        let Some(el) = node.get() else {
            return;
        };
```

- [ ] **Step 4: Verify it compiles for both targets**

Run: `just check`
Expected: fmt clean, clippy clean, and `cargo check -p chaos-web -p chaos-ui
--target wasm32-unknown-unknown` clean.

Run: `cargo nextest run -p chaos-ui`
Expected: the existing `inside_zoom_has_the_shared_gesture_flags` test still
passes.

- [ ] **Step 5: Verify in a browser**

```bash
just build-web
CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server
```

With devtools' Network tab open at `http://127.0.0.1:4600`:

1. Load the dashboard → **no** request for `/vendor/echarts.min.js`.
2. Open the Home tab → exactly **one** request for it; the chart renders.
3. Navigate to Weather and back → still exactly one request total; charts render.

Per the memory note "verify UI in headless browser": if no interactive browser
is available, drive it with geckodriver from nix
(`nix-shell -p geckodriver firefox`) against the running server and assert the
same three points from the performance/network log.

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-web/index.html crates/chaos-ui/src/echarts.rs
git -c commit.gpgsign=false commit -m "perf(ui): load the echarts bundle on demand instead of blocking every page" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 6: Optimize the wasm (wasm-opt + the `wasm-release` profile)

`[profile.wasm-release]` (`Cargo.toml:74-78`) has never been used: nothing
passes it to trunk. And the shipped wasm still carries a 724 KB `name` section,
so `wasm-opt` is not stripping. Both are trunk asset attributes on the rust
link, which matters — trunk computes the SRI hash *after* running wasm-opt, so a
post-build hook would produce an integrity mismatch and a page that refuses to
boot.

**Files:**
- Modify: `crates/chaos-web/index.html` (add a `rel="rust"` link)

- [ ] **Step 1: Record the baseline**

```bash
just build-web
ls -l crates/chaos-web/dist/*_bg.wasm
nix-shell -p wabt --run "wasm-objdump -h crates/chaos-web/dist/*_bg.wasm" | grep -E 'Custom|Code'
```

Expected: ~4 769 951 bytes, with `Custom ... "name"` (~724 KB) and
`Custom ... "producers"` sections present. Write both numbers down.

- [ ] **Step 2: Add the rust link with the optimization attributes**

In `crates/chaos-web/index.html`, add directly below the
`<link data-trunk rel="css" href="styles.css" />` line:

```html
    <!-- Explicit rust link so release builds get the size-tuned profile
         (opt-level "z" + panic=abort, see [profile.wasm-release] in
         Cargo.toml) and a stripped wasm-opt pass. It has to run through
         trunk rather than a post-build hook: trunk computes the SRI
         integrity hash after wasm-opt, and a hook rewriting the file
         afterwards would make the browser refuse to boot it. -->
    <link data-trunk rel="rust" data-cargo-profile-release="wasm-release"
          data-wasm-opt="z" data-wasm-opt-params="--strip-debug --strip-producers" />
```

- [ ] **Step 3: Rebuild and measure**

```bash
just build-web
ls -l crates/chaos-web/dist/*_bg.wasm
nix-shell -p wabt --run "wasm-objdump -h crates/chaos-web/dist/*_bg.wasm" | grep -cE 'Custom.*(name|producers)'
nix-shell -p brotli --run "brotli -q 11 -c crates/chaos-web/dist/*_bg.wasm | wc -c"
```

Expected: the wasm is materially smaller than the Step 1 baseline (the
measured floor for `-Oz --strip-debug` alone was 3 594 251 bytes / 783 662
brotli; `opt-level = "z"` should land under that), the `grep -c` prints `0`, and
the brotli figure is below 783 662.

If trunk fails with an unknown-argument error from wasm-opt,
`data-wasm-opt-params` replaced rather than appended trunk's own flags: drop
that attribute, keep `data-wasm-opt="z"`, re-run, and note in the commit body
that the `name` section survives.

- [ ] **Step 4: Verify the app still boots**

```bash
CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server
```

Load `http://127.0.0.1:4600`: the dashboard renders, no console errors (an SRI
failure shows as "Failed to find a valid digest in the integrity attribute"),
and switching to the Home tab still draws a chart. `panic = "abort"` means a
Rust panic no longer unwinds — `console_error_panic_hook` still reports it, so
check the console is clean.

- [ ] **Step 5: Verify through nix too**

```bash
git add crates/chaos-web/index.html
nix build .#chaos-web-static
ls -l result/*_bg.wasm result/*_bg.wasm.br
```

Expected: matches the Step 3 sizes — the sandboxed build gets the same
attributes.

- [ ] **Step 6: Commit**

```bash
git add crates/chaos-web/index.html
git -c commit.gpgsign=false commit -m "perf(web): build the wasm with the size profile and a stripped wasm-opt pass" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 7: Boot skeleton

`<body>` is empty until the wasm instantiates, so even at ~700 KB there is a
white screen. An inline skeleton paints immediately; `mount_to_body` appends
next to it, so `main.rs` removes it once mounted.

**Files:**
- Modify: `crates/chaos-web/index.html` (inline `<style>` in `<head>`, markup in `<body>`)
- Modify: `crates/chaos-web/src/main.rs:47-58`
- Modify: `crates/chaos-web/Cargo.toml`

- [ ] **Step 1: Add the inline skeleton styles**

In `crates/chaos-web/index.html`, add directly above the closing `</head>`
(after the existing `<script>` block):

```html
    <!-- Painted before the wasm boots; main.rs drops it after mount. The
         colors are inlined from styles.css (--bg, --border, --muted,
         --accent) because that sheet may not have arrived yet. -->
    <style>
      #chaos-boot {
        position: fixed;
        inset: 0;
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        gap: 1.25rem;
        background: #14161c;
        color: #8a90a0;
        font-family: system-ui, sans-serif;
      }
      #chaos-boot-bar {
        width: 9rem;
        height: 3px;
        border-radius: 3px;
        background: #2a2e39;
        overflow: hidden;
      }
      #chaos-boot-bar span {
        display: block;
        width: 40%;
        height: 100%;
        border-radius: 3px;
        background: #7c9aff;
        animation: chaos-boot-slide 1.1s ease-in-out infinite;
      }
      @keyframes chaos-boot-slide {
        from { transform: translateX(-100%); }
        to { transform: translateX(250%); }
      }
      @media (prefers-reduced-motion: reduce) {
        #chaos-boot-bar span { animation: none; width: 100%; }
      }
    </style>
```

- [ ] **Step 2: Add the skeleton markup**

Replace `<body></body>` in `crates/chaos-web/index.html` with:

```html
  <body>
    <div id="chaos-boot">
      <img src="/assets/logo.svg" alt="" width="64" height="64" />
      <div id="chaos-boot-bar"><span></span></div>
    </div>
  </body>
```

- [ ] **Step 3: Add the web-sys features the removal needs**

In `crates/chaos-web/Cargo.toml`, replace:

```toml
web-sys = { workspace = true, features = ["Window", "Location", "Storage"] }
```

with:

```toml
web-sys = { workspace = true, features = ["Window", "Location", "Storage", "Document", "Element"] }
```

- [ ] **Step 4: Remove the skeleton after mounting**

In `crates/chaos-web/src/main.rs`, the `main` function currently ends with:

```rust
    mount_to_body(move || view! { <App config=config.clone()/> });
```

Replace that line with:

```rust
    mount_to_body(move || view! { <App config=config.clone()/> });
    // The boot skeleton (index.html) paints while the wasm loads; mount_to_body
    // appends the app beside it rather than replacing it, so drop it now.
    if let Some(node) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("chaos-boot"))
    {
        node.remove();
    }
```

- [ ] **Step 5: Verify it compiles**

Run: `just check`
Expected: fmt clean, clippy clean, wasm check clean.

- [ ] **Step 6: Verify in a browser**

```bash
just build-web
CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server
```

Load `http://127.0.0.1:4600` with devtools throttled to "Slow 3G": the logo and
the sliding bar appear well before the app, and vanish the moment the dashboard
renders — no leftover `#chaos-boot` in the inspected DOM, no white flash.

- [ ] **Step 7: Commit**

```bash
git add crates/chaos-web/index.html crates/chaos-web/src/main.rs crates/chaos-web/Cargo.toml
git -c commit.gpgsign=false commit -m "feat(web): paint a boot skeleton while the wasm loads" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

---

### Task 8: End-to-end verification and documentation

**Files:**
- Modify: `docs/ARCHITECTURE.md` (the `## Frontend` section, lines 59-66)

- [ ] **Step 1: Measure the whole page as served**

```bash
nix build .#chaos-web-static
CHAOS_STATIC_DIR=$(readlink -f result) \
CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server &
sleep 5
for f in $(curl -s http://127.0.0.1:4600/ | grep -oE '/[a-zA-Z0-9_./-]+\.(wasm|js|css)' | sort -u); do
  echo -n "$f "
  curl -sI -H 'Accept-Encoding: br' "http://127.0.0.1:4600$f" \
    | grep -iE 'content-length|content-encoding' | tr '\n' ' '
  echo
done
kill %1
```

Expected: the wasm and the bindgen glue both report `content-encoding: br` with
a combined `content-length` under 800 000, and `/vendor/echarts.min.js` does not
appear in the index at all (it is injected at runtime now). Record the total —
it is the headline number for the final report.

- [ ] **Step 2: Confirm the full suite is green**

Run: `just check && just test`
Expected: all clean.

- [ ] **Step 3: Document the delivery decisions**

In `docs/ARCHITECTURE.md`, append to the `## Frontend` section, after the
"API base resolution" bullet:

```markdown
- Delivery: the dist is precompressed at build time (`packages.chaos-web-static`
  adds `.br`/`.gz` siblings) and served by `ServeDir::precompressed_br`, with
  `Cache-Control: immutable` on trunk's fingerprinted filenames and `no-cache`
  on everything else (`chaos-server/src/api/static_assets.rs`). Compressing per
  request instead would mean re-brotli-ing megabytes of wasm on every cold load;
  `chaos-desktop` keeps consuming the uncompressed `chaos-web` because it bakes
  that dist into its binary.
- The wasm is built with `[profile.wasm-release]` and a stripped `wasm-opt -Oz`
  pass, both wired through `data-*` attributes on the `rel="rust"` link in
  `index.html` — trunk computes the SRI integrity hash after wasm-opt, so a
  post-build hook would break booting.
- ECharts (~1 MB) loads on demand via `chaosLoadECharts` rather than blocking
  every page; only the Home and Weather tabs pay for it.
```

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git -c commit.gpgsign=false commit -m "docs: record the frontend delivery and wasm size decisions" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01L88hCp5gyGDgJV3tcARSCP"
```

- [ ] **Step 5: Report the before/after**

Summarize, with the real measured numbers from Task 1 Step 2, Task 6 Step 3 and
Task 8 Step 1:

- wasm: baseline bytes → optimized bytes → brotli bytes
- dashboard cold load total, before and after
- whether `data-wasm-opt-params` had to be dropped
- that deploying needs the user to bump the chaos flake input in `/etc/nixos`
  and `nixos-rebuild switch` (this session cannot: sudo needs a password, and
  `/etc/nixos` commits are theirs to make)

---

## Self-review

**Spec coverage:**

| Spec section | Task |
| --- | --- |
| 1. Precompressed static assets — build side | Task 4 |
| 1. Precompressed static assets — serve side | Task 3 |
| 2. Cache-Control for content-hashed assets | Tasks 2, 3 |
| 3. wasm-opt and the wasm-release profile | Task 6 |
| 4. Lazy ECharts | Task 5 |
| 5. Boot skeleton | Task 7 |
| Testing — unit + integration | Tasks 2, 3 |
| Testing — manual verification | Tasks 4, 5, 6, 7, 8 |
| Risk: `data-wasm-opt-params` may replace flags | Task 6 Step 3 fallback |
| Risk: `immutable` on a wrong file | Task 2 tests (vendor, assets, uuid) |

**Ordering note:** Task 5 (lazy ECharts) precedes Task 6 (wasm-opt) so that the
slow, measured release build in Task 6 happens once, after the last change that
affects wasm contents.

**Type consistency:** `cache_control_for(&str) -> &'static str`, `cache_headers`
(axum `from_fn` middleware) and `router(&Path) -> Router` are named identically
in Tasks 2, 3 and the `merge` call site. `load_echarts()` in `echarts.rs` binds
`window.chaosLoadECharts` defined in Task 5 Step 1; the `ready`/`failed` signals
used in Step 3 are both declared before use.
