{
  description = "chaos - unified entry point for local services (web + desktop)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    rust-overlay,
  }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs {
      inherit system;
      overlays = [rust-overlay.overlays.default];
      # Android SDK/NDK for the mobile shell
      config.allowUnfree = true;
      config.android_sdk.accept_license = true;
    };
    inherit (pkgs) lib;

    androidNdkVersion = "27.0.12077973";
    androidComposition = pkgs.androidenv.composeAndroidPackages {
      # what the tauri-generated gradle project compiles against
      platformVersions = ["34" "36"];
      buildToolsVersions = ["34.0.0" "35.0.0"];
      includeNDK = true;
      ndkVersion = androidNdkVersion;
    };

    version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

    # Toolchain pinned by rust-toolchain.toml (stable + wasm32 target).
    rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
    rustPlatform = pkgs.makeRustPlatform {
      cargo = rustToolchain;
      rustc = rustToolchain;
    };

    # trunk invokes wasm-bindgen; its CLI version must match the wasm-bindgen
    # crate version in Cargo.lock exactly (wasm-bindgen is not semver-stable).
    # We read the version from the lock file so they can never drift; the two
    # hashes below must be refreshed when the wasm-bindgen version changes
    # (nix will print the expected hash on mismatch).
    #
    # Guarded by pathExists so the shell can bootstrap a fresh checkout:
    # enter it once without Cargo.lock, run `cargo generate-lockfile`,
    # `git add Cargo.lock`, then re-enter.
    hasCargoLock = builtins.pathExists ./Cargo.lock;

    wasm-bindgen-cli = let
      cargoLock = builtins.fromTOML (builtins.readFile ./Cargo.lock);
      wasmBindgen =
        lib.findFirst
        (p: p.name == "wasm-bindgen")
        (throw "wasm-bindgen not found in Cargo.lock")
        cargoLock.package;
    in
      pkgs.buildWasmBindgenCli rec {
        src = pkgs.fetchCrate {
          pname = "wasm-bindgen-cli";
          version = wasmBindgen.version;
          hash = "sha256-H6Is3fiZVxZCfOMWK5dWMSrtn50VGv0sfdnsT+cTtyk=";
        };

        cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
          inherit src;
          inherit (src) pname version;
          hash = "sha256-VucqkXbCi4qtQzY/HrXiDnbSURsagPsdNVMn1Tw3UiY=";
        };
      };

    # Native libraries required by Tauri v2 (webview + GTK stack).
    tauriLibs = with pkgs; [
      webkitgtk_4_1
      gtk3
      libsoup_3
      glib
      cairo
      pango
      gdk-pixbuf
      atk
      librsvg
      openssl
      dbus
    ];

    chaos-server = rustPlatform.buildRustPackage {
      pname = "chaos-server";
      inherit version;
      src = self;

      cargoLock.lockFile = ./Cargo.lock;

      # Only the backend: the desktop crate would drag the webkit stack in.
      cargoBuildFlags = ["-p" "chaos-server"];
      cargoTestFlags = ["-p" "chaos-server"];

      meta = {
        description = "chaos backend: dashboard API, service monitor, link store";
        mainProgram = "chaos-server";
      };
    };

    chaos-web = pkgs.stdenv.mkDerivation {
      pname = "chaos-web";
      inherit version;
      src = self;

      cargoDeps = pkgs.rustPlatform.importCargoLock {lockFile = ./Cargo.lock;};

      nativeBuildInputs = [
        rustToolchain
        pkgs.trunk
        pkgs.binaryen
        wasm-bindgen-cli
        pkgs.rustPlatform.cargoSetupHook
      ];

      buildPhase = ''
        runHook preBuild
        export HOME=$TMPDIR
        cd crates/chaos-web
        trunk build --release --offline true --dist dist
        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall
        cp -r dist $out
        runHook postInstall
      '';

      meta.description = "chaos web frontend (static trunk dist)";
    };
    # Desktop shell. generate_context! bakes the web dist into the binary at
    # compile time, so the chaos-web output is copied in place before cargo
    # runs. wrapGAppsHook3 wires GSettings schemas + TLS (glib-networking),
    # without which WebKitGTK apps crash or fail https at runtime.
    chaos-desktop = rustPlatform.buildRustPackage {
      pname = "chaos-desktop";
      inherit version;
      src = self;

      cargoLock.lockFile = ./Cargo.lock;

      cargoBuildFlags = ["-p" "chaos-desktop"];
      cargoTestFlags = ["-p" "chaos-desktop"];

      nativeBuildInputs = with pkgs; [pkg-config wrapGAppsHook3];
      buildInputs = tauriLibs ++ [pkgs.glib-networking];

      preBuild = ''
        rm -rf crates/chaos-web/dist
        cp -r ${chaos-web} crates/chaos-web/dist
      '';

      postInstall = ''
        install -Dm644 crates/chaos-desktop/icons/128x128.png \
          $out/share/icons/hicolor/128x128/apps/chaos.png
        install -Dm644 crates/chaos-desktop/icons/32x32.png \
          $out/share/icons/hicolor/32x32/apps/chaos.png
        mkdir -p $out/share/applications
        cat > $out/share/applications/chaos.desktop <<INI
        [Desktop Entry]
        Name=chaos
        Comment=Dashboard, links and calendar for local services
        Exec=chaos-desktop
        Icon=chaos
        Type=Application
        Categories=Utility;
        INI
      '';

      meta = {
        description = "chaos desktop shell (Tauri)";
        mainProgram = "chaos-desktop";
      };
    };
  in {
    packages.${system} = {
      inherit chaos-server chaos-web chaos-desktop;
      default = chaos-server;
    };

    nixosModules = {
      chaos = import ./nix/module.nix self;
      default = self.nixosModules.chaos;
    };

    devShells.${system} = {
      default = pkgs.mkShell {
        name = "chaos";

        nativeBuildInputs = with pkgs; [
          pkg-config
          gobject-introspection
        ];

        buildInputs = tauriLibs;

        packages = with pkgs;
          [
            rustToolchain
            trunk
            binaryen # wasm-opt, used by trunk release builds
            cargo-tauri
            just
            monolith # page snapshots for the link archiver
          ]
          ++ lib.optional hasCargoLock wasm-bindgen-cli;

        # Some webkit/nvidia combinations render a blank Tauri window without it.
        env.WEBKIT_DISABLE_DMABUF_RENDERER = "1";
      };

      # Android build of the shell: `nix develop .#android`, then `just apk`
      # (or `cargo tauri android build --apk --target aarch64` in
      # crates/chaos-desktop).
      android = pkgs.mkShell {
        name = "chaos-android";

        packages = with pkgs;
          [
            rustToolchain
            trunk
            binaryen
            just
            cargo-tauri
            jdk17
            androidComposition.androidsdk
          ]
          ++ lib.optional hasCargoLock wasm-bindgen-cli;

        env = rec {
          JAVA_HOME = pkgs.jdk17.home;
          ANDROID_HOME = "${androidComposition.androidsdk}/libexec/android-sdk";
          NDK_HOME = "${ANDROID_HOME}/ndk/${androidNdkVersion}";
        };

        # The tauri CLI insists on `rustup target add`; the rust-overlay
        # toolchain already ships every Android target, so a no-op is honest.
        shellHook = ''
          shim_dir=$(mktemp -d)
          printf '#!/bin/sh\nexit 0\n' > "$shim_dir/rustup"
          chmod +x "$shim_dir/rustup"
          export PATH="$shim_dir:$PATH"
        '';
      };
    };

    formatter.${system} = pkgs.alejandra;
  };
}
