#!/usr/bin/env bash
# Compiles and runs the TypeScript COW isolation test.
#
# Mirrors scripts/test_isolation.sh (Java) for Node.js / TypeScript: shows
# that without isolation a tenant secret leaks across jobs sharing a warm
# worker, and with COW isolation the secret is confined to the per-job
# upper layer.
#
# Exits non-zero if the isolation contract regresses.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
TS_DIR="${SCRIPT_DIR}/../docker/typescript/warmup"

if ! command -v node > /dev/null 2>&1; then
    echo "node not found - install Node.js >= 20 and retry" >&2
    exit 1
fi

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

cp "${TS_DIR}/WarmWorkerIsolationTest.ts" "${WORK_DIR}/"

(
    cd "${WORK_DIR}"
    # Local TypeScript install so we don't depend on a global tsc.
    npm install --silent --no-save --no-audit --no-fund \
        typescript@5.3 @types/node@20 > /dev/null
    ./node_modules/.bin/tsc \
        --target es2022 --module commonjs --strict --esModuleInterop \
        --moduleResolution node --types node \
        WarmWorkerIsolationTest.ts
    node WarmWorkerIsolationTest.js
)
