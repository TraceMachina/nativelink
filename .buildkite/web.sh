#!/usr/bin/env bash
# What CI / Web checks on pull requests: the web apps' tests and build.
set -euo pipefail

cd web
for attempt in 1 2 3; do
    if nix develop --impure --command bash -c "pnpm install --frozen-lockfile && pnpm dedupe"; then
        break
    fi
    [[ $attempt -lt 3 ]] || exit 1
    echo "pnpm install failed; retrying in 10 seconds."
    sleep 10
done
nix develop --impure --command bash -c "pnpm run test && pnpm run build"
