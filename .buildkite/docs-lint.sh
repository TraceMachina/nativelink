#!/usr/bin/env bash
# What CI / Docs checks. Every check runs, so one failure hides no other.
set -euo pipefail

cd web
run() { nix develop --impure --command bash -c "$1"; }

run "pnpm install"
run "pnpm run --filter @nativelink/docs gen:changelog"

failed=()
check() {
    echo "--- $1"
    run "$2" || failed+=("$1")
}
check "Anchors are explicit" "pnpm run --filter @nativelink/docs lint:anchors"
check "Internal links resolve" "pnpm run --filter @nativelink/docs lint:links"
check "Config snippets match the schema" "pnpm run --filter @nativelink/docs lint:snippets"
check "No calls to action outside the boundary page" "pnpm run --filter @nativelink/docs lint:boundaries"
check "The reading path has no dead ends" "pnpm run --filter @nativelink/docs lint:navigation"

echo "--- Generated pages are not hand-edited"
run "pnpm run --filter @nativelink/docs gen:llms"
if ! git diff --quiet -- apps/docs; then
    echo "Regenerating changed committed files. Run the generator and commit the result; do not hand-edit generated pages."
    git diff --stat -- apps/docs
    failed+=("Generated pages are not hand-edited")
fi

if [[ ${#failed[@]} -gt 0 ]]; then
    echo "^^^ +++"
    printf 'Failed: %s\n' "${failed[@]}"
    exit 1
fi
