#!/usr/bin/env bash
# What CI / Docs / Vale checks: Vale's errors in the files this pull request
# changes. The styles are committed, so nothing is synced.
set -euo pipefail

base=${BUILDKITE_PULL_REQUEST_BASE_BRANCH:-main}
git fetch --quiet --no-tags origin "$base"
mapfile -t files < <(git diff --name-only --diff-filter=d --merge-base FETCH_HEAD -- \
    '*.md' '*.mdx' 'nativelink-config/src/*.rs')
if [[ ${#files[@]} -eq 0 ]]; then
    echo "No changed files Vale checks."
    exit 0
fi
nix shell --no-write-lock-file --inputs-from . nixpkgs#vale --command \
    vale --no-wrap "${files[@]}"
