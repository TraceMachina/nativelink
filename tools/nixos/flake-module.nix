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
      namespace = "nixos";
      cfg = config.${namespace};
    in {
      options.${namespace} = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default =
            if builtins.pathExists /etc/NIXOS
            then true
            else false;
        };
        installationScript = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = lib.mkDoc ''
            A bash snippet that creates a nixos.bazelrc file in the repository.
          '';
        };
        path = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [];
          description = "List of paths to include in the Bazel environment.";
        };
      };

      config.${namespace} = lib.mkIf cfg.enable {
        installationScript = let
          bazelrcContent = ''
            # Add to your NixOS config:
            # programs.nix-ld.enable = true;
            # services.envfs.enable = true;

            build --action_env=PATH=${pathString}
            build --host_action_env=PATH=${pathString}
          '';

          pathString = builtins.concatStringsSep ":" cfg.path;
        in
          import ../installation-script.nix {
            inherit bazelrcContent namespace pkgs;
          };
      };
    }
  );
}
