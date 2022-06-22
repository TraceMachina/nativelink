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
  # If there's a permission denied it may return a non-zero exit code, so just ignore it.
  FILES="$(find . \
    -type f \
    -name '*.rs' \
    ! -name '*.pb.rs' \
    ! -ipath '*/.*' \
    ! -ipath '*/target/*' \
  2> >(grep -v 'Permission denied' >&2) || true)"
fi
echo "$FILES" | parallel -I% --max-args 1 $RUST_FMT --emit files --edition 2018 %
