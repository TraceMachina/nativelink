# FIXME: Really this should be using nix2container or skopeo, but they can't do that
# See https://github.com/podman-container-tools/skopeo/issues/1136
# * docker manifest create fails with permission errors (and no logging about _what_)
# * Podman breaks in CI because fun with permissions
# So we're down to regctl, but better options welcomed
{
  writeShellScriptBin,
  regclient,
  trivy-report,
}:
writeShellScriptBin "create-multi-arch-image" ''
  set -euo pipefail

  REGISTRY=$1
  shift
  IMAGE_TARGET=$(echo $1 | tr '[:upper:]' '[:lower:]')
  shift
  FULL_IMAGE_TARGET=''${REGISTRY}/''${IMAGE_TARGET}
  echo "Creating multi-arch image for ''${FULL_IMAGE_TARGET} from: $@"
  set -x

  if [ $REGISTRY == "localhost:5000" ]; then
    ${regclient}/bin/regctl registry set --tls disabled localhost:5000
  fi

  IMAGES=

  for image in "$@"
  do
    IMAGE_TAG=$(nix eval .#$image.imageTag --raw)
    IMAGE_DIR=$(mktemp -d)
    nix run .#$image.copyTo oci:''${IMAGE_DIR}:''${IMAGE_TAG}
    IMAGES="$IMAGES --ref ocidir://''${IMAGE_DIR}:''${IMAGE_TAG}"
  done

  ${regclient}/bin/regctl -v info index create ''${FULL_IMAGE_TARGET} ''${IMAGES}

  ${trivy-report}/bin/trivy-report ''${FULL_IMAGE_TARGET}
''
