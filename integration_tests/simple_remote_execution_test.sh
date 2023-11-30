#!/bin/bash
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

# This test is to ensure we can run the same job two times on a single node and no errors occur
# and the results are said to be executed remotely by bazel.
# This test is also here to ensure GrpcStore is being used properly.

if [[ $UNDER_TEST_RUNNER -ne 1 ]]; then
  echo "This script should be run under run_integration_tests.sh" 
  exit 1
fi
set -xo pipefail

rm -rf "$CACHE_DIR/build_events.json"

# First run our test under bazel. It should not be cached.
OUTPUT=$(
   bazel --output_base="$BAZEL_CACHE_DIR" \
   test --config self_test --config self_execute \
   //:dummy_test --nocache_test_results --build_event_json_file="$CACHE_DIR/build_events.json"
)
STRATEGY=$(jq --slurp -r '.[] | select(.id.testResult.label=="//:dummy_test") | .testResult.executionInfo.strategy' "$CACHE_DIR/build_events.json")
if [[ "$STRATEGY" != "remote" ]]; then
   echo "$OUTPUT"
   echo ""
   echo "Expected to have executed remotely, but says: $STRATEGY"
   exit 1
fi

# Clean our local cache.
bazel --output_base="$BAZEL_CACHE_DIR" clean
rm -rf "$CACHE_DIR/build_events.json"

# Now run it under bazel again. This time the remote cache should have it.
OUTPUT=$(
   bazel --output_base="$BAZEL_CACHE_DIR" \
   test --config self_test --config self_execute \
   //:dummy_test --nocache_test_results --build_event_json_file="$CACHE_DIR/build_events.json"
)
STRATEGY=$(jq --slurp -r '.[] | select(.id.testResult.label=="//:dummy_test") | .testResult.executionInfo.strategy' "$CACHE_DIR/build_events.json")
if [[ "$STRATEGY" != "remote" ]]; then
   echo "$OUTPUT"
   echo ""
   echo "Expected to have executed remotely, but says: $STRATEGY"
   exit 1
fi
