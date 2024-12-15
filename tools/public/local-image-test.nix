{
  dive,
  trivy,
  writeShellScriptBin,
}:
writeShellScriptBin "local-image-test" ''
  set -xeuo pipefail

  echo "Testing image: $1"

  # Commit hashes would not be a good choice here as they are not
  # fully dependent on the inputs to the image. For instance, amending
  # nothing would still lead to a new hash. Instead we use the
  # derivation hash as the tag so that the tag is reused if the image
  # didn't change.
  IMAGE_TAG=$(nix eval .#$1.imageTag --raw)
  IMAGE_NAME=$(nix eval .#$1.imageName --raw)

  nix run .#$1.copyTo \
    docker-daemon:''${IMAGE_NAME}:''${IMAGE_TAG}

  # Ensure that the image has minimal closure size.
  # TODO(aaronmondal): The default allows 10% inefficiency. Since we control all
  #                    our images fully we should enforce 0% inefficiency. At
  #                    the moment this breaks lre-cc.
  CI=1 ${dive}/bin/dive \
    ''${IMAGE_NAME}:''${IMAGE_TAG}

  # TODO(aaronmondal): Keep monitoring this for better solutions to ratelimits:
  #                    https://github.com/aquasecurity/trivy-action/issues/389
  ${trivy}/bin/trivy image \
    ''${IMAGE_NAME}:''${IMAGE_TAG} \
    --db-repository public.ecr.aws/aquasecurity/trivy-db:2
''
