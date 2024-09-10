{
  pkgs,
  buildImage,
  ...
}: let
  # NativeLink Bridge
  nativelink-bridge = pkgs.stdenv.mkDerivation {
    name = "nativelink-bridge";
    src = ./.;
    buildInputs = [pkgs.bun];
    installPhase = ''
      mkdir -p $out
      cp -r $src/* $out
    '';
  };
in
  buildImage {
    name = "nativelink-bridge";

    # Container configuration
    config = {
      WorkingDir = "${nativelink-bridge}";
      Entrypoint = ["${pkgs.bun}/bin/bun" "run" "index.ts"];
      ExposedPorts = {
        "8080/tcp" = {};
      };
      Labels = {
        "org.opencontainers.image.description" = "A simple Bun environment image";
        "org.opencontainers.image.title" = "Bun Environment";
      };
    };
  }
