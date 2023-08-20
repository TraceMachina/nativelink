#!/bin/bash
# Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

set -eu -o pipefail

cd "$(dirname $(realpath $0))"

RUST_FMT="$(find ./bazel-bin/external/rules_rust/tools/rustfmt/rustfmt.runfiles -ipath */bin/rustfmt | head -n1)"
if [ ! -e "$RUST_FMT" ] ; then
  bazel build @rules_rust//:rustfmt
fi
if [ "${1:-}" != "" ]; then
  FILES="$@"
else
  # If there's a permission denied it may return a non-zero exit code, so just ignore it.
  FILES="$(find . \
    -type f \
    -name '*.rs' \
    ! -name '*\.pb\.rs' \
    ! -ipath '.*/proto/genproto/lib\.rs' \
    ! -ipath '*/.*' \
    ! -ipath '*/target/*' \
  2> >(grep -v 'Permission denied' >&2) || true)"
fi
export BUILD_WORKSPACE_DIRECTORY=$PWD
echo "$FILES" | parallel -I% --max-args 1 $RUST_FMT --emit files --edition 2021 %
