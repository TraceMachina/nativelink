{
  description = "nativelink";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-utils.url = "github:numtide/flake-utils";
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nix2container = {
      url = "github:nlewo/nix2container";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
  };

  outputs = inputs @ {
    self,
    flake-parts,
    crane,
    rust-overlay,
    nix2container,
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
        inputs.git-hooks.flakeModule
        ./local-remote-execution/flake-module.nix
      ];
      perSystem = {
        config,
        pkgs,
        system,
        ...
      }: let
        stable-rust-version = "1.79.0";
        nightly-rust-version = "2024-07-24";

        # TODO(aaronmondal): Make musl builds work on Darwin.
        # See: https://github.com/TraceMachina/nativelink/issues/751
        stable-rust =
          if pkgs.stdenv.isDarwin
          then pkgs.rust-bin.stable.${stable-rust-version}
          else pkgs.pkgsMusl.rust-bin.stable.${stable-rust-version};
        nightly-rust =
          if pkgs.stdenv.isDarwin
          then pkgs.rust-bin.nightly.${nightly-rust-version}
          else pkgs.pkgsMusl.rust-bin.nightly.${nightly-rust-version};

        # TODO(aaronmondal): Tools like rustdoc don't work with the `pkgsMusl`
        # package set because of missing libgcc_s. Fix this upstream and use the
        # `stable-rust` toolchain in the devShell as well.
        # See: https://github.com/oxalica/rust-overlay/issues/161
        stable-rust-native = pkgs.rust-bin.stable.${stable-rust-version};

        maybeDarwinDeps = pkgs.lib.optionals pkgs.stdenv.isDarwin [
          pkgs.darwin.apple_sdk.frameworks.Security
          pkgs.libiconv
        ];

        llvmPackages = pkgs.llvmPackages_18;

        customStdenv = pkgs.callPackage ./tools/llvmStdenv.nix {inherit llvmPackages;};

        # TODO(aaronmondal): This doesn't work with rules_rust yet.
        # Tracked in https://github.com/TraceMachina/nativelink/issues/477.
        customClang = pkgs.callPackage ./tools/customClang.nix {stdenv = customStdenv;};

        craneLib =
          if pkgs.stdenv.isDarwin
          then (crane.mkLib pkgs).overrideToolchain stable-rust.default
          else
            (crane.mkLib pkgs).overrideToolchain (stable-rust.default.override {
              targets = ["x86_64-unknown-linux-musl"];
            });

        src = pkgs.lib.cleanSourceWith {
          src = craneLib.path ./.;
          filter = path: type:
            (builtins.match "^.*(data/SekienSkashita\.jpg|nativelink-config/README\.md)" path != null)
            || (craneLib.filterCargoSources path type);
        };

        commonArgs =
          {
            inherit src;
            stdenv =
              if pkgs.stdenv.isDarwin
              then customStdenv
              else pkgs.pkgsMusl.stdenv;
            strictDeps = true;
            buildInputs = [pkgs.cacert] ++ maybeDarwinDeps;
            nativeBuildInputs = maybeDarwinDeps;
          }
          // pkgs.lib.optionalAttrs (!pkgs.stdenv.isDarwin) {
            CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
            CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
          };

        # Additional target for external dependencies to simplify caching.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        nativelink = craneLib.buildPackage (commonArgs
          // {
            inherit cargoArtifacts;
          });

        nativelink-debug = craneLib.buildPackage (commonArgs
          // {
            inherit cargoArtifacts;
            CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static --cfg tokio_unstable";
            CARGO_PROFILE = "smol";
            cargoExtraArgs = "--features enable_tokio_console";
          });

        publish-ghcr = pkgs.callPackage ./tools/publish-ghcr.nix {};

        local-image-test = pkgs.callPackage ./tools/local-image-test.nix {};

        nativelink-is-executable-test = pkgs.callPackage ./tools/nativelink-is-executable-test.nix {inherit nativelink;};

        rbe-configs-gen = pkgs.callPackage ./local-remote-execution/rbe-configs-gen.nix {};

        generate-toolchains = pkgs.callPackage ./tools/generate-toolchains.nix {inherit rbe-configs-gen;};

        native-cli = pkgs.callPackage ./native-cli/default.nix {};

        docs = pkgs.callPackage ./tools/docs.nix {rust = stable-rust.default;};

        inherit (nix2container.packages.${system}.nix2container) pullImage;
        inherit (nix2container.packages.${system}.nix2container) buildImage;

        nativelink-image = buildImage {
          name = "nativelink";
          copyToRoot = [
            (pkgs.buildEnv {
              name = "nativelink-buildEnv";
              paths = [nativelink];
              pathsToLink = ["/bin"];
            })
          ];
          config = {
            Entrypoint = [(pkgs.lib.getExe' nativelink "nativelink")];
            Labels = {
              "org.opencontainers.image.description" = "An RBE compatible, high-performance cache and remote executor.";
              "org.opencontainers.image.documentation" = "https://github.com/TraceMachina/nativelink";
              "org.opencontainers.image.licenses" = "Apache-2.0";
              "org.opencontainers.image.revision" = "${self.rev or self.dirtyRev or "dirty"}";
              "org.opencontainers.image.source" = "https://github.com/TraceMachina/nativelink";
              "org.opencontainers.image.title" = "NativeLink";
              "org.opencontainers.image.vendor" = "Trace Machina, Inc.";
            };
          };
        };

        nativelink-worker-init = pkgs.callPackage ./tools/nativelink-worker-init.nix {inherit buildImage self nativelink-image;};

        rbe-autogen = pkgs.callPackage ./local-remote-execution/rbe-autogen.nix {
          inherit buildImage;
          stdenv = customStdenv;
        };
        createWorker = pkgs.callPackage ./tools/create-worker.nix {inherit buildImage self;};
        siso-chromium = buildImage {
          name = "siso-chromium";
          fromImage = pullImage {
            imageName = "gcr.io/chops-public-images-prod/rbe/siso-chromium/linux";
            imageDigest = "sha256:26de99218a1a8b527d4840490bcbf1690ee0b55c84316300b60776e6b3a03fe1";
            sha256 = "sha256-v2wctuZStb6eexcmJdkxKcGHjRk2LuZwyJvi/BerMyw=";
            tlsVerify = true;
            arch = "amd64";
            os = "linux";
          };
        };
        lre-cc = pkgs.callPackage ./local-remote-execution/lre-cc.nix {
          inherit customClang buildImage;
          stdenv = customStdenv;
        };
        toolchain-drake = buildImage {
          name = "toolchain-drake";
          # imageDigest and sha256 are generated by toolchain-drake.sh for non-reproducible builds.
          fromImage = pullImage {
            imageName = "localhost:5001/toolchain-drake";
            imageDigest = ""; # DO NOT COMMIT DRAKE IMAGE_DIGEST VALUE
            sha256 = ""; # DO NOT COMMIT DRAKE SHA256 VALUE
            tlsVerify = false;
            arch = "amd64";
            os = "linux";
          };
        };
      in rec {
        _module.args.pkgs = let
          nixpkgs-patched = (import self.inputs.nixpkgs {inherit system;}).applyPatches {
            name = "nixpkgs-patched";
            src = self.inputs.nixpkgs;
            patches = [
              ./tools/nixpkgs_link_libunwind_and_libcxx.diff
              ./tools/nixpkgs_disable_ratehammering_pulumi_tests.diff
              ./tools/nixpkgs_playwright.diff
            ];
          };
        in
          import nixpkgs-patched {
            inherit system;
            overlays = [(import rust-overlay)];
          };
        apps = {
          default = {
            type = "app";
            program = "${nativelink}/bin/nativelink";
          };
          native = {
            type = "app";
            program = "${native-cli}/bin/native";
          };
        };
        packages = rec {
          inherit publish-ghcr local-image-test nativelink-is-executable-test nativelink nativelink-debug native-cli lre-cc nativelink-worker-init;
          default = nativelink;

          rbe-autogen-lre-cc = rbe-autogen lre-cc;
          nativelink-worker-lre-cc = createWorker lre-cc;
          lre-java = pkgs.callPackage ./local-remote-execution/lre-java.nix {inherit buildImage;};
          rbe-autogen-lre-java = rbe-autogen lre-java;
          nativelink-worker-lre-java = createWorker lre-java;
          nativelink-worker-siso-chromium = createWorker siso-chromium;
          nativelink-worker-toolchain-drake = createWorker toolchain-drake;
          image = nativelink-image;
        };
        checks = {
          # TODO(aaronmondal): Fix the tests.
          # tests = craneLib.cargoNextest (commonArgs
          #   // {
          #   inherit cargoArtifacts;
          #   cargoNextestExtraArgs = "--all";
          #   partitions = 1;
          #   partitionType = "count";
          # });
        };
        pre-commit.settings = {
          hooks = import ./tools/pre-commit-hooks.nix {
            inherit pkgs nightly-rust;
          };
        };
        local-remote-execution.settings = {
          Env =
            if pkgs.stdenv.isDarwin
            then [] # Doesn't support Darwin yet.
            else lre-cc.meta.Env;
          prefix = "lre";
        };
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = let
            bazel = pkgs.writeShellScriptBin "bazel" ''
              unset TMPDIR TMP
              exec ${pkgs.bazelisk}/bin/bazelisk "$@"
            '';
          in
            [
              # Development tooling goes here.
              bazel
              stable-rust-native.default
              pkgs.pre-commit
              pkgs.awscli2
              pkgs.skopeo
              pkgs.dive
              pkgs.cosign
              pkgs.kubectl
              pkgs.kubernetes-helm
              pkgs.cilium-cli
              pkgs.vale
              pkgs.trivy
              pkgs.docker-client
              pkgs.kind
              pkgs.tektoncd-cli
              pkgs.pulumi
              pkgs.pulumiPackages.pulumi-language-go
              pkgs.go
              pkgs.kustomize
              pkgs.nodePackages.pnpm

              # Additional tools from within our development environment.
              local-image-test
              generate-toolchains
              customClang
              native-cli
              docs
            ]
            ++ pkgs.lib.optionals (!pkgs.stdenv.isDarwin) [
              # The docs on Mac require a manual setup outside the flake.
              pkgs.playwright-driver.browsers
            ]
            ++ maybeDarwinDeps;
          shellHook =
            ''
              # Generate the .pre-commit-config.yaml symlink when entering the
              # development shell.
              ${config.pre-commit.installationScript}

              # Generate lre.bazelrc which configures LRE toolchains when running
              # in the nix environment.
              ${config.local-remote-execution.installationScript}

              # The Bazel and Cargo builds in nix require a Clang toolchain.
              # TODO(aaronmondal): The Bazel build currently uses the
              #                    irreproducible host C++ toolchain. Provide
              #                    this toolchain via nix for bitwise identical
              #                    binaries across machines.
              export CC=clang
            ''
            + pkgs.lib.optionalString (!pkgs.stdenv.isDarwin) ''
              export PLAYWRIGHT_BROWSERS_PATH=${pkgs.playwright-driver.browsers}
              export PLAYWRIGHT_NODEJS_PATH=${pkgs.nodePackages_latest.nodejs}
            '';
        };
      };
    }
    // {
      flakeModule = ./local-remote-execution/flake-module.nix;
    };
}
