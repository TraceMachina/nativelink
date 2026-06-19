# FIXME: Really this should be using nix2container or skopeo, but they can't do that
# See https://github.com/podman-container-tools/skopeo/issues/1136
# * docker manifest create fails with permission errors (and no logging about _what_)
# * Podman breaks in CI because fun with permissions
# So we're down to regctl, but better options welcomed
{
  writeShellScriptBin,
  regclient,
  dive,
  trivy,
}:
writeShellScriptBin "create-multi-arch-image" ''
  set -euo pipefail

  REGISTRY=$1
  shift
  IMAGE_TARGET=$1
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

  # Ensure that the image has minimal closure size.
  # TODO(palfrey): The default allows 10% inefficiency. Since we control all
  #                    our images fully we should enforce 0% inefficiency. At
  #                    the moment this breaks lre-cc.
  CI=1 ${dive}/bin/dive ''${FULL_IMAGE_TARGET}

  # TODO(palfrey): Keep monitoring this for better solutions to ratelimits:
  #                    https://github.com/aquasecurity/trivy-action/issues/389
  ${trivy}/bin/trivy image \
    ''${FULL_IMAGE_TARGET} \
    --db-repository public.ecr.aws/aquasecurity/trivy-db:2
''
