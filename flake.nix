{
  description = "turbo-cache";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = inputs@{ nixpkgs, flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" ];
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
        in
        {
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = [
              # Development tooling goes here.
              pkgs.cargo
              openssl_static # Required explicitly for cargo test support.
              bazel
              pkgs.awscli2
            ];
            shellHook = ''
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
