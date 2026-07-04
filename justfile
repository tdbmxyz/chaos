# Development commands. Run inside `nix develop` (or with direnv active).

default:
    @just --list

# Run the backend with the example config (http://127.0.0.1:4600)
server:
    CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server

# Serve the frontend with hot reload on http://127.0.0.1:8080 (run `just server` in another terminal)
web:
    cd crates/chaos-web && trunk serve

# Run the desktop app in dev mode (starts trunk itself via beforeDevCommand)
desktop:
    cd crates/chaos-desktop && cargo tauri dev

# Build the production frontend bundle
build-web:
    cd crates/chaos-web && trunk build --release

# Full check: formatting, lints, native + wasm compilation.
# chaos-desktop needs the web dist to exist, hence the placeholder.
check:
    mkdir -p crates/chaos-web/dist && touch crates/chaos-web/dist/index.html
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo check -p chaos-web -p chaos-ui --target wasm32-unknown-unknown

fmt:
    cargo fmt --all
