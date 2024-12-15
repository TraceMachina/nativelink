{
  writeShellScriptBin,
  skopeo,
  cosign,
  trivy,
}:
writeShellScriptBin "publish-ecr" ''
  set -xeuo pipefail

  AWS_REGISTRY=''${AWS_ACCOUNT_ID}.dkr.ecr.''${AWS_REGION}.amazonaws.com

  aws ecr get-login-password --region ''${AWS_REGION} | ${skopeo}/bin/skopeo \
    login \
    --username=AWS \
    --password-stdin \
    ''${AWS_REGISTRY}

  # Commit hashes would not be a good choice here as they are not
  # fully dependent on the inputs to the image. For instance, amending
  # nothing would still lead to a new hash. Instead we use the
  # derivation hash as the tag so that the tag is reused if the image
  # didn't change.
  #
  # If a second argument is passed it overrides the tag value.
  IMAGE_TAG=''${2:-$(nix eval .#$1.imageTag --raw)}
  IMAGE_NAME=$(nix eval .#$1.imageName --raw)

  TAGGED_IMAGE=''${AWS_REGISTRY,,}/''${IMAGE_NAME}:''${IMAGE_TAG}

  nix run .#$1.copyTo docker://''${TAGGED_IMAGE}

  aws ecr get-login-password --region ''${AWS_REGION} | ${cosign}/bin/cosign \
    login \
    --username=AWS \
    --password-stdin \
    ''${AWS_REGISTRY}

  ${cosign}/bin/cosign \
    sign \
    --yes \
    ''${AWS_REGISTRY,,}/''${IMAGE_NAME}@$( \
      ${skopeo}/bin/skopeo \
        inspect \
        --format "{{ .Digest }}" \
        docker://''${TAGGED_IMAGE} \
  )

  ${trivy}/bin/trivy \
    image \
    --format sarif \
    ''${TAGGED_IMAGE} \
  > trivy-results.sarif
''
