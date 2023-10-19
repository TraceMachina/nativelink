{
  description = "turbo-cache";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nci.url = "github:yusdacra/nix-cargo-integration";
  };

  outputs = inputs@{ nixpkgs, flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" ];
      imports = [
        inputs.nci.flakeModule
      ];

      perSystem = { config, self', inputs', pkgs, system, ... }:
        {
          nci = {
            projects."turbo-cache" = { path = ./.; };
            crates."cas" =
              let
                # Link OpenSSL statically into the openssl-sys crate.
                openssl_static = pkgs.openssl.override { static = true; };
              in
              {
                drvConfig = {
                  mkDerivation = {
                    nativeBuildInputs = [ openssl_static pkgs.glibc.static ];
                  };
                  env = {
                    # Create a statically linked executable.
                    RUSTFLAGS = "-C target-feature=+crt-static";

                    # Required by transitive openssl-sys crate.
                    OPENSSL_INCLUDE_DIR = "${openssl_static.dev}/include";
                    OPENSSL_LIB_DIR = "${openssl_static.out}/lib";
                  };
                };
                depsDrvConfig = {
                  mkDerivation = {
                    nativeBuildInputs = [ openssl_static ];
                  };
                  env = {
                    # Required by transitive openssl-sys crate.
                    OPENSSL_INCLUDE_DIR = "${openssl_static.dev}/include";
                    OPENSSL_LIB_DIR = "${openssl_static.out}/lib";
                  };
                };
              };
          };
          packages.default = config.nci.outputs."cas".packages.release;
          devShells.default = pkgs.mkShell {
            nativeBuildInputs = [
              # Development tooling goes here.
              pkgs.cargo
              pkgs.google-cloud-sdk
              pkgs.awscli2

              # Wrap Bazel so that the Cargo build can see OpenSSL from nixpkgs.
              (import ./tools/wrapped-bazel.nix { inherit pkgs; })
            ];
          };
        };
    };
}
