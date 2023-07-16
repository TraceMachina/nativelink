#!/bin/bash

set -euo pipefail

export TMPDIR="${TEST_TMPDIR:-${TMPDIR:-/tmp}}"
tsan_suppresions_file=$(mktemp -t tsan_suppressions.XXXXXX)
trap "rm -f $tsan_suppresions_file" EXIT

cat <<EOF > "$tsan_suppresions_file"
race:std::rt::lang_start_internal
EOF

export TSAN_OPTIONS="suppressions=$tsan_suppresions_file"
export RUST_TEST_THREADS=1
# Note: We cannot use `exec` here or else our `trap` cleanup will not run.
"$@"
