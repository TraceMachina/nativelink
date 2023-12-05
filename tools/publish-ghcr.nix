{ pkgs, ... }:

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
  IMAGE_TAG=$(nix eval .#image.imageTag --raw)

  TAGGED_IMAGE=''${GHCR_REGISTRY}/''${GHCR_IMAGE_NAME,,}:''${IMAGE_TAG}

  $(nix build .#image --print-build-logs --verbose) \
    && ./result \
    | ${pkgs.zstd}/bin/zstd \
    | ${pkgs.skopeo}/bin/skopeo \
      copy \
      docker-archive:/dev/stdin \
      docker://''${TAGGED_IMAGE}

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
