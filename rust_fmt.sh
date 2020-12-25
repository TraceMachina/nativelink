#!/bin/bash
# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

set -eu -o pipefail

cd "$(dirname $(realpath $0))"

RUST_FMT="./bazel-bin/external/raze__rustfmt_nightly__1_4_21/cargo_bin_rustfmt"
if [ "${1:-}" != "" ]; then
  FILES="$@"
else
  FILES="$(find . -name '*.rs')"
fi
echo "$FILES" | xargs $RUST_FMT --emit files --edition 2018
