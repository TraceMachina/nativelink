#!/bin/bash
# Copyright 2024 The Native Link Authors. All rights reserved.
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

set -euo pipefail

RUST_MAIN_CODE=("nativelink-config/src/"
"nativelink-error/src/"
"nativelink-scheduler/src/"
"nativelink-service/src/"
"nativelink-store/src/"
"nativelink-util/src/"
"nativelink-worker/src/"
"src/bin/");
CFG_ATTR="enable_tracing"
GIT_ROOT=$(git rev-parse --show-toplevel)

function run_clippy_tracing() {
  action="$1"
  for path in "${RUST_MAIN_CODE[@]}"; do
    echo "$action: $path"
    clippy-tracing --action "$action" --cfg-attr="$CFG_ATTR" --path "$path"
  done
}

if [ $# -ne 1 ]; then
  echo "Usage: $0 <command>"
  exit 1
fi

command="$1"
pushd $GIT_ROOT
case "$command" in
  "strip")
    run_clippy_tracing "strip"
    ;;
  "check")
    run_clippy_tracing "check"
    ;;
  "fix")
    run_clippy_tracing "fix"
    ;;
  *)
    echo "Invalid command: $command"
    popd
    exit 1
    ;;
esac
popd
