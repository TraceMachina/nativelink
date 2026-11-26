#!/usr/bin/env bash
# What CI / Pre-commit checks, except the tests of the fork build-results
# replay: only GitHub's fork workflow uses that script, and GitHub runs them.
set -euo pipefail

echo "--- :python: CI script tests"
python3 -m unittest discover -s .github/scripts -p 'test_ci*.py'

echo "--- :nix: nix flake check"
nix flake check --no-write-lock-file
