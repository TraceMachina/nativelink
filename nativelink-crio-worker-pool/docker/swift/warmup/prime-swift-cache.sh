#!/bin/bash
# Prime the Swift module cache with the imports a typical build action
# touches. Called from CachePrimingConfig at template-creation time so
# the on-disk cache lands in the COW lower layer and every cloned job
# gets it for free.
set -euo pipefail

CACHE_DIR="${SWIFT_MODULE_CACHE:-/opt/warmup/module-cache}"
mkdir -p "${CACHE_DIR}"

# Synthetic source that imports the modules build actions touch most
# often. Adjust the imports to match your build's real surface area;
# anything imported here gets its .pcm/.swiftmodule resolved into the
# cache and is essentially free for subsequent compilations.
PRIMER="$(mktemp -d)/primer.swift"
cat > "${PRIMER}" << 'EOF'
import Foundation
import Dispatch
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

@inline(never) func touch() {
    _ = Date().timeIntervalSinceNow
    _ = JSONEncoder()
    _ = DispatchQueue.global()
}
EOF

echo "Priming Swift module cache at ${CACHE_DIR}"
swiftc -typecheck -module-cache-path "${CACHE_DIR}" "${PRIMER}"
rm -rf "$(dirname "${PRIMER}")"

# Report the warm cache size so deployments can size template_cache_path.
size=$(du -sh "${CACHE_DIR}" 2> /dev/null | awk '{print $1}')
echo "Swift module cache primed: ${size}"
