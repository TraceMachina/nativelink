#!/usr/bin/env bash
set -euo pipefail

if [[ -n ${BUILD_WORKSPACE_DIRECTORY:-} ]]; then
    repo_root="${BUILD_WORKSPACE_DIRECTORY}"
else
    repo_root="$(git rev-parse --show-toplevel)"
fi

cd "${repo_root}/web/platform"
exec bun run astro check
