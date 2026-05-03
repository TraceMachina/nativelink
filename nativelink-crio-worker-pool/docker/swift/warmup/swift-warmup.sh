#!/bin/bash
# Swift warmup driver. Run at template-creation time, then again on each
# pool acquisition (so transient state in the upper layer also gets primed).
#
# What we're warming up:
#   1. swiftc's module cache (preserved across jobs via the COW lower layer).
#   2. The compiled SwiftWarmup binary's own resident pages.
#   3. The shared linker / dyld caches that swift programs hit on first run.
set -euo pipefail

WARMUP_DIR="${WARMUP_DIR:-/opt/warmup}"
ITERATIONS="${WARMUP_ITERATIONS:-100}"

echo "=== Starting Swift warmup ==="
swift --version 2>&1 | head -n 1

# Run the precompiled warmup binary - exercises Foundation, codable, math.
"${WARMUP_DIR}/SwiftWarmup" "${ITERATIONS}"

# Trigger swiftc once to confirm the module cache is hot. We use -typecheck
# so we don't pay codegen / link cost; the goal is to warm the parser and
# the module loader, not produce an artifact.
swiftc -typecheck \
    -module-cache-path "${SWIFT_MODULE_CACHE:-/opt/warmup/module-cache}" \
    "${WARMUP_DIR}/SwiftWarmup.swift"

echo "=== Swift warmup complete ==="
