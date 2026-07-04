# ADR 0001 — UI stack: Leptos (CSR) + trunk + Tauri v2

Date: 2026-07-04 · Status: accepted

## Context

The app must be both the browser start page served on the LAN and a desktop
application, written in Rust, split into crates, with minimal future refactoring.
Candidates: Leptos + trunk + Tauri, or Dioxus 0.7 (which bundles web/desktop/mobile
behind one CLI).

## Decision

Leptos 0.8 in **CSR mode** built by trunk; the identical static bundle is served by
axum (web) and embedded by Tauri v2 (desktop).

## Consequences

- One frontend artifact for both delivery modes; the server needs no wasm toolchain.
- SSR/islands are deliberately not used — adopting them later would mean revisiting
  the delivery model, so the dashboard is designed to be fine as a SPA.
- Tauri is a second toolchain to care about (webkitgtk stack on NixOS), isolated in
  `chaos-desktop` + the flake devshell.
- `wasm-bindgen-cli` must exactly match the `wasm-bindgen` crate version; the flake
  derives it from `Cargo.lock` so they cannot drift.
