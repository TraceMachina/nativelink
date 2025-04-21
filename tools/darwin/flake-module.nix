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
      namespace = "darwin";
      cfg = config.${namespace};
    in {
      options.${namespace} = {
        enable = lib.mkOption {
          type = lib.types.bool;
          default =
            if pkgs.stdenv.isDarwin
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
      };

      config.${namespace} = lib.mkIf cfg.enable {
        installationScript = let
          bazelrcContent = ''
            build --@rules_rust//:extra_rustc_flags=-L${pkgs.libiconv}/lib,-Lframework=${pkgs.darwin.Security}/Library/Frameworks,-Lframework=${pkgs.darwin.CF}/Library/Frameworks
            build --@rules_rust//:extra_exec_rustc_flags=-L${pkgs.libiconv}/lib,-Lframework=${pkgs.darwin.Security}/Library/Frameworks,-Lframework=${pkgs.darwin.CF}/Library/Frameworks
          '';
        in
          import ../installation-script.nix {
            inherit bazelrcContent namespace pkgs;
          };
      };
    }
  );
}
