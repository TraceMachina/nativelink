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
