{ pkgs }:

let
  openssl_static = pkgs.openssl.override { static = true; };
in
pkgs.writeShellScriptBin "bazel" ''

# TODO(aaronmondal): Make GCC work or ship the full C++ toolchain in the flake.
CC=clang

if [[
    "$1" == "build" ||
    "$1" == "coverage" ||
    "$1" == "run" ||
    "$1" == "test"
]]; then
    ${pkgs.bazel}/bin/bazel $1 \
        --action_env=OPENSSL_INCLUDE_DIR="${openssl_static.dev}/include" \
        --action_env=OPENSSL_LIB_DIR="${openssl_static.out}/lib" \
        ''${@:2}
else
    ${pkgs.bazel}/bin/bazel $@
fi''
