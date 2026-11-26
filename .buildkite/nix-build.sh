#!/usr/bin/env bash
# Builds the Nix packages that CI / Nix builds on the cluster client. On the
# trusted lane, with an Attic token, it then pushes what it built to the
# nativelink cache, so no other agent or runner builds it again.
set -euo pipefail

packages=(
    .#nativelink
    .#nativelink-is-executable-test
    .#rbe-toolchain-with-nativelink-test
    .#rbe-toolchain-list
)

nix build -L --no-write-lock-file --no-link "${packages[@]}"

if [[ $BUILDKITE_PIPELINE_SLUG == *-forks ]]; then
    exit 0
fi
if ! ATTIC_TOKEN=$(buildkite-agent secret get ATTIC_TOKEN 2> /dev/null); then
    echo "No Attic token; skipping the push."
    exit 0
fi

# A failed push only costs other agents a rebuild, so it does not fail the
# build.
push() {
    local attic config derivations
    # The client GitHub's trusted runs use, pinned.
    attic=$(nix build --no-link --print-out-paths \
        github:TraceMachina/attic/53ad4f0271b7b8cace13d22df314a78d8e64be09) || return
    # The token goes into a config file that lives only for this job, never
    # into argv.
    config=$(mktemp -d) || return
    trap 'rm -rf "$config"' RETURN
    mkdir "$config/attic" || return
    cat > "$config/attic/config.toml" << TOML || return
default-server = "nativelink"
[servers.nativelink]
endpoint = "https://attic.uc1.scdev.nativelink.net/"
token = "$ATTIC_TOKEN"
TOML
    # The packages and everything built for them, such as the Rust
    # dependencies. Attic skips what it or cache.nixos.org already holds.
    mapfile -t derivations < <(nix path-info --derivation --no-write-lock-file "${packages[@]}")
    [[ ${#derivations[@]} -gt 0 ]] || return
    nix-store --query --requisites --include-outputs "${derivations[@]}" |
        grep -v '\.drv$' |
        XDG_CONFIG_HOME=$config "$attic/bin/attic" push --stdin nativelink
}

echo "--- :nix: Pushing to Attic"
push || echo "^^^ +++ Attic push failed; the build result stands."
