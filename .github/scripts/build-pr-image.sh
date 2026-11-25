#!/usr/bin/env bash
# Build OCI layouts from the pull request checked out in the current
# directory. The registry password is not in this environment.

set -euo pipefail

out=$1
image=$2
multi=$3
shift 3

mkdir -p "$out"

copy_layout() {
  local attr=$1
  local dir=$2
  local tag
  mkdir -p "$dir"
  tag=$(nix eval --raw ".#${attr}.imageTag")
  nix run ".#${attr}.copyTo" "oci:${dir}:${tag}"
}

if [ "$multi" = "true" ]; then
  i=0
  for component in "$@"; do
    copy_layout "$component" "${out}/${i}"
    i=$((i + 1))
  done
else
  copy_layout "$image" "${out}/0"
fi
