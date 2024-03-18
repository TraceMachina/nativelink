{pkgs, ...}:
pkgs.writeShellScriptBin "publish-ghcr" ''
  set -xeuo pipefail

  echo $GHCR_PASSWORD | ${pkgs.skopeo}/bin/skopeo \
    login \
    --username=$GHCR_USERNAME \
    --password-stdin \
    ghcr.io

  # Commit hashes would not be a good choice here as they are not
  # fully dependent on the inputs to the image. For instance, amending
  # nothing would still lead to a new hash. Instead we use the
  # derivation hash as the tag so that the tag is reused if the image
  # didn't change.
  #
  # If a positional argument is passed it overrides the tag value.
  IMAGE_TAG=''${1:-$(nix eval .#image.imageTag --raw)}

  TAGGED_IMAGE=''${GHCR_REGISTRY}/''${GHCR_IMAGE_NAME,,}:''${IMAGE_TAG}

  nix run .#image.copyTo docker://''${TAGGED_IMAGE}

  echo $GHCR_PASSWORD | ${pkgs.cosign}/bin/cosign \
    login \
    --username=$GHCR_USERNAME \
    --password-stdin \
    ghcr.io

  ${pkgs.cosign}/bin/cosign \
    sign \
    --yes \
    ''${GHCR_REGISTRY}/''${GHCR_IMAGE_NAME,,}@$( \
      ${pkgs.skopeo}/bin/skopeo \
        inspect \
        --format "{{ .Digest }}" \
        docker://''${TAGGED_IMAGE} \
  )
''
