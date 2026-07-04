# chaos

Unified entry point for my local services: a [glance](https://github.com/glanceapp/glance)-style
dashboard combined with native [Linkwarden](https://github.com/linkwarden/linkwarden)-style link
management, delivered as a web page **and** a desktop app from a single Rust codebase.

## Stack

- **Leptos 0.8** (CSR) for the UI, built with **trunk**
- **Tauri v2** wrapping the same frontend as a desktop app
- **Axum** backend serving the API and the static frontend
- **Nix flake** for the dev environment; NixOS module planned for deployment

## Layout

| Crate | Role |
| --- | --- |
| `chaos-domain` | Shared types + API contract (compiles native & wasm, zero I/O) |
| `chaos-client` | Typed HTTP client for the API (native & wasm via reqwest) |
| `chaos-server` | Axum backend: service monitor, links store, static serving |
| `chaos-ui` | Leptos components/pages, shared by web and desktop |
| `chaos-web` | Trunk entrypoint (index.html, styles, wasm bin) |
| `chaos-desktop` | Tauri v2 shell embedding the built frontend |

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the reasoning,
[docs/ROADMAP.md](docs/ROADMAP.md) for the phases, and `docs/adr/` for recorded decisions.
The manga/webtoon reader is **out of scope** here — it becomes a sibling application, see
[docs/manga-app.md](docs/manga-app.md).

## Development

```console
$ nix develop            # or direnv with `use flake`
$ just server            # backend on http://127.0.0.1:4600 (example config)
$ just web               # frontend with hot reload on http://127.0.0.1:8080
$ just desktop           # Tauri dev window
$ just check             # fmt + clippy + wasm check
```

First checkout only: `Cargo.lock` drives the pinned `wasm-bindgen-cli` in the flake.
If the lock file is missing, enter the shell, run `cargo generate-lockfile`,
`git add Cargo.lock`, and re-enter the shell.

## Deployment

```console
$ nix build .#chaos-server   # backend binary
$ nix build .#chaos-web      # static frontend dist
```

A NixOS module is provided as `nixosModules.chaos` — see
[docs/deployment.md](docs/deployment.md) for the `services.chaos` options and
the recipe replacing the old glance configuration.
