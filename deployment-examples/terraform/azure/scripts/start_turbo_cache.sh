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

# Starts turbo-cache remote execution service.
#
# This service will try and figure out what kind of service
# it should morph into and then configure and launch it.

set -euxo pipefail

# Try to mount any volumes if needed. Ignore any errors that might occur.
~/create_filesystem.sh /mnt/data || true

# Fetch Azure metadata
INSTANCE_ID=$(curl -H Metadata:true --silent "http://169.254.169.254/metadata/instance/compute/vmId?api-version=2019-03-11&format=text")
AZURE_REGION=$(curl -H Metadata:true --silent "http://169.254.169.254/metadata/instance/compute/location?api-version=2019-03-11&format=text")

# Fetch resource tags
TAGS_JSON=$(az resource list --ids "/subscriptions/${subscription_id}/resourceGroups/${resource_group}/providers/Microsoft.Compute/virtualMachines/${INSTANCE_ID}" --query '[0].tags' --output json)

TOTAL_AVAIL_MEMORY=$(( $(grep MemFree /proc/meminfo | awk '{print $2}') * 10000 / 12 ))

# These environmental variables are used inside the json file resolution.
export TURBO_CACHE_AZURE_REGION="$AZURE_REGION"
export TURBO_CACHE_AZURE_CONTAINER=$(echo "$TAGS_JSON" | jq -r '.["turbo_cache_azure_container"]')

TYPE=$(echo "$TAGS_JSON" | jq -r '.["turbo_cache_instance_type"]')
if [ "$TYPE" == "avm_builder" ]; then
    az vm deallocate --name "$INSTANCE_ID" --resource-group "$resource_group"
    sleep 30
elif [ "$TYPE" == "cas" ]; then
    export TURBO_CACHE_AC_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    export TURBO_CACHE_CAS_INDEX_CACHE_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 20 ))
    export TURBO_CACHE_CAS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $TURBO_CACHE_AC_CONTENT_LIMIT - $TURBO_CACHE_CAS_INDEX_CACHE_LIMIT ))

    /usr/local/bin/turbo-cache /root/cas.json
elif [ "$TYPE" == "scheduler" ]; then
    export TURBO_CACHE_AC_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    export TURBO_CACHE_CAS_INDEX_CACHE_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 20 ))
    export TURBO_CACHE_CAS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $TURBO_CACHE_AC_CONTENT_LIMIT - $TURBO_CACHE_CAS_INDEX_CACHE_LIMIT ))

    /usr/local/bin/turbo-cache /root/scheduler.json
elif [ "$TYPE" == "worker" ]; then
    SCHEDULER_URL=$(echo "$TAGS_JSON" | jq -r '.["turbo_cache:scheduler_url"]')
    DISK_SIZE=$(df /mnt/data | tail -n 1 | awk '{ print $2 }')

    export MAX_WORKER_DISK_USAGE=$(( DISK_SIZE * 100 / 125 ))
    export SCHEDULER_ENDPOINT=$(echo "$TAGS_JSON" | jq -r '.["turbo_cache:scheduler_endpoint"]')

    mkdir -p /mnt/data
    mkdir -p /worker
    mount --bind /mnt/data /worker
    /usr/local/bin/turbo-cache /root/worker.json
fi
