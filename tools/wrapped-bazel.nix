{ bazel
, writeShellScriptBin

  # Use openssl from nix without explicitly tracking nix store paths in our build
  # files. This is required for the openssl-sys crate.
, openssl
}:

writeShellScriptBin "bazel" ''

if [[
    "$1" == "build" ||
    "$1" == "coverage" ||
    "$1" == "run" ||
    "$1" == "test"
]]; then
    ${bazel}/bin/bazel $1 \
        --action_env=OPENSSL_INCLUDE_DIR="${openssl.dev}/include" \
        --action_env=OPENSSL_LIB_DIR="${openssl.out}/lib" \
        ''${@:2}
else
    ${bazel}/bin/bazel $@
fi''
