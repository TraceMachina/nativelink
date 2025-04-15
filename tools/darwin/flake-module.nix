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
    in {
      options.${namespace} = lib.mkOption {
        type = lib.types.submoduleWith {
          modules = [
            ../../modules/installation-script.nix
          ];
          specialArgs = {inherit pkgs;};
        };
      };

      config.${namespace} = {
        enable =
          if pkgs.stdenv.isDarwin
          then true
          else false;
        bazelrcContent = ''
          build --@rules_rust//:extra_rustc_flags=-L${pkgs.libiconv}/lib,-Lframework=${pkgs.darwin.Security}/Library/Frameworks,-Lframework=${pkgs.darwin.CF}/Library/Frameworks
          build --@rules_rust//:extra_exec_rustc_flags=-L${pkgs.libiconv}/lib,-Lframework=${pkgs.darwin.Security}/Library/Frameworks,-Lframework=${pkgs.darwin.CF}/Library/Frameworks
        '';
        inherit namespace;
      };
    }
  );
}
