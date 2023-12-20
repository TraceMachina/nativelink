#!/bin/bash
# Copyright 2023 The Native Link Authors. All rights reserved.
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
PROMETHEUS_IP=$(kubernetes_prometheus_ip)

# Run bazel to populate some of the metrics.
"${BIT_BAZEL_BINARY:-}" \
    --output_base="$BAZEL_CACHE_DIR" \
    build \
    --config=lre \
    --remote_instance_name=main \
    --remote_cache=grpc://"$CACHE_IP":50051 \
    --remote_executor=grpc://"$SCHEDULER_IP":50052 \
    //local-remote-execution/examples:hello_lre

# Our service may take a few seconds to get started, so retry a few times.
all_contents=$(curl \
    --retry 5 \
    --retry-delay 0 \
    --retry-max-time 30 \
    http://"$PROMETHEUS_IP":50061/metrics
)

echo "$all_contents"

# Check static metrics in some of the stores. These settings are set
# in the config file of integration tests for the CAS.
echo 'Checking: nativelink_stores_AC_MAIN_STORE_evicting_map_max_bytes 500000000'
grep -q 'nativelink_stores_AC_MAIN_STORE_evicting_map_max_bytes 500000000' <<< "$all_contents"
echo 'Checking: nativelink_stores_AC_MAIN_STORE_read_buff_size_bytes 32768'
grep -q 'nativelink_stores_AC_MAIN_STORE_read_buff_size_bytes 32768' <<< "$all_contents"
echo 'Checking: nativelink_stores_AC_MAIN_STORE_evicting_map_max_bytes 500000000'
grep -q 'nativelink_stores_AC_MAIN_STORE_evicting_map_max_bytes 500000000' <<< "$all_contents"

# Ensure our store metrics are only published once.
count=$(grep -c 'nativelink_stores_AC_MAIN_STORE_evicting_map_max_bytes 500000000' <<< "$all_contents")
if [[ $count -ne 1 ]]; then
  echo "Expected to find 1 instance of CAS_MAIN_STORE, but found $count"
  exit 1
fi

# Check dynamic metrics in some of the stores.
# These are the most stable settings to test that are dymaic.
echo 'Checking: nativelink_stores_AC_MAIN_STORE_evicting_map_item_size_bytes{quantile="0.99"}'
grep -q 'nativelink_stores_AC_MAIN_STORE_evicting_map_item_size_bytes{quantile="0.99"}' <<< "$all_contents"
echo 'Checking: nativelink_stores_AC_MAIN_STORE_evicting_map_items_in_store_total 3'
grep -q 'nativelink_stores_AC_MAIN_STORE_evicting_map_items_in_store_total 3' <<< "$all_contents"
