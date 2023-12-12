{ pkgs, nativelink, ... }:

let
  customStdenv = import ../tools/llvmStdenv.nix { inherit pkgs; };

  # Bazel expects a single frontend for both C and C++. That works for GCC but
  # not for clang. This wrapper selects `clang` or `clang++` depending on file
  # ending.
  # TODO(aaronmondal): The necessity of this is a bug.
  #                    See: https://github.com/NixOS/nixpkgs/issues/216047
  #                    and https://github.com/NixOS/nixpkgs/issues/150655
  customClang = pkgs.writeShellScriptBin "customClang" ''
    #! ${customStdenv.shell}
    function isCxx() {
       if [ $# -eq 0 ]; then false
       elif [ "$1" == '-xc++' ]; then true
       elif [[ -f "$1" && "$1" =~ [.](hh|H|hp|hxx|hpp|HPP|h[+]{2}|tcc|cc|cp|cxx|cpp|CPP|c[+]{2}|C)$ ]]; then true
       else isCxx "''${@:2}"; fi
    }
    if isCxx "$@"; then
      exec "${customStdenv.cc}/bin/clang++" "$@"
    else
      exec "${customStdenv.cc}/bin/clang" "$@"
    fi
  '';
in

# TODO(aaronmondal): Bazel and a few other tools in this container are only
#                    required to generate the toolchains but are not needed
#                    during runtime. Split this image into a generator image and
#                    a toolchain container and write a new rbe_configs_gen tool.
#                    This will enable endless optimization and customization
#                    opportunities for custom toolchain containers.
pkgs.dockerTools.streamLayeredImage {
  name = "nativelink-toolchain";

  contents = [
    # The worker.
    nativelink

    # Required to generate toolchain configs.
    pkgs.bazel

    # Minimal user setup. Required by Bazel.
    pkgs.fakeNss

    # Required for communication with trusted sources.
    pkgs.cacert

    # Tools that we would usually forward from the host.
    pkgs.bash
    pkgs.coreutils

    # We need these tools to generate the RBE autoconfiguration.
    pkgs.findutils
    pkgs.gnutar

    customStdenv.cc.bintools

    pkgs.llvmPackages_16.libunwind
  ];

  extraCommands = ''
    mkdir -m 0777 tmp

    # Bazel process wrappers expect `env` at `/usr/bin/env`
    mkdir -p -m 0777 usr/bin
    ln -s /bin/env usr/bin/env
  '';

  config = {
    WorkingDir = "/home/bazelbuild";
    Env = [
      # Add all tooling here so that the generated toolchains use `/nix/store/*`
      # paths instead of `/bin` or `/usr/bin`. This way we're guaranteed to use
      # binary identical toolchains during local and remote execution.
      ("PATH=" + (pkgs.lib.strings.concatStringsSep ":" [
        "${customStdenv.cc.bintools}/bin"
        "${customClang}/bin"
        "${customStdenv}/bin"
        "${pkgs.coreutils}/bin"
        "${pkgs.findutils}/bin"
        "${pkgs.gnutar}/bin"
      ]))
      "JAVA_HOME=${pkgs.jdk11_headless}/lib/openjdk"

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
        "-L${pkgs.llvmPackages_16.libcxx}/lib"
        "-L${pkgs.llvmPackages_16.libcxxabi}/lib"
        "-L${pkgs.llvmPackages_16.libunwind}/lib"
        "-lc++"
        ("-Wl," +
        "-rpath,${pkgs.llvmPackages_16.libcxx}/lib," +
        "-rpath,${pkgs.llvmPackages_16.libcxxabi}/lib," +
        "-rpath,${pkgs.llvmPackages_16.libunwind}/lib," +
        "-rpath,${pkgs.glibc}/lib"
        )
      ]}"
    ];
  };
}
