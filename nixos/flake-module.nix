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
        cfg = config.nixos;
      in {
        options = {
          nixos = {
            pkgs = lib.mkOption {
              type = lib.types.uniq (lib.types.lazyAttrsOf (lib.types.raw or lib.types.unspecified));
              description = "Nixpkgs to use.";
              default = pkgs;
              defaultText = lib.literalMD "`pkgs` (module argument)";
            };
            settings = lib.mkOption {
              type = lib.types.submoduleWith {
                modules = [./modules/nixos.nix];
                specialArgs = {inherit (cfg) pkgs;};
              };
              default = {};
              description = "Configuration for Bazel on NixOS.";
            };
            installationScript = lib.mkOption {
              type = lib.types.str;
              description = "Create nixos.bazelrc.";
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
