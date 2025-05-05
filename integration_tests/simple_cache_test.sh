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

# This is a sanity check test to ensure we are caching test results.

if [[ $UNDER_TEST_RUNNER -ne 1 ]]; then
    echo "This script should be run under run_integration_tests.sh"
    exit 1
fi
set -x

# First run our test under bazel. It should not be cached.
OUTPUT=$(bazel --output_base="$BAZEL_CACHE_DIR" test --config self_test //:dummy_test)
if [[ $OUTPUT =~ .*'(cached)'.* ]]; then
    echo "Expected first bazel run to not have test cached."
    echo "STDOUT:"
    echo "$OUTPUT"
    exit 1
fi

# Clean our local cache.
bazel --output_base="$BAZEL_CACHE_DIR" clean

# Now run it under bazel again. This time the remote cache should have it.
OUTPUT=$(bazel --output_base="$BAZEL_CACHE_DIR" test --config self_test //:dummy_test)
if [[ ! $OUTPUT =~ .*'(cached)'.* ]]; then
    echo "Expected second bazel run to have test cached."
    echo "STDOUT:"
    echo "$OUTPUT"
    exit 1
fi
