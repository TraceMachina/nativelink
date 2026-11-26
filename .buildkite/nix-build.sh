#!/usr/bin/env bash
# Builds the Nix packages that CI / Nix builds on the cluster client.
set -euo pipefail

exec nix build -L --no-write-lock-file --no-link \
    .#nativelink \
    .#nativelink-is-executable-test \
    .#rbe-toolchain-with-nativelink-test \
    .#rbe-toolchain-list
