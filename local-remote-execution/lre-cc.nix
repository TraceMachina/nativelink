{
  buildImage,
  lre,
  lib,
  coreutils,
  findutils,
  gnutar,
}: let
  # This environment is shared between toolchain autogen images and the final
  # toolchain image.
  Env = [
    # Add all tooling here so that the generated toolchains use `/nix/store/*`
    # paths instead of `/bin` or `/usr/bin`. This way we're guaranteed to use
    # binary identical toolchains during local and remote execution.
    ("PATH="
      + (lib.strings.concatStringsSep ":" [
        "${lre.stdenv.cc.bintools}/bin"
        "${lre.clang}/bin"
        "${lre.stdenv}/bin"
        "${coreutils}/bin"
        "${findutils}/bin"
        "${gnutar}/bin"
      ]))

    "CC=${lre.clang}/bin/customClang"
  ];
in
  buildImage {
    name = "lre-cc";
    maxLayers = 100;
    config = {inherit Env;};
    # Attached for passthrough to rbe-configs-gen.
    meta = {inherit Env;};

    # Don't set a tag here so that the image is tagged by its derivation hash.
    # tag = null;
  }
