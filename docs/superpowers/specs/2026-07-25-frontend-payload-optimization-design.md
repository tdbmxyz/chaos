# Frontend payload optimization — design

**Date:** 2026-07-25
**Status:** approved

## Problem

Opening the dashboard downloads ~5.9 MB before anything is painted. Measured
against the live server (`curl -sI http://localhost/<asset>`, 2026-07-25):

| asset | served | brotli -q 11 |
| --- | --- | --- |
| `chaos-web-*_bg.wasm` | 4 761 056 B | 865 858 B |
| `vendor/echarts.min.js` | 1 034 102 B | 271 456 B |
| `chaos-web-*.js` (bindgen glue) | 60 830 B | ~18 000 B |
| `styles-*.css` | 39 000 B | ~8 000 B |
| **total** | **~5.9 MB** | **~1.16 MB** |

Four independent causes, all confirmed on the deployed artifacts:

1. **Nothing is compressed.** `chaos-server` builds its static service as
   `ServeDir::new(dir).fallback(ServeFile::new(index))`
   (`crates/chaos-server/src/api/mod.rs:83-86`) with no compression, and
   traefik has no `compress` middleware. Five sixths of the transfer is
   compressible air.
2. **`wasm-opt` output is not stripped.** The shipped wasm still carries a
   724 KB `name` custom section and a `producers` section
   (`wasm-objdump -h`). Running `wasm-opt -Oz --strip-debug --strip-producers`
   on the exact deployed file takes 3.4 s and yields 4 769 951 → 3 594 251 B
   (783 662 B brotli).
3. **`[profile.wasm-release]` is dead config.** `Cargo.toml:74-78` defines
   `opt-level = "z"` / `panic = "abort"`, but nothing passes that profile to
   trunk — not `justfile:52`, not `flake.nix:128` — so release builds use
   `[profile.release]` (`opt-level = 3`, `lto = true`).
4. **ECharts is a blocking 1 MB `<script>` in `<head>`**
   (`crates/chaos-web/index.html:16`), parsed on every page load though only
   the Home and Weather tabs use it.

Plus a smaller one: assets are content-hashed but served with no
`Cache-Control`, only `last-modified: Thu, 01 Jan 1970 00:00:01 GMT` (the nix
store timestamp). Conditional requests do return 304, but every force-reload
refetches all 5.9 MB and each navigation costs a revalidation round trip.

Nothing is painted until the wasm boots, because `<body>` is empty and
`mount_to_body` only runs after instantiation.

## Goals

- Cut the cold-load transfer for the dashboard from ~5.9 MB to well under 1 MB.
- Paint something immediately instead of a white screen.
- Keep ECharts off the dashboard path entirely (Home/Weather pay for it).
- Change nothing in `/etc/nixos`; every change lands in this repo.
- No regression for the Tauri desktop shell or the Android APK, which bake the
  same dist into the binary and must not grow.

## Non-goals

- Shrinking the Rust side by auditing dependencies (`reqwest`-wasm, `chrono`,
  `url`, `serde_json`) with `twiggy`. Worth doing later; much larger project.
- Code-splitting the wasm per route. Leptos CSR has no supported story for it.
- Server-side rendering.
- Runtime (per-request) compression. See the decision below.

## Design

### 1. Precompressed static assets

Compression happens once at build time, not per request.

**Why not runtime compression:** a `tower_http::CompressionLayer` would
re-brotli 4.7 MB on every cold request. At brotli quality 11 that is ~2 s of
CPU per request; at a cheap quality it gives up most of the win. Precompressing
once in the nix build costs nothing at serve time and lets us use `-q 11`.

**Why not traefik's `compress` middleware:** it only covers the public domain.
LAN clients and the Tauri/Android shells talk to `chaos-server` directly and
would stay uncompressed. It also lives in `/etc/nixos`, which this work must
not touch.

**Build side.** A new flake package `chaos-web-static` wraps the existing
`chaos-web` dist and adds `.br` + `.gz` siblings for every compressible file
(`.wasm .js .css .html .json .svg .map`):

```
chaos-web         → trunk dist, unchanged (consumed by chaos-desktop)
chaos-web-static  → chaos-web + *.br + *.gz (consumed by the NixOS module)
```

The split matters: `chaos-desktop` copies the dist into the binary via
`generate_context!` (`flake.nix:158-159`), so adding compressed siblings there
would bloat the desktop binary and the APK by ~1 MB of files Tauri's asset
protocol never serves. `services.chaos.webPackage` defaults to
`chaos-web-static` instead of `chaos-web`.

**Serve side.** `ServeDir` and the `ServeFile` fallback get
`.precompressed_br().precompressed_gzip()`. tower-http picks the encoding from
`Accept-Encoding`, falls back to the identity file when no sibling exists (so
local `trunk build` dists and `just server` keep working), and sets
`Content-Encoding` + `Vary: accept-encoding` itself.

Subresource integrity is unaffected: browsers verify SRI hashes against the
decoded bytes, and trunk computes them after `wasm-opt` runs.

### 2. Cache-Control for content-hashed assets

Trunk emits content-hashed filenames (`chaos-web-<16 hex>_bg.wasm`,
`styles-<16 hex>.css`), which are safe to cache forever. `index.html` is not.

A thin middleware over the static service sets, based on the request path:

- hashed asset (`-<16+ hex>` immediately before the extension) →
  `public, max-age=31536000, immutable`
- everything else (`index.html`, `/vendor/*`, `/assets/*`) →
  `no-cache` (still revalidates, still gets its 304)

The path → header decision is a pure function so it can be unit-tested without
a server. `/vendor/echarts.min.js` is deliberately in the `no-cache` bucket: it
is version-pinned by hand, not by filename.

### 3. wasm-opt and the wasm-release profile

Both are wired through trunk's asset attributes on an explicit rust link in
`crates/chaos-web/index.html`, so trunk hashes and SRI-signs the *optimized*
file:

```html
<link data-trunk rel="rust" data-cargo-profile-release="wasm-release"
      data-wasm-opt="z" data-wasm-opt-params="--strip-debug --strip-producers" />
```

A post-build hook is explicitly wrong here — trunk computes integrity hashes
before hooks run, so a hook that rewrites the wasm produces an SRI mismatch and
a page that refuses to boot.

`data-cargo-profile-release` applies to release builds only, so `trunk serve`
dev builds keep their fast profile. The `wasm-release` profile already exists
and needs no edit.

### 4. Lazy ECharts

The blocking `<script src="/vendor/echarts.min.js">` is replaced by a memoized
loader function in the existing inline script block:

```js
window.chaosLoadECharts = function () { /* injects the <script> once, returns a Promise */ }
```

`ChartCanvas` (`crates/chaos-ui/src/echarts.rs`) awaits it before
`echarts.init`. The loader is memoized in JS, so N charts mounting at once
trigger exactly one fetch. A load failure surfaces through the existing
`failed` signal and its "Chart failed to load" message — the page keeps
working, which is the current behavior when the bundle is missing.

Keeping the loader in JS (rather than injecting a `<script>` from Rust and
wiring up promise plumbing) matches the file's existing pattern: the tooltip
formatters already live in that inline block and are reached by name.

### 5. Boot skeleton

`index.html` gets a `<div id="chaos-boot">` holding the logo and a CSS
spinner, styled by an inline `<style>` in `<head>` so it paints before
`styles-*.css` arrives. Colors are hardcoded to the theme values from
`crates/chaos-web/styles.css:2-12` (`--bg: #14161c`, `--muted: #8a90a0`)
because the stylesheet may not have loaded yet.

`mount_to_body` appends to `<body>`, so `crates/chaos-web/src/main.rs` removes
the node by id immediately after mounting.

## Expected result

| | before | after |
| --- | --- | --- |
| wasm | 4.76 MB | ~0.7 MB br (3.6 MB → strip+`opt-level="z"`, then brotli) |
| bindgen glue | 60 KB | ~18 KB br |
| css | 39 KB | ~8 KB br |
| echarts | 1.01 MB, blocking | 0 on the dashboard; ~271 KB br on Home/Weather |
| **dashboard cold load** | **~5.9 MB** | **~0.73 MB** |
| repeat load | full revalidation | `immutable`, no request |
| first paint | after wasm boots | immediate skeleton |

The wasm figure is a projection: 783 662 B is measured for
`-Oz --strip-debug` + brotli on the current `opt-level = 3` build, and
`opt-level = "z"` + `panic = "abort"` should take it below that. The plan
records the real number.

## Testing

Unit + integration tests in `crates/chaos-server` (`cargo nextest run`):

- `cache_control_for` — hashed vs unhashed vs index paths (pure function).
- Static serving with a temp dist containing `app-<hash>.wasm` and its `.br`
  sibling: `Accept-Encoding: br` → `content-encoding: br` and the compressed
  bytes; no `Accept-Encoding` → identity bytes, no `content-encoding`; missing
  sibling → identity.
- Cache-Control on both a hashed asset and `index.html` through the real
  router.

Manual verification (no automated coverage possible):

- `nix build .#chaos-web-static` → `.br`/`.gz` siblings exist; `nix build
  .#chaos-desktop` → dist inside has none.
- Release build → `wasm-objdump -h` shows no `name`/`producers` section, and
  the wasm is smaller than the recorded baseline.
- Dashboard load in a browser: skeleton paints, no `echarts.min.js` request;
  open Home → exactly one `echarts.min.js` request, chart renders; navigate
  away and back → no second request.

## Risks

- **`data-wasm-opt-params` may replace trunk's own wasm-opt arguments** rather
  than append (including feature flags like `--enable-reference-types`). If the
  build fails, drop the params attribute and keep `data-wasm-opt="z"`; the
  `name` section survives but everything else still applies. A manual
  `wasm-opt -Oz --strip-debug` on the real artifact already succeeded, so
  binaryen itself handles this module fine.
- **`opt-level = "z"` can cost runtime speed.** The profile is the repo's own
  existing choice. If the dashboard feels sluggish afterwards, `"s"` is the
  middle setting; the plan records the size delta so the trade is visible.
- **`immutable` on a wrong file would pin a stale asset for a year.** Mitigated
  by requiring a content hash in the filename and by unit-testing the
  classifier, including the `index.html` and `/vendor/` cases.
