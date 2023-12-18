#!/usr/bin/env bash
# Copyright 2022 The Native Link Authors. All rights reserved.
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

# This is a sanity check test to ensure we are caching test results.

# --- begin runfiles.bash initialization v3 ---
# Copy-pasted from the Bazel Bash runfiles library v3.
set -uo pipefail; set +e; f=bazel_tools/tools/bash/runfiles/runfiles.bash
source "${RUNFILES_DIR:-/dev/null}/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "${RUNFILES_MANIFEST_FILE:-/dev/null}" | cut -f2- -d' ')" 2>/dev/null || \
  source "$0.runfiles/$f" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  source "$(grep -sm1 "^$f " "$0.exe.runfiles_manifest" | cut -f2- -d' ')" 2>/dev/null || \
  { echo>&2 "ERROR: cannot find $f"; exit 1; }; f=; set -e
# --- end runfiles.bash initialization v3 ---

set -xeuo pipefail

source "$(rlocation nativelink/deployment-examples/kubernetes/bazel_k8s_prelude.sh)"
source "$(rlocation nativelink/tools/integration_test_utils.sh)"

BAZEL_CACHE_DIR=$(temporary_cache)/bazel
CACHE_IP=$(kubernetes_insecure_cache_ip)
SCHEDULER_IP=$(kubernetes_scheduler_ip)

# First run our test under bazel. It should not be cached.
OUTPUT=$("${BIT_BAZEL_BINARY:-}" \
    --output_base="$BAZEL_CACHE_DIR" \
    test \
    --config=lre \
    --remote_instance_name=main \
    --remote_cache=grpc://"$CACHE_IP":50051 \
    --remote_executor=grpc://"$SCHEDULER_IP":50052 \
    //:dummy_test)

if [[ "$OUTPUT" =~ .*'(cached)'.* ]]; then
   echo "Expected first bazel run to not have test cached."
   echo "STDOUT:"
   echo "$OUTPUT"
   exit 1
fi

# Clean our local cache.
"${BIT_BAZEL_BINARY:-}" --output_base="$BAZEL_CACHE_DIR" clean

# Now run it under bazel again. This time the remote cache should have it.
OUTPUT=$("${BIT_BAZEL_BINARY:-}" \
    --output_base="$BAZEL_CACHE_DIR" \
    test \
    --config=lre \
    --remote_instance_name=main \
    --remote_cache=grpc://"$CACHE_IP":50051 \
    --remote_executor=grpc://"$SCHEDULER_IP":50052 \
    //:dummy_test)

if [[ ! "$OUTPUT" =~ .*'(cached)'.* ]]; then
   echo "Expected second bazel run to have test cached."
   echo "STDOUT:"
   echo "$OUTPUT"
   exit 1
fi
