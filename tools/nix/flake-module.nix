# Nix-specific bazel config setup
# Note, Nix-specific, not _NixOS_. That goes in ../nixos/flake-module.nix
{
  lib,
  flake-parts-lib,
  ...
}: {
  options.perSystem = flake-parts-lib.mkPerSystemOption (
    {pkgs, ...}: let
      namespace = "nix";
      aws-lc-system-dir = pkgs.callPackage ../aws-lc-system-dir.nix {
        inherit (pkgs) aws-lc;
        aws-lc-dev = pkgs.aws-lc.dev;
      };
    in {
      options.${namespace} = {
        installationScript = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = lib.mkDoc ''
            A bash snippet that creates a nix.bazelrc file in the repository.
          '';
        };
      };

      config.${namespace} = {
        installationScript = let
          bazelrcContent = ''
            build --action_env=AWS_LC_SYS_SYSTEM_DIR=${aws-lc-system-dir}
            build --action_env=AWS_LC_SYS_USE_SYSTEM="true"
          '';
        in
          import ../installation-script.nix {
            inherit bazelrcContent namespace pkgs;
          };
      };
    }
  );
}
