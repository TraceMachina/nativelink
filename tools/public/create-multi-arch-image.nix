# FIXME: Really this should be using nix2container or skopeo, but they can't do that
# See https://github.com/podman-container-tools/skopeo/issues/1136
# * docker manifest create requires you to have pushed already
# * Podman breaks in CI because fun with permissions
# So we're down to regctl, but better options welcomed
{
  writeShellScriptBin,
  regctl,
}:
writeShellScriptBin "create-multi-arch-image" ''
  set -euo pipefail

  echo "Testing images: $@"
  set -x

  # We use a fixed image target to make later copying easier
  IMAGE_TARGET=nativelink-multi-arch:latest

  IMAGES=

  for image in "$@"
  do
    NEW_IMAGE="$(nix eval .#$image.imageName --raw):$(nix eval .#$image.imageTag --raw)"
    IMAGE_DIR=$(mktemp -d)
    nix run .#$image.copyTo oci:''${IMAGE_DIR}
    IMAGES="$IMAGES --ref ocidir://''${IMAGE_DIR}"
    break
  done

  ${regctl}/bin/regctl -v trace index create ''${IMAGE_TARGET} ''${IMAGES}
''
