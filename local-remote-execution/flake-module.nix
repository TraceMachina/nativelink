{
  lib,
  flake-parts-lib,
  ...
}: {
  options.perSystem = flake-parts-lib.mkPerSystemOption (
    {
      config,
      pkgs,
      ...
    }: let
      namespace = "lre";
      cfg = config.${namespace};
    in {
      options.${namespace} = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default = true;
        };
        installationScript = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = lib.mkDoc ''
            A bash snippet that creates a nixos.bazelrc file in the repository.
          '';
        };
        Env = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          description = lib.mdDoc ''
            The environment that makes up the LRE toolchain.

            For instance, to make the the environment from the lre-cc toolchain
            available through the `local-remote-execution.installationScript`:

            ```nix
            inherit (lre-cc.meta) Env;
            ```

            To add a tool to the local execution environment without adding it to
            the remote environment:

            ```nix
            Env = [ pkgs.some_local_tool ];
            ```

            If you require toolchains that aren't available in the default LRE
            setups:

            ```
            let
              // A custom remote execution container.
              myCustomToolchain = let
                Env = [
                  "PATH=''${pkgs.somecustomTool}/bin"
                ];
              in nix2container.buildImage {
                name = "my-custom-toolchain";
                maxLayers = 100;
                config = {inherit Env;};
                meta = {inherit Env;};
              };
            in
            Env = lre-cc.meta.Env ++ myCustomToolchain.meta.Env;
            ```

            The evaluated contents of `Env` are printed in `lre.bazelrc`, this
            causes nix to put these dependencies into your local nix store, but
            doesn't influence any other tooling like Bazel builds.
          '';
          default = [];
        };
        prefix = lib.mkOption {
          type = lib.types.str;
          description = lib.mdDoc ''
            An optional Bazel config prefix for the flags in `lre.bazelrc`.

            If set, builds need to explicitly enable the LRE config via
            `--config=<prefix>`.

            Defaults to an empty string, enabling LRE by default.
          '';
          default = "";
        };
      };

      config.${namespace} = lib.mkIf cfg.enable {
        installationScript = let
          bazelrcContent = ''
            # These are the paths used by your local LRE config. If you get cache misses
            # between local and remote execution, double-check these values against the
            # toolchain configs in the `@local-remote-execution` repository at the
            # commit that you imported in your `MODULE.bazel`.
            ${lib.concatLines maybeEnv}
            # Bazel-side configuration for LRE.
            ${lib.concatLines maybePrefixedConfig}
          '';

          nixExecToRustExec = p: let
            inherit (p.stdenv.buildPlatform) system;
          in
            {
              "aarch64-darwin" = "aarch64-apple-darwin";
              "aarch64-linux" = "aarch64-unknown-linux-gnu";
              "x86_64-darwin" = "x86_64-apple-darwin";
              "x86_64-linux" = "x86_64-unknown-linux-gnu";
            }.${
              system
            } or (throw "Unsupported Nix exec platform: ${system}");

          nixExecToDefaultRustTarget = p: let
            inherit (p.stdenv.targetPlatform) system;
          in
            {
              "aarch64-darwin" = "aarch64-apple-darwin";
              "aarch64-linux" = "aarch64-unknown-linux-musl";
              "x86_64-darwin" = "x86_64-apple-darwin";
              "x86_64-linux" = "x86_64-unknown-linux-musl";
            }.${
              system
            } or (throw "Unsupported Nix target platform: ${system}");

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

              # TODO(palfrey): Remove after resolution of:
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
                    # TODO(palfrey): Reimplement rbe-configs-gen for C++ so that we
                    #                    can support more execution and target platforms.
                    "@local-remote-execution//generated-cc/config:platform"
                  ])
              }"
            ]
            # C++.
            # TODO(palfrey): At the moment lre-cc only supports x86_64-linux.
            #                    Extend this to more nix systems.
            # See: https://github.com/bazelbuild/bazel/issues/19714#issuecomment-1745604978
            ++ lib.optionals pkgs.stdenv.isLinux [
              "--action_env=BAZEL_DO_NOT_DETECT_CPP_TOOLCHAIN=1"

              # The C++ toolchain running on the execution platform.
              "--extra_toolchains=@local-remote-execution//generated-cc/config:cc-toolchain"

              # TODO(palfrey): Support different target platforms in lre-cc and add
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
              # TODO(palfrey): At the moment these platforms are "rust-specific".
              #                    Generalize this to all languages.
              "--platforms=@local-remote-execution//rust/platforms:${nixExecToDefaultRustTarget pkgs}"
            ];

          maybeEnv =
            if cfg.Env == []
            then ["#" "# WARNING: No environment set. LRE will not work locally."]
            else ["#"] ++ (map (x: "# " + x) (lib.lists.unique cfg.Env));

          # If the `local-remote-execution.prefix` is set to a nonempty string,
          # prefix the Bazel build commands with that string. This will disable LRE
          # by default and require adding `--config=<prefix>` to Bazel invocations.
          maybePrefixedConfig =
            if (cfg.prefix == "")
            then map (x: "build " + x) defaultConfig
            else map (x: "build:" + cfg.prefix + " " + x) defaultConfig;
        in
          import ../tools/installation-script.nix {
            inherit bazelrcContent namespace pkgs;
          };
      };

      config.packages = {
        inherit (pkgs.lre) stable-rust nightly-rust;
        rbe-autogen-lre-cc = pkgs.rbe-autogen pkgs.lre.lre-cc.image;
      };
    }
  );
}
