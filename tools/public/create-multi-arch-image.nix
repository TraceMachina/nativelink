# FIXME: Really this should be using nix2container or skopeo, but they can't do that
# See https://github.com/podman-container-tools/skopeo/issues/1136
# docker manifest create also bugs out if you're not logged in
# So we're down to podman, so have to copy stuff in and out
{
  writeShellScriptBin,
  podman,
  skopeo,
}:
writeShellScriptBin "create-multi-arch-image" ''
  set -euo pipefail

  echo "Testing images: $@"
  set -x

  # We use the first image name/tags as a base
  IMAGE_TAG=$(nix eval .#$1.imageTag --raw)
  IMAGE_NAME=$(nix eval .#$1.imageName --raw)-multi
  IMAGE_TARGET=''${IMAGE_NAME}:''${IMAGE_TAG}

  IMAGES=

  # Starting with Podman 5.x, a policy.json file is required.
  # If none exists, use skopeo's default permissive policy.
  # https://github.com/containers/image/blob/main/docs/containers-policy.json.5.md
  # https://github.com/NixOS/nixpkgs/blob/nixos-unstable/nixos/modules/virtualisation/containers.nix
  if [[ ! -f "/etc/containers/policy.json" && ! -f "$HOME/.config/containers/policy.json" ]]; then
    echo "No policy found, using skopeo's default instead."
    install -Dm444 "${skopeo.policy}/default-policy.json" "$HOME/.config/containers/policy.json"
  fi

  for image in "$@"
  do
    NEW_IMAGE="$(nix eval .#$image.imageName --raw):$(nix eval .#$image.imageTag --raw)"
    nix run .#$image.copyTo docker-daemon:''${NEW_IMAGE}
    ${podman}/bin/podman pull docker-daemon:''${NEW_IMAGE}
    IMAGES="$IMAGES ''${NEW_IMAGE}"
  done

  ${podman}/bin/podman manifest create --all --amend ''${IMAGE_TARGET} ''${IMAGES}
''
