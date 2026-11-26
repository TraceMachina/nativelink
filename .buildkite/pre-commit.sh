#!/usr/bin/env bash
# What CI / Pre-commit checks: the CI scripts' tests and nix flake check.
set -euo pipefail

# The image's Python has no pip, so the venv uses the flake's.
python=(nix shell --no-write-lock-file --inputs-from . nixpkgs#python3 --command)

echo "--- :python: CI script tests"
python3 -m unittest discover -s .github/scripts -p 'test_ci*.py'

echo "--- :python: Fork build-results replay tests"
venv=$(mktemp -d)
trap 'rm -rf "$venv"' EXIT
"${python[@]}" python3 -m venv "$venv"
"$venv/bin/pip" install --quiet --require-hashes --only-binary=:all: \
    -r .github/scripts/requirements-bep-replay.txt
BEP_REPLAY_TESTS_REQUIRED=1 "$venv/bin/python" -m unittest discover \
    -s .github/scripts -p 'test_ci_bep_replay.py'

echo "--- :nix: nix flake check"
nix flake check --no-write-lock-file
