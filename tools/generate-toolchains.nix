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

    IMAGE_TAG=$(nix eval .#lre.imageTag --raw)

    $(nix build .#lre --print-build-logs --verbose) \
      && ./result \
      | ${pkgs.skopeo}/bin/skopeo \
        copy \
        docker-archive:/dev/stdin \
        docker-daemon:nativelink-toolchain:''${IMAGE_TAG}

    ${rbeConfigsGen}/bin/rbe_configs_gen \
      --toolchain_container=nativelink-toolchain:''${IMAGE_TAG} \
      --exec_os=linux \
      --target_os=linux \
      --bazel_version=${pkgs.bazel_7.version} \
      --output_src_root=''${SRC_ROOT} \
      --output_config_path=generated \
      --bazel_path=${pkgs.bazel_7}/bin/bazel \
      --cpp_env_json=cpp_env.json

    pre-commit run -a
  ''
