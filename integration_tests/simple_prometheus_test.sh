#!/bin/bash
# Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

set -euo pipefail

# Run bazel to populate some of the metrics.
bazel --output_base="$BAZEL_CACHE_DIR" test --config self_test //:dummy_test

# Our service may take a few seconds to get started, so retry a few times.
all_contents="$(curl --retry 5 --retry-delay 0 --retry-max-time 30 http://127.0.0.1:50051/metrics)"

echo "$all_contents"

# Check static metrics in some of the stores. These settings are set
# in the config file of integration tests for the CAS.
echo 'Checking: turbo_cache_stores_AC_MAIN_STORE_evicting_map_max_bytes 500000000'
grep -q 'turbo_cache_stores_AC_MAIN_STORE_evicting_map_max_bytes 500000000' <<< "$all_contents"
echo 'Checking: turbo_cache_stores_AC_MAIN_STORE_read_buff_size_bytes 32768'
grep -q 'turbo_cache_stores_AC_MAIN_STORE_read_buff_size_bytes 32768' <<< "$all_contents"
echo 'Checking: turbo_cache_stores_CAS_MAIN_STORE_evicting_map_max_bytes 10000000000'
grep -q 'turbo_cache_stores_CAS_MAIN_STORE_evicting_map_max_bytes 10000000000' <<< "$all_contents"

# Check dynamic metrics in some of the stores.
# These are the most stable settings to test that are dymaic.
echo 'Checking: turbo_cache_stores_AC_MAIN_STORE_evicting_map_item_size_bytes{quantile="0.99"}'
grep -q 'turbo_cache_stores_AC_MAIN_STORE_evicting_map_item_size_bytes{quantile="0.99"}' <<< "$all_contents"
echo 'Checking: turbo_cache_stores_AC_MAIN_STORE_evicting_map_items_in_store_total 3'
grep -q 'turbo_cache_stores_AC_MAIN_STORE_evicting_map_items_in_store_total 3' <<< "$all_contents"
