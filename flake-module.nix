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
      namespace = "nativelink";
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
        api-key = lib.mkOption {
          type = lib.types.str;
          description = lib.mdDoc ''
            The API key to connect to the NativeLink Cloud.

            You should only use read-only keys here to prevent cache-poisoning and
            malicious artifact extractions.

            Defaults to NativeLink's shared read-only api key.
          '';
          default = "065f02f53f26a12331d5cfd00a778fb243bfb4e857b8fcd4c99273edfb15deae";
        };
        endpoint = lib.mkOption {
          type = lib.types.str;
          description = lib.mdDoc ''
            The NativeLink Cloud endpoint.

            Defaults to NativeLink's shared cache.
          '';
          default = "grpcs://cas-tracemachina-shared.build-faster.nativelink.net";
        };
        prefix = lib.mkOption {
          type = lib.types.str;
          description = lib.mdDoc ''
            An optional Bazel config prefix for the flags in `nativelink.bazelrc`.

            If set, builds need to explicitly enable the nativelink config via
            `--config=<prefix>`.

            Defaults to an empty string, enabling the cache by default.
          '';
          default = "";
        };
      };

      config.${namespace} = {
        installationScript = let
          bazelrcContent = lib.concatLines maybePrefixedConfig;

          # These flags cause Bazel builds to connect to NativeLink's read-only cache.
          #
          # ```nix
          # devShells.default = pkgs.mkShell {
          #   shellHook = ''
          #   # Generate the `lre.bazelrc` config file.
          #   ${config.nativelink.installationScript}
          #   '';
          # };
          # ```
          defaultConfig = [
            "--remote_cache=${cfg.endpoint}"
            "--remote_header=x-nativelink-api-key=${cfg.api-key}"
            "--remote_header=x-nativelink-project=nativelink-ci"
            "--nogenerate_json_trace_profile"
            "--remote_upload_local_results=false"
            "--remote_cache_async"
          ];

          # If the `nativelink.prefix` is set to a nonempty string,
          # prefix the Bazel build commands with that string. This will disable
          # connecting to the nativelink-cloud by default and require adding
          # `--config=<prefix>` to Bazel invocations.
          maybePrefixedConfig =
            if (cfg.prefix == "")
            then map (x: "build " + x) defaultConfig
            else map (x: "build:" + cfg.prefix + " " + x) defaultConfig;
        in
          import ./tools/installation-script.nix {
            inherit bazelrcContent namespace pkgs;
          };
      };
    }
  );
}
