{
  pkgs,
  buildImage,
  ...
}: let
  description = "A simple Bun environment image";
  title = "Bun Environment";

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
        "org.opencontainers.image.description" = description;
        "org.opencontainers.image.title" = title;
      };
    };
  }
