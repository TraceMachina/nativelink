{pkgs}: let
  rbeConfigsGen = import ../local-remote-execution/rbe-configs-gen.nix {
    inherit pkgs;
  };
in
  pkgs.writeShellScriptBin "generate-toolchains" ''
    #!{pkgs.bash}/bin/bash
    set -xeuo pipefail

    SRC_ROOT=$(git rev-parse --show-toplevel)/local-remote-execution

    cd "''${SRC_ROOT}"

    LRE_CC_IMAGE_TAG=$(nix eval .#lre-cc.imageTag --raw)

    nix run .#rbe-autogen-lre-cc.copyTo \
      docker-daemon:rbe-autogen-lre-cc:''${LRE_CC_IMAGE_TAG} -L

    ${rbeConfigsGen}/bin/rbe_configs_gen \
      --toolchain_container=rbe-autogen-lre-cc:''${LRE_CC_IMAGE_TAG} \
      --exec_os=linux \
      --target_os=linux \
      --bazel_version=${pkgs.bazel_7.version} \
      --output_src_root=''${SRC_ROOT} \
      --output_config_path=generated-cc \
      --generate_java_configs=false \
      --generate_cpp_configs=true \
      --bazel_path=${pkgs.bazel_7}/bin/bazel \
      --cpp_env_json=cpp_env.json

    LRE_JAVA_IMAGE_TAG=$(nix eval .#lre-java.imageTag --raw)

    nix run .#rbe-autogen-lre-java.copyTo \
      docker-daemon:rbe-autogen-lre-java:''${LRE_JAVA_IMAGE_TAG} -L

    ${rbeConfigsGen}/bin/rbe_configs_gen \
      --toolchain_container=rbe-autogen-lre-java:''${LRE_JAVA_IMAGE_TAG} \
      --exec_os=linux \
      --target_os=linux \
      --bazel_version=${pkgs.bazel_7.version} \
      --output_src_root=''${SRC_ROOT} \
      --output_config_path=generated-java \
      --generate_java_configs=true \
      --generate_cpp_configs=false \
      --bazel_path=${pkgs.bazel_7}/bin/bazel \
      --cpp_env_json=cpp_env.json

    pre-commit run -a
  ''
