{ pkgs, ... }:

pkgs.writeShellScriptBin "local-image-test" ''
  set -xeuo pipefail

  # Commit hashes would not be a good choice here as they are not
  # fully dependent on the inputs to the image. For instance, amending
  # nothing would still lead to a new hash. Instead we use the
  # derivation hash as the tag so that the tag is reused if the image
  # didn't change.
  IMAGE_TAG=$(nix eval .#image.imageTag --raw)

  $(nix build .#image --print-build-logs --verbose) \
    && ./result \
    | ${pkgs.skopeo}/bin/skopeo \
      copy \
      docker-archive:/dev/stdin \
      docker-daemon:native-link:''${IMAGE_TAG}

  # Ensure that the image has minimal closure size.
  CI=1 ${pkgs.dive}/bin/dive \
    native-link:''${IMAGE_TAG} \
    --highestWastedBytes=0
''
