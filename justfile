# Development commands. Run inside `nix develop` (or with direnv active).

default:
    @just --list

# Run the backend with the example config (http://127.0.0.1:4600)
server:
    CHAOS_CONFIG=crates/chaos-server/chaos.example.toml cargo run -p chaos-server

# Serve the frontend with hot reload on http://127.0.0.1:8080 (run `just server` in another terminal)
web:
    cd crates/chaos-web && trunk serve

# Run the desktop shell against a server (rebuilds the web dist first)
desktop server="http://127.0.0.1:4600":
    cd crates/chaos-web && trunk build
    CHAOS_SERVER={{server}} cargo run -p chaos-desktop

# Build the desktop bundle (deb; NixOS installs use the flake package)
bundle:
    cd crates/chaos-web && trunk build --release
    cd crates/chaos-desktop && cargo tauri build

# Build the signed Android APK. Enters the .#android dev shell itself
# (Android SDK/NDK + JDK come from nix, not ~/Android).
apk:
    cd crates/chaos-web && trunk build --release
    nix develop .#android --command sh -c 'cd crates/chaos-desktop && cargo tauri android build --apk --target aarch64'

# Build the signed APK and attach it to the GitHub release of the
# current workspace version. Runs locally because the release keystore
# never leaves this machine (CI only publishes the nix-built artifacts).
release-apk: apk
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
    cp crates/chaos-desktop/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk \
       "chaos-${version}.apk"
    gh release upload "v${version}" "chaos-${version}.apk" --clobber
    rm "chaos-${version}.apk"

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

# Run the test suite (nextest: parallel test execution, same as CI)
test:
    cargo nextest run --workspace
