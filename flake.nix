{
  description = "Statically detect memory, concurrency bugs and possible panic locations for Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, ... }:
    let
      # Systems supported
      allSystems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      # Helper to provide system-specific attributes
      forAllSystems = f: nixpkgs.lib.genAttrs allSystems (system: f rec {
        pkgs = nixpkgs.legacyPackages.${system};
        # Single source of truth: `rust-toolchain.toml`
        rustToolchain = (rust-overlay.lib.mkRustBin { } pkgs.buildPackages).fromRustupToolchainFile ./rust-toolchain.toml;
      });
    in
    {
      packages = forAllSystems ({ pkgs, rustToolchain }: rec {
        default = lockbud;
        lockbud = (pkgs.makeRustPlatform { cargo = rustToolchain; rustc = rustToolchain; }).buildRustPackage {
          pname = "lockbud";
          version = (pkgs.lib.importTOML ./Cargo.toml).package.version;
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          env.RUST_SYSROOT = rustToolchain;
        };
      });

      apps = forAllSystems ({ pkgs, rustToolchain }: {
        default = {
          type = "app";
          program = pkgs.lib.getExe (pkgs.writeShellApplication {
            name = "cargo-lockbud";
            runtimeInputs = [ self.packages.${pkgs.stdenv.hostPlatform.system}.lockbud rustToolchain pkgs.stdenv.cc ];
            text = ''exec cargo lockbud "$@"'';
          });
          meta.description = "Run `cargo lockbud` with the pinned nightly";
        };
      });

      devShells = forAllSystems ({ pkgs, rustToolchain }: {
        default = pkgs.mkShell {
          packages = [ rustToolchain pkgs.python3 ];
          RUST_SYSROOT = rustToolchain;
        };
      });
    };
}
