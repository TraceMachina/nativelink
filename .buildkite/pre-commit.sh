#!/usr/bin/env bash
# What CI / Pre-commit checks: the CI scripts' tests and nix flake check.
set -euo pipefail

# The image's Python has no pip, so the venv uses the flake's. The pinned
# wheels are for CPython 3.12 and need a libstdc++ that Nix's Python does not
# look for.
python=(nix shell --no-write-lock-file --inputs-from . nixpkgs#python312 --command)
libstdcxx=$(nix build --no-link --print-out-paths --no-write-lock-file \
    --inputs-from . nixpkgs#stdenv.cc.cc.lib)

echo "--- :python: CI script tests"
python3 -m unittest discover -s .github/scripts -p 'test_ci*.py'

echo "--- :python: Fork build-results replay tests"
venv=$(mktemp -d)
trap 'rm -rf "$venv"' EXIT
"${python[@]}" python3 -m venv "$venv"
"$venv/bin/pip" install --quiet --disable-pip-version-check --require-hashes \
    --only-binary=:all: -r .github/scripts/requirements-bep-replay.txt
LD_LIBRARY_PATH=$libstdcxx/lib BEP_REPLAY_TESTS_REQUIRED=1 "$venv/bin/python" -m unittest discover \
    -s .github/scripts -p 'test_ci_bep_replay.py'

echo "--- :nix: nix flake check"
nix flake check --no-write-lock-file
