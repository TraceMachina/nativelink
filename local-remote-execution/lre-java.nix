{
  pkgs,
  buildImage,
  ...
}: let
  # This config is shared between toolchain autogen images and the final
  # toolchain image.
  Env = [
    # Add all tooling here so that the generated toolchains use `/nix/store/*`
    # paths instead of `/bin` or `/usr/bin`. This way we're guaranteed to use
    # binary identical toolchains during local and remote execution.
    ("PATH="
      + (pkgs.lib.strings.concatStringsSep ":" [
        "${pkgs.coreutils}/bin"
        "${pkgs.findutils}/bin"
        "${pkgs.gnutar}/bin"
      ]))
    "JAVA_HOME=${pkgs.jdk17_headless}/lib/openjdk"
  ];
in
  buildImage {
    name = "lre-java";
    maxLayers = 100;
    config = {inherit Env;};
    # Attached for passthrough to rbe-configs-gen.
    meta = {inherit Env;};

    # Don't set a tag here so that the image is tagged by its derivation hash.
    # tag = null;
  }
