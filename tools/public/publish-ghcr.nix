{
  writeShellScriptBin,
  skopeo,
  cosign,
  trivy-report,
}:
writeShellScriptBin "publish-ghcr" ''
  set -xeuo pipefail

  echo $GHCR_PASSWORD | ${skopeo}/bin/skopeo \
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
  # If a second argument is passed it overrides the tag value.
  IMAGE_TAG=''${2:-$(nix eval .#$1.imageTag --raw)}
  IMAGE_NAME=$(nix eval .#$1.imageName --raw)

  TAGGED_IMAGE=''${GHCR_REGISTRY,,}/''${IMAGE_NAME}:''${IMAGE_TAG}

  nix run .#$1.copyTo docker://''${TAGGED_IMAGE}

  # Skip signing if SKIP_SIGNING is set (useful for PR builds)
  if [[ "''${SKIP_SIGNING:-false}" != "true" ]]; then
    echo $GHCR_PASSWORD | ${cosign}/bin/cosign \
      login \
      --username=$GHCR_USERNAME \
      --password-stdin \
      ghcr.io

    DIGEST_IMAGE=''${GHCR_REGISTRY,,}/''${IMAGE_NAME}@$( \
      ${skopeo}/bin/skopeo \
        inspect \
        --format "{{ .Digest }}" \
        docker://''${TAGGED_IMAGE} \
    )

    # A duplicated transparency-log POST fails the whole publish with a 409
    # conflict even though the entry landed. A fresh attempt signs with a
    # new ephemeral key, which the log accepts, so retry instead of failing.
    for attempt in 1 2 3; do
      if ${cosign}/bin/cosign sign --yes "''${DIGEST_IMAGE}"; then
        break
      elif [ "''${attempt}" = 3 ]; then
        echo "Signing failed after ''${attempt} attempts"
        exit 1
      fi
      echo "Signing attempt ''${attempt} failed; retrying"
      sleep 10
    done
  else
    echo "Skipping cosign signing (SKIP_SIGNING=true)"
  fi

  ${trivy-report}/bin/trivy-report ''${TAGGED_IMAGE}

  echo "Published: ''${TAGGED_IMAGE}"
''
