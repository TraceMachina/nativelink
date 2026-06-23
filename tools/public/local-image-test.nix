{
  dive,
  trivy,
  writeShellScriptBin,
  skopeo,
}:
writeShellScriptBin "local-image-test" ''
  set -euo pipefail

  echo "Testing image: $1"
  set -x

  if [[ $1 == *:* ]]; then
    # Nix targets generally don't have a :, so we assume this is an existing docker image name
    IMAGE_TARGET=$1
  else
    # Commit hashes would not be a good choice here as they are not
    # fully dependent on the inputs to the image. For instance, amending
    # nothing would still lead to a new hash. Instead we use the
    # derivation hash as the tag so that the tag is reused if the image
    # didn't change.
    IMAGE_TAG=$(nix eval .#$1.imageTag --raw)
    IMAGE_NAME=$(nix eval .#$1.imageName --raw)
    IMAGE=$(nix eval .#$1 --raw)
    IMAGE_TARGET=''${IMAGE_NAME}:''${IMAGE_TAG}

    # Inlining from https://github.com/nlewo/nix2container/blob/76be9608a7f4d6c985d28b0e7be903ae2547df3e/default.nix#L88
    # so we can run --debug on skopeo
    # nix run .#$1.copyTo docker-daemon:''${IMAGE_TARGET}
    nix build .#$1
    EXTRA_ARGS=
    if [[ -n ''${DOCKER_HOST:-} ]]; then
      EXTRA_ARGS="--dest-daemon-host=''${DOCKER_HOST}"
    fi
    ${skopeo}/bin/skopeo --debug --insecure-policy copy ''${EXTRA_ARGS} nix:''${IMAGE} docker-daemon:''${IMAGE_TARGET}
  fi

  # Ensure that the image has minimal closure size.
  # TODO(palfrey): The default allows 10% inefficiency. Since we control all
  #                    our images fully we should enforce 0% inefficiency. At
  #                    the moment this breaks lre-cc.
  CI=1 ${dive}/bin/dive ''${IMAGE_TARGET}

  # TODO(palfrey): Keep monitoring this for better solutions to ratelimits:
  #                    https://github.com/aquasecurity/trivy-action/issues/389
  ${trivy}/bin/trivy image \
    ''${IMAGE_TARGET} \
    --db-repository public.ecr.aws/aquasecurity/trivy-db:2
''
