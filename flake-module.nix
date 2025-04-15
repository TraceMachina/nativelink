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
    in {
      options.${namespace} = lib.mkOption {
        type = lib.types.submoduleWith {
          modules = [
            ./modules/installation-script.nix
            ./modules/nativelink.nix
          ];
          specialArgs = {inherit pkgs;};
        };
      };

      config.${namespace} = let
        cfg = config.${namespace};
      in {
        enable = true;
        bazelrcContent = let
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
          # If the `nativelink.settings.prefix` is set to a nonempty string,
          # prefix the Bazel build commands with that string. This will disable
          # connecting to the nativelink-cloud by default and require adding
          # `--config=<prefix>` to Bazel invocations.
          maybePrefixedConfig =
            if (cfg.prefix == "")
            then map (x: "build " + x) defaultConfig
            else map (x: "build:" + cfg.prefix + " " + x) defaultConfig;
        in
          lib.concatLines maybePrefixedConfig;
        inherit namespace;
      };
    }
  );
}
