# Starter: the manga/webtoon application (sibling project)

Decided in [ADR 0002](adr/0002-scope-manga-separate-app.md): the manga reader is its
own application, not part of chaos. This document is its seed — copy it into the new
repository when work starts.

## Vision

A Suwayomi/Tachidesk-Sorayomi-style system: server-side library hosting manga and
webtoons with reading progress tracking, plus clients (web UI and app) for reading —
with a first-class **offline mode**: chapters downloaded to the device remain readable
without the server, progress made offline is tracked locally and **merged back** when
the server is reachable again.

## Recommended architecture

Same skeleton as chaos (proven template):

| Crate | Role |
| --- | --- |
| `domain` | Library/chapter/page/progress types, **sync protocol** types |
| `client` | API client (native + wasm) |
| `server` | Axum backend (or adapter, see below) |
| `ui` | Leptos reader components (web + desktop + later mobile) |
| `web`, `desktop` | Shells, like chaos |
| `store` | **New vs chaos**: client-side persistent storage + sync engine |

Key differences from chaos:

1. **Clients are not stateless.** The `store` crate owns a local database
   (SQLite on native via sqlx; IndexedDB or OPFS-backed SQLite on web) holding
   downloaded chapters and a local progress journal.
2. **Sync engine.** Progress events are append-only facts (`read page P of chapter C
   at time T on device D`), which makes merging trivial: last-write-wins per chapter
   with max(page) tie-break covers the realistic conflicts. Design the journal
   format in `domain` from day one; it is the hard-to-refactor part.
3. **Server strategy: adapter first.** Reimplementing Suwayomi-Server means losing the
   Tachiyomi extension ecosystem (Kotlin APKs). Start with the server crate as a thin
   Rust facade over Suwayomi-Server's GraphQL API (it already handles sources,
   library, downloads); the facade owns the sync journal endpoint. A native library
   backend (own storage, own sources) can replace the facade later behind the same
   `domain` contract. Suwayomi's GraphQL schema is in
   `inspirations/Suwayomi-Server` (chaos repo) for reference.
4. **Mobile matters here.** Tauri v2 supports Android/iOS; the reader UI should be
   designed mobile-first (page turner, prefetch, webtoon vertical scroll).

## First steps (mirroring chaos Phase 0)

1. Copy chaos's flake + workspace layout; rename crates.
2. `domain`: Manga, Chapter, PageRef, ReadProgress, DownloadState, sync journal types.
3. Facade server: proxy Suwayomi GraphQL → REST/JSON contract from `domain`;
   endpoints: library list, chapter list, page image, progress read/write.
4. UI: library grid + basic paged reader against the facade.
5. Then: `store` crate + download manager + journal merge.

## Naming

To be chosen — sibling of `chaos`; something short from the same register
(e.g. `eris`, `nyx`) works.
