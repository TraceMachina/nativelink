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

# Starts nativelink remote execution service.
#
# This service will try and figure out what kind of service
# it should morph into and then configure and launch it.

set -euxo pipefail

# Try to mount any volumes if needed. Ignore any errors that might occur.
~/create_filesystem.sh /mnt/data || true

INSTANCE_ID=$(curl --silent http://instance-data/latest/meta-data/instance-id)
AWS_REGION=$(curl --silent http://instance-data/latest/meta-data/placement/availability-zone | rev | cut -c2- | rev)

TAGS=$(aws ec2 describe-tags --filters "Name=resource-id,Values=$INSTANCE_ID" --region "$AWS_REGION")

TOTAL_AVAIL_MEMORY=$(( $(grep MemFree /proc/meminfo | awk '{print $2}') * 10000 / 12 ))

# These environmental variables are used inside the json file resolution.
export NATIVELINK_AWS_REGION="$AWS_REGION"
export NATIVELINK_AWS_S3_CAS_BUCKET=$(echo "$TAGS"| jq -r '.Tags[] | select(.Key == "nativelink:s3_cas_bucket").Value')

TYPE=$(echo "$TAGS"| jq -r '.Tags[] | select(.Key == "nativelink:instance_type").Value')
if [ "$TYPE" == "ami_builder" ]; then
    # This instance type is special. It is not used in production, but due to terraform being unable
    # to delete/stop instances, we wait for this instance to restart after the AMI is created then
    # just terminate it.
    aws ec2 terminate-instances --region "$AWS_REGION" --instance-ids "$INSTANCE_ID"
    sleep 30 # This is so the service doesn't try to run this script again immediately.
elif [ "$TYPE" == "cas" ]; then
    # 10% goes to action cache.
    export NATIVELINK_AC_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    # 5% goes to the index memory cache.
    export NATIVELINK_CAS_INDEX_CACHE_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 20 ))
    # 85% goes to the content memory cache.
    export NATIVELINK_CAS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $NATIVELINK_AC_CONTENT_LIMIT - $NATIVELINK_CAS_INDEX_CACHE_LIMIT ))

    /usr/local/bin/nativelink /root/cas.json
elif [ "$TYPE" == "scheduler" ]; then
    # 10% goes to action cache.
    export NATIVELINK_AC_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    # 5% goes to the index memory cache.
    export NATIVELINK_CAS_INDEX_CACHE_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 20 ))
    # 85% goes to the content memory cache.
    export NATIVELINK_CAS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $NATIVELINK_AC_CONTENT_LIMIT - $NATIVELINK_CAS_INDEX_CACHE_LIMIT ))

    /usr/local/bin/nativelink /root/scheduler.json
elif [ "$TYPE" == "worker" ]; then
    SCHEDULER_URL=$(echo "$TAGS"| jq -r '.Tags[] | select(.Key == "nativelink:scheduler_url").Value')
    DISK_SIZE=$(df /mnt/data | tail -n 1 | awk '{ print $2 }')

    # This is used inside the "worker.json" file.
    export MAX_WORKER_DISK_USAGE=$(( DISK_SIZE * 100 / 125  )) # Leave about 20% disk space for active tasks.
    export SCHEDULER_ENDPOINT=$(echo "$TAGS"| jq -r '.Tags[] | select(.Key == "nativelink:scheduler_endpoint").Value')

    mkdir -p /mnt/data
    mkdir -p /worker
    mount --bind /mnt/data /worker
    /usr/local/bin/nativelink /root/worker.json
fi
