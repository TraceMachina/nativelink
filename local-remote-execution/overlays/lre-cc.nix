{
  nix2container,
  lre,
  lib,
  coreutils,
  findutils,
  gnutar,
  bazel_7,
  writeShellScriptBin,
  rbe-configs-gen,
}: let
  lre-cc-configs-gen = writeShellScriptBin "lre-cc" ''
    set -xeuo pipefail

    SRC_ROOT=$(git rev-parse --show-toplevel)/local-remote-execution

    cd "''${SRC_ROOT}"

    LRE_CC_IMAGE_TAG=${lre.lre-cc.image.imageTag};

    nix run .#rbe-autogen-lre-cc.copyTo \
      docker-daemon:rbe-autogen-lre-cc:''${LRE_CC_IMAGE_TAG} -L

    ${rbe-configs-gen}/bin/rbe_configs_gen \
      --toolchain_container=rbe-autogen-lre-cc:''${LRE_CC_IMAGE_TAG} \
      --exec_os=linux \
      --target_os=linux \
      --bazel_version=${bazel_7.version} \
      --output_src_root=''${SRC_ROOT} \
      --output_config_path=generated-cc \
      --generate_java_configs=false \
      --generate_cpp_configs=true \
      --bazel_path=${bazel_7}/bin/bazel \
      --cpp_env_json=cpp_env.json

    # The rbe_configs_gen tool automatically sets the exec_properties of the
    # generated platform to the generator container name and tag. For efficiency
    # reasons the actual deployment won't be the same as this generator
    # container, so we modify this in the generated configuration.
    sed -i \
      's|rbe-autogen-lre-cc|lre-cc|g' \
      ''${SRC_ROOT}/generated-cc/config/BUILD

    chmod 644 \
      ''${SRC_ROOT}/generated-cc/LICENSE \
      ''${SRC_ROOT}/generated-cc/config/BUILD \

    pre-commit run -a
  '';

  # TODO(aaronmondal): Move the generator logic into this packageset.
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

  image = nix2container.buildImage {
    name = "lre-cc";
    maxLayers = 100;
    config = {inherit Env;};
    # Attached for passthrough to rbe-configs-gen.
    meta = {inherit Env;};

    # Don't set a tag here so that the image is tagged by its derivation hash.
    # tag = null;
  };
in {
  inherit lre-cc-configs-gen image;
  meta = {inherit Env;};
}
