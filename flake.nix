{
  description = "native-link";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    pre-commit-hooks.url = "github:cachix/pre-commit-hooks.nix";
  };

  outputs = inputs@{ nixpkgs, flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" ];
      imports = [ inputs.pre-commit-hooks.flakeModule ];
      perSystem = { config, self', inputs', pkgs, system, ... }:
        let
          # Link OpenSSL statically into the openssl-sys crate.
          openssl_static = pkgs.openssl.override { static = true; };

          # Wrap Bazel so that the Cargo build can see OpenSSL from nixpkgs.
          bazel = import ./tools/wrapped-bazel.nix {
            openssl = openssl_static;
            bazel = pkgs.bazel;
            writeShellScriptBin = pkgs.writeShellScriptBin;
          };

          hooks = import ./tools/pre-commit-hooks.nix { inherit pkgs; };
        in
        {
          pre-commit.settings = { inherit hooks; };
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = [
              # Development tooling goes here.
              pkgs.cargo
              pkgs.rustc
              pkgs.pre-commit
              openssl_static # Required explicitly for cargo test support.
              bazel
            ];
            shellHook = ''
              # Generate the .pre-commit-config.yaml symlink when entering the
              # development shell.
              ${config.pre-commit.installationScript}

              # The Bazel and Cargo builds in nix require a Clang toolchain.
              # TODO(aaronmondal): The Bazel build currently uses the
              #                    irreproducible host C++ toolchain. Provide
              #                    this toolchain via nix for bitwise identical
              #                    binaries across machines.
              export CC=clang
            '';
          };
        };
    };
}
