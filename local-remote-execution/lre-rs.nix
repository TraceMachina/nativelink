{
  lib,
  buildImage,
  lre-cc,
  rust-toolchains,
}: let
  Env =
    # Rust requires a functional C++ toolchain.
    lre-cc.meta.Env
    ++ [
      # This causes the rust toolchains to be available under `/nix/store/*`
      # paths but not under "generic" paths like `/bin` or `/usr/bin`.
      # This way we're guaranteed to use binary identical toolchains during
      # local and remote execution.
      "RUST=${lib.concatStringsSep ":" rust-toolchains}"
    ];
in
  buildImage {
    name = "lre-rs";
    maxLayers = 100;
    config = {inherit Env;};
    # Passthrough so that other images can reuse the environment.
    meta = {inherit Env;};

    # Don't set a tag here so that the image is tagged by its derivation hash.
    # tag = null;
  }
