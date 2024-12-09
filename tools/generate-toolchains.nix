{
  bazel_7,
  writeShellScriptBin,
  rbe-configs-gen,
}:
writeShellScriptBin "generate-toolchains" ''
  set -xeuo pipefail

  SRC_ROOT=$(git rev-parse --show-toplevel)/local-remote-execution

  cd "''${SRC_ROOT}"

  LRE_JAVA_IMAGE_TAG=$(nix eval .#lre-java.imageTag --raw)

  nix run .#rbe-autogen-lre-java.copyTo \
    docker-daemon:rbe-autogen-lre-java:''${LRE_JAVA_IMAGE_TAG} -L

  ${rbe-configs-gen}/bin/rbe_configs_gen \
    --toolchain_container=rbe-autogen-lre-java:''${LRE_JAVA_IMAGE_TAG} \
    --exec_os=linux \
    --target_os=linux \
    --bazel_version=${bazel_7.version} \
    --output_src_root=''${SRC_ROOT} \
    --output_config_path=generated-java \
    --generate_java_configs=true \
    --generate_cpp_configs=false \
    --bazel_path=${bazel_7}/bin/bazel \
    --cpp_env_json=cpp_env.json

  # See comment above for C++.
  sed -i \
    's|rbe-autogen-lre-java|lre-java|g' \
    ''${SRC_ROOT}/generated-java/config/BUILD

  chmod 644 \
    ''${SRC_ROOT}/generated-java/LICENSE \
    ''${SRC_ROOT}/generated-java/config/BUILD \
    ''${SRC_ROOT}/generated-java/java/BUILD

  pre-commit run -a
''
