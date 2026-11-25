#!/usr/bin/env bash
# Push OCI layouts that were already built. Run this only after the trusted
# checkout has been restored, so the tools come from that checkout's Nixpkgs
# pin and the pull request cannot replace them.

set -euo pipefail

image=$1
tag=$2
oci_root=$3

owner=$(printf '%s' "$GITHUB_REPOSITORY_OWNER" | tr '[:upper:]' '[:lower:]')
dest="ghcr.io/${owner}/${image}:${tag}"

nix_bin=/nix/var/nix/profiles/default/bin/nix
export HOME
HOME=$(mktemp -d)

tools=$(
  env -i \
    HOME="$HOME" \
    PATH="/usr/bin:/bin:${nix_bin%/*}" \
    NIX_CONFIG=$'experimental-features = nix-command flakes\n' \
    "$nix_bin" build --no-link --print-out-paths --impure --expr '
      let
        flake = builtins.getFlake (toString ./.);
        pkgs = flake.inputs.nixpkgs.legacyPackages.x86_64-linux;
      in
        pkgs.symlinkJoin {
          name = "pr-image-publish";
          paths = [ pkgs.skopeo pkgs.regclient ];
        }
    '
)

skopeo="${tools}/bin/skopeo"
regctl="${tools}/bin/regctl"

echo "$GHCR_PASSWORD" | "$skopeo" login --username "$GHCR_USERNAME" --password-stdin ghcr.io
echo "$GHCR_PASSWORD" | "$regctl" registry login ghcr.io --user "$GHCR_USERNAME" --pass-stdin

mapfile -t dirs < <(find "$oci_root" -mindepth 1 -maxdepth 1 -type d | sort)
if [ "${#dirs[@]}" -eq 0 ]; then
  echo "No image layout was produced" >&2
  exit 1
fi

refs=()
for dir in "${dirs[@]}"; do
  layout_tag=$("$skopeo" list-tags "oci:${dir}" | jq -r '.Tags[0]')
  if [ -z "$layout_tag" ] || [ "$layout_tag" = "null" ]; then
    echo "No tag in ${dir}" >&2
    exit 1
  fi
  refs+=(--ref "ocidir://${dir}:${layout_tag}")
done

if [ "${#dirs[@]}" -eq 1 ]; then
  "$skopeo" copy --all "oci:${dirs[0]}:${layout_tag}" "docker://${dest}"
else
  "$regctl" tag delete "$dest" || true
  "$regctl" index create "$dest" "${refs[@]}"
fi

echo "Published: ${dest}"
