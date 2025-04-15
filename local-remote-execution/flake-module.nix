{
  lib,
  flake-parts-lib,
  ...
}: {
  options.perSystem = flake-parts-lib.mkPerSystemOption (
    {
      config,
      options,
      pkgs,
      ...
    }: let
      namespace = "lre";
    in {
      options.${namespace} = lib.mkOption {
        type = lib.types.submoduleWith {
          modules = [
            ../modules/installation-script.nix
            ./modules/lre.nix
          ];
          specialArgs = {inherit pkgs;};
        };
      };

      config.${namespace} = let
        cfg = config.${namespace};
      in {
        enable = true;
        bazelrcContent = let
          nixExecToRustExec = p: let
            inherit (p.stdenv.buildPlatform) system;
          in
            {
              "aarch64-darwin" = "aarch64-apple-darwin";
              "aarch64-linux" = "aarch64-unknown-linux-gnu";
              "x86_64-darwin" = "x86_64-apple-darwin";
              "x86_64-linux" = "x86_64-unknown-linux-gnu";
            }
            .${system}
            or (throw "Unsupported Nix exec platform: ${system}");

          nixExecToDefaultRustTarget = p: let
            inherit (p.stdenv.targetPlatform) system;
          in
            {
              "aarch64-darwin" = "aarch64-apple-darwin";
              "aarch64-linux" = "aarch64-unknown-linux-musl";
              "x86_64-darwin" = "x86_64-apple-darwin";
              "x86_64-linux" = "x86_64-unknown-linux-musl";
            }
            .${system}
            or (throw "Unsupported Nix target platform: ${system}");

          # These flags cause a Bazel build to use LRE toolchains regardless of whether
          # the build is running in local or remote configuration.
          #
          # To make a remote executor compatible, make the remote execution image (for
          # instance `nativelink-worker-lre-cc` available to the build.
          #
          # To make local execution compatible, add the `lre.installationscript` to the
          # flake's `mkShell.shellhook`:
          #
          # ```nix
          # devShells.default = pkgs.mkShell {
          #   shellHook = ''
          #   # Generate the `lre.bazelrc` config file.
          #   ${config.local-remote-execution.installationScript}
          #   '';
          # };
          # ```
          defaultConfig =
            [
              # Global configuration.

              # TODO(aaronmondal): Remove after resolution of:
              #                    https://github.com/bazelbuild/bazel/issues/7254
              "--define=EXECUTOR=remote"

              # When using remote executors that differ from the host this needs to
              # be manually extended via the `--extra_execution_platforms` flag.
              "--extra_execution_platforms=${
                lib.concatStringsSep "," ([
                    # Explicitly duplicate the host as an execution platform. This way
                    # local execution is treated the same as remote execution.
                    "@local-remote-execution//rust/platforms:${nixExecToRustExec pkgs}"
                  ]
                  ++ lib.optionals pkgs.stdenv.isLinux [
                    # TODO(aaronmondal): Reimplement rbe-configs-gen for C++ so that we
                    #                    can support more execution and target platforms.
                    "@local-remote-execution//generated-cc/config:platform"
                  ])
              }"
            ]
            # C++.
            # TODO(aaronmondal): At the moment lre-cc only supports x86_64-linux.
            #                    Extend this to more nix systems.
            # See: https://github.com/bazelbuild/bazel/issues/19714#issuecomment-1745604978
            ++ lib.optionals pkgs.stdenv.isLinux [
              "--action_env=BAZEL_DO_NOT_DETECT_CPP_TOOLCHAIN=1"

              # The C++ toolchain running on the execution platform.
              "--extra_toolchains=@local-remote-execution//generated-cc/config:cc-toolchain"

              # TODO(aaronmondal): Support different target platforms in lre-cc and add
              #                    `--platforms` settings here.
            ]
            # Rust.
            ++ [
              # The rust toolchains executing on the execution platform. We default to the
              # platforms corresponding to the host. When using remote executors that
              # differ from the platform corresponding to the host this should be extended
              # via manual `--extra_toolchains` arguments.
              "--extra_toolchains=@local-remote-execution//rust:rust-${pkgs.system}"
              "--extra_toolchains=@local-remote-execution//rust:rustfmt-${pkgs.system}"

              # Defaults for rust target platforms. This is a convenience setting that
              # may be overridden by manual `--platforms` arguments.
              #
              # TODO(aaronmondal): At the moment these platforms are "rust-specific".
              #                    Generalize this to all languages.
              "--platforms=@local-remote-execution//rust/platforms:${nixExecToDefaultRustTarget pkgs}"
            ];

          maybeEnv =
            if cfg.Env == []
            then ["#" "# WARNING: No environment set. LRE will not work locally."]
            else ["#"] ++ (map (x: "# " + x) (lib.lists.unique cfg.Env));

          # If the `local-remote-execution.settings.prefix` is set to a nonempty string,
          # prefix the Bazel build commands with that string. This will disable LRE
          # by default and require adding `--config=<prefix>` to Bazel invocations.
          maybePrefixedConfig =
            if (cfg.prefix == "")
            then map (x: "build " + x) defaultConfig
            else map (x: "build:" + cfg.prefix + " " + x) defaultConfig;
        in ''
          # These are the paths used by your local LRE config. If you get cache misses
          # between local and remote execution, double-check these values against the
          # toolchain configs in the `@local-remote-execution` repository at the
          # commit that you imported in your `MODULE.bazel`.
          ${lib.concatLines maybeEnv}
          # Bazel-side configuration for LRE.
          ${lib.concatLines maybePrefixedConfig}
        '';
        inherit namespace;
      };
      config.packages = {
        inherit (pkgs.lre) stable-rust nightly-rust;
        rbe-autogen-lre-cc = pkgs.rbe-autogen pkgs.lre.lre-cc.image;
      };
    }
  );
}
