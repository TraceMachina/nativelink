{
  pkgs,
  buildImage,
  ...
}: let
  customStdenv = import ../tools/llvmStdenv.nix {inherit pkgs;};
  customClang = pkgs.callPackage ../tools/customClang.nix {
    inherit pkgs;
    stdenv = customStdenv;
  };

  # This environment is shared between toolchain autogen images and the final
  # toolchain image.
  Env = [
    # Add all tooling here so that the generated toolchains use `/nix/store/*`
    # paths instead of `/bin` or `/usr/bin`. This way we're guaranteed to use
    # binary identical toolchains during local and remote execution.
    ("PATH="
      + (pkgs.lib.strings.concatStringsSep ":" [
        "${customStdenv.cc.bintools}/bin"
        "${customClang}/bin"
        "${customStdenv}/bin"
        "${pkgs.coreutils}/bin"
        "${pkgs.findutils}/bin"
        "${pkgs.gnutar}/bin"
      ]))

    "CC=${customClang}/bin/customClang"

    # TODO(aaronmondal): The rbe_config_gen tool invokes bazel inside the
    #                    container to determine compileflags/linkflags.
    #                    Setting these variables here causes them to be baked
    #                    into the generated toolchain config. They don't
    #                    influence remote action invocations as NativeLink
    #                    invokes commands "raw" in the container. However, it
    #                    would be nicer to handle this as part of the nix
    #                    stdenv instead.
    "BAZEL_LINKOPTS=${pkgs.lib.concatStringsSep ":" [
      "-L${pkgs.llvmPackages_17.libcxx}/lib"
      "-L${pkgs.llvmPackages_17.libunwind}/lib"
      "-lc++"
      (
        "-Wl,"
        + "-rpath,${pkgs.llvmPackages_17.libcxx}/lib,"
        + "-rpath,${pkgs.llvmPackages_17.libunwind}/lib,"
        + "-rpath,${pkgs.glibc}/lib"
      )
    ]}"
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
