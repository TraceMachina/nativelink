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
        cfg = config.local-remote-execution;
      in {
        options = {
          local-remote-execution = {
            pkgs = lib.mkOption {
              type = lib.types.uniq (lib.types.lazyAttrsOf (lib.types.raw or lib.types.unspecified));
              description = lib.mdDoc ''
                Nixpkgs to use in the local-remote-execution settings.
              '';
              default = pkgs;
              defaultText = lib.literalMD "`pkgs` (module argument)";
            };
            settings = lib.mkOption {
              type = lib.types.submoduleWith {
                modules = [./modules/lre.nix];
                specialArgs = {inherit (cfg) pkgs;};
              };
              default = {};
              description = lib.mdDoc ''
                The LRE configuration.
              '';
            };
            installationScript = lib.mkOption {
              type = lib.types.str;
              description = lib.mdDoc "A lre.bazelrc generator for local-remote-execution.";
              default = cfg.settings.installationScript;
              defaultText = lib.literalMD "bazelrc contents";
              readOnly = true;
            };
          };
        };
      }
    );
  };

  config = {
    perSystem = {pkgs, ...}: {
      packages = {
        inherit (pkgs.lre) stable-rust nightly-rust;
        rbe-autogen-lre-cc = pkgs.rbe-autogen pkgs.lre.lre-cc.image;
      };
    };
  };
}
