{
  description = "name";
  inputs = {
    flake-parts = {
      follows = "nativelink/flake-parts";
    };
    git-hooks = {
      follows = "nativelink/git-hooks";
    };
    nativelink = {
      url = "github:TraceMachina/nativelink/f9ff630e09a3c22d6a3abea68d1bacc775eac6bb";
    };
    nixpkgs = {
      follows = "nativelink/nixpkgs";
    };
    rust-overlay = {
      follows = "nativelink/rust-overlay";
    };
  };
  outputs = inputs @ {
    flake-parts,
    git-hooks,
    nativelink,
    nixpkgs,
    rust-overlay,
    ...
  }:
    flake-parts.lib.mkFlake {inherit inputs;} {
      systems = [
        "x86_64-linux"
        "x86_64-darwin"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      imports = [
        git-hooks.flakeModule
        nativelink.flakeModules.lre
      ];

      perSystem = {
        config,
        pkgs,
        system,
        ...
      }: {
        _module.args.pkgs = import nixpkgs {
          inherit system;
          overlays = [
            # Add `pkgs.lre` from NativeLink.
            nativelink.overlays.lre
            rust-overlay.overlays.default
          ];
        };

        # Option from the NativeLink flake module.
        lre = {
          # Use the lre-cc environment from NativeLink locally.
          inherit (pkgs.lre.lre-cc.meta) Env;
        };

        # Option from the git-hooks flake module.
        pre-commit.settings = {
          hooks = import ./pre-commit-hooks.nix {inherit pkgs;};
        };

        devShells.default = pkgs.mkShell {
          packages = [
            # Development tools
            pkgs.git

            # Build dependencies
            # Bazel installed by Bazelisk.
            (pkgs.writeShellScriptBin "bazel" ''
              unset TMPDIR TMP
              exec ${pkgs.bazelisk}/bin/bazelisk "$@"
            '')
            pkgs.lre.clang
          ];
          shellHook = ''
            # Generate lre.bazelrc, which configures Bazel toolchains.
            ${config.lre.installationScript}
            # Generate .pre-commit-config.yaml symlink.
            ${config.pre-commit.installationScript}
          '';
        };
      };
    };
}
