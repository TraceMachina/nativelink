{
  lib,
  flake-parts-lib,
  ...
}: {
  options = {
    perSystem = flake-parts-lib.mkPerSystemOption (
      {
        config,
        options,
        pkgs,
        ...
      }: let
        cfg = config.nativelink;
      in {
        options = {
          nativelink = {
            pkgs = lib.mkOption {
              type = lib.types.uniq (lib.types.lazyAttrsOf (lib.types.raw or lib.types.unspecified));
              description = "Nixpkgs to use.";
              default = pkgs;
              defaultText = lib.literalMD "`pkgs` (module argument)";
            };
            settings = lib.mkOption {
              type = lib.types.submoduleWith {
                modules = [./modules/nativelink.nix];
                specialArgs = {inherit (cfg) pkgs;};
              };
              default = {};
              description = "Configuration for Bazel on Darwin.";
            };
            installationScript = lib.mkOption {
              type = lib.types.str;
              description = "Create nativelink.bazelrc.";
              default = cfg.settings.installationScript;
              defaultText = lib.literalMD "bazelrc content";
              readOnly = true;
            };
          };
        };
      }
    );
  };
}
