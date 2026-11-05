{
  description = "chaos - browser dashboard for local services";

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
    chaos-monolith = pkgs.callPackage ./nix/monolith.nix {};

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

    # Trunk 0.21.14 vendors libdeflate-sys 1.23.1, whose evex512 target
    # attributes were removed by GCC 16. Keep this compatibility compiler
    # scoped to Trunk (web builds and every dev shell), not the app/toolchain.
    # Remove the override when Trunk bundles libdeflate >= 1.25.
    trunk = pkgs.trunk.overrideAttrs (old: {
      env =
        (old.env or {})
        // {
          # cc-rs's target-specific override takes precedence over HOST_CC,
          # which Nix's Rust hooks export from the current stdenv.
          CC_x86_64_unknown_linux_gnu = "${pkgs.gcc15}/bin/cc";
        };
    });

    chaos-server = rustPlatform.buildRustPackage {
      pname = "chaos-server";
      inherit version;
      src = self;

      cargoLock.lockFile = ./Cargo.lock;

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
        trunk
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

    # The dist as chaos-server serves it: brotli/gzip siblings generated once
    # here so ServeDir answers compressed requests without re-compressing
    # megabytes of wasm per request (see api/static_assets.rs).
    chaos-web-static =
      pkgs.runCommand "chaos-web-static-${version}" {
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
  in {
    packages.${system} = {
      inherit chaos-server chaos-web chaos-web-static chaos-monolith;
      default = chaos-server;
    };

    nixosModules = {
      chaos = import ./nix/module.nix self;
      default = self.nixosModules.chaos;
    };

    devShells.${system} = {
      default = pkgs.mkShell {
        name = "chaos";

        packages = with pkgs;
          [
            rustToolchain
            trunk
            binaryen # wasm-opt, used by trunk release builds
            cargo-nextest
            just
            chaos-monolith # page snapshots, with local SVG references preserved
          ]
          ++ lib.optional hasCargoLock wasm-bindgen-cli;
      };
    };

    formatter.${system} = pkgs.alejandra;
  };
}
