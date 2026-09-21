#!/bin/bash
# Copyright 2022 The NativeLink Authors. All rights reserved.
#
# Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    See LICENSE file for details
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# This is a sanity check test to ensure we are caching test results.

if [[ $UNDER_TEST_RUNNER -ne 1 ]]; then
    echo "This script should be run under run_integration_tests.sh"
    exit 1
fi

# This test uploads a locally-run test result to the cache and asserts the
# re-run is a cache hit, so it needs --remote_upload_local_results=true.
# nativelink.bazelrc sets that false repo-wide to stop PR/developer lanes from
# writing to the *shared* cache (cache-poisoning guard). Uploads here target
# only the ephemeral, per-run docker-compose cache, so they are safe -- but to
# keep the anti-poisoning default intact this test runs only where local
# uploads are explicitly enabled: merges to main and the scheduled canary set
# NL_LOCAL_UPLOADS=1 (see main.yaml / nativelink-cloud-canary.yaml).
if [[ ${NL_LOCAL_UPLOADS:-0} != "1" ]]; then
    echo "Skipping $(basename "$0"): requires local cache uploads (main/canary lanes only)."
    exit 0
fi
set -x

# A command-line flag overrides the repo-wide --remote_upload_local_results
# =false so this test can populate the cache it then asserts on.
TEST_FLAGS=(--config self_test --remote_upload_local_results=true)

# First run our test under bazel. It should not be cached.
OUTPUT=$(bazel --output_base="$BAZEL_CACHE_DIR" test "${TEST_FLAGS[@]}" //:dummy_test)
if [[ $OUTPUT =~ .*'(cached)'.* ]]; then
    echo "Expected first bazel run to not have test cached."
    echo "STDOUT:"
    echo "$OUTPUT"
    exit 1
fi

# Clean our local cache.
bazel --output_base="$BAZEL_CACHE_DIR" clean

# Now run it under bazel again. This time the remote cache should have it.
OUTPUT=$(bazel --output_base="$BAZEL_CACHE_DIR" test "${TEST_FLAGS[@]}" //:dummy_test)
if [[ ! $OUTPUT =~ .*'(cached)'.* ]]; then
    echo "Expected second bazel run to have test cached."
    echo "STDOUT:"
    echo "$OUTPUT"
    exit 1
fi
