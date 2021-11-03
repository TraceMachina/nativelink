#!/bin/bash
# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

set -eu -o pipefail

cd "$(dirname $(realpath $0))"

RUST_FMT="./bazel-bin/external/raze__rustfmt_nightly__1_4_38/cargo_bin_rustfmt"
if [ ! -e "$RUST_FMT" ] ; then
  bazel build @raze__rustfmt_nightly__1_4_38//:cargo_bin_rustfmt
fi
if [ "${1:-}" != "" ]; then
  FILES="$@"
else
  FILES="$(find . -name '*.rs' ! -name '*.pb.rs')"
fi
echo "$FILES" | parallel -I% --max-args 1 $RUST_FMT --emit files --edition 2018 %
