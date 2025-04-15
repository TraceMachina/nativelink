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
      namespace = "nixos";
    in {
      options.${namespace} = lib.mkOption {
        type = lib.types.submoduleWith {
          modules = [
            ../../modules/installation-script.nix
            ./modules/nixos.nix
          ];
          specialArgs = {inherit pkgs;};
        };
      };

      config.${namespace} = {
        enable =
          if builtins.pathExists /etc/NIXOS
          then true
          else false;
        bazelrcContent = let
          pathString = builtins.concatStringsSep ":" config.${namespace}.path;
        in ''
          # Add to your NixOS config:
          # programs.nix-ld.enable = true;
          # services.envfs.enable = true;

          build --action_env=PATH=${pathString}
          build --host_action_env=PATH=${pathString}
        '';
        inherit namespace;
      };
    }
  );
}
