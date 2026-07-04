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
    };
    inherit (pkgs) lib;

    # Toolchain pinned by rust-toolchain.toml (stable + wasm32 target).
    rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

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
  in {
    devShells.${system}.default = pkgs.mkShell {
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

    formatter.${system} = pkgs.alejandra;
  };
}
