# ADR 0002 — Manga/webtoon reader is a separate application

Date: 2026-07-04 · Status: accepted

## Context

The original vision bundled three things: a glance-like dashboard, Linkwarden-like
links, and a Suwayomi/Sorayomi-like manga system (server-side library + reader client
+ offline downloads with server merge on reconnect).

The manga system has a fundamentally different architecture: it needs client-side
persistent storage, an offline-first sync engine, mobile targets, and either its own
media backend or integration with Suwayomi-Server. Bundling it into chaos would force
chaos's clients to stop being stateless.

## Decision

chaos = dashboard + links only. The manga reader becomes a sibling application built
on the same principles (Rust workspace, Leptos + trunk + Tauri, Nix flake). Its
starting point is documented in [../manga-app.md](../manga-app.md). chaos links to it
like any other service tile.

## Consequences

- chaos clients stay stateless; the server remains the only writer. No sync engine
  in chaos, ever.
- Crate conventions (`*-domain`, `*-client`, `*-ui`, shells) are deliberately
  reusable as the template for the manga app.
