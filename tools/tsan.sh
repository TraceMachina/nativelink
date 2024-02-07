#!/bin/bash
# Copyright 2022 The NativeLink Authors. All rights reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# This file is used to run tests under ThreadSanitizer. Generally it should
# be invoked under bazel using `bazel run --config tsan //{target_here}`.
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
