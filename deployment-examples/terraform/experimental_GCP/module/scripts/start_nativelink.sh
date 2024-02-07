#!/bin/bash
# Copyright 2023 The NativeLink Authors. All rights reserved.
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

function make_certs_and_export_envs() {
    openssl req -nodes -x509 -sha256 -newkey rsa:2048 -keyout /root/key.pem -out /root/cert.pem -days 356 -subj '/CN=localhost'
    NATIVELINK_CERT_FILE=/root/cert.pem
    NATIVELINK_KEY_FILE=/root/key.pem
    export NATIVELINK_CERT_FILE
    export NATIVELINK_KEY_FILE
}

function setup_s3_envs() {
    AWS_JOINED_KEYS=$(gcloud secrets versions access latest --secret=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/nativelink-hmac-secret-key -H "Metadata-Flavor: Google"))
    AWS_ENDPOINT_URL=https://storage.googleapis.com
    AWS_ACCESS_KEY_ID="$(echo "$AWS_JOINED_KEYS" | cut -d ":" -f 1)"
    AWS_SECRET_ACCESS_KEY="$(echo "$AWS_JOINED_KEYS" | cut -d ":" -f 2)"
    cas_s3_bucket_pair=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/nativelink-cas-bucket -H "Metadata-Flavor: Google")
    NATIVELINK_CAS_S3_BUCKET="$(echo "$cas_s3_bucket_pair" | cut -d ":" -f 1)"
    NATIVELINK_CAS_S3_BUCKET_REGION="$(echo "$cas_s3_bucket_pair" | cut -d ":" -f 2)"
    ac_s3_bucket_pair=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/nativelink-ac-bucket -H "Metadata-Flavor: Google")
    NATIVELINK_AC_S3_BUCKET="$(echo "$ac_s3_bucket_pair" | cut -d ":" -f 1)"
    NATIVELINK_AC_S3_BUCKET_REGION="$(echo "$ac_s3_bucket_pair" | cut -d ":" -f 2)"
    AWS_DEFAULT_REGION="$(curl http://metadata.google.internal/computeMetadata/v1/instance/zone -H Metadata-Flavor:Google | cut '-d/' -f4)"

    export AWS_ENDPOINT_URL
    export AWS_ACCESS_KEY_ID
    export AWS_SECRET_ACCESS_KEY
    export NATIVELINK_CAS_S3_BUCKET
    export NATIVELINK_CAS_S3_BUCKET_REGION
    export NATIVELINK_AC_S3_BUCKET
    export NATIVELINK_AC_S3_BUCKET_REGION
    export AWS_DEFAULT_REGION
}

function setup_publisher_cron() {
    project_id=$(curl http://metadata.google.internal/computeMetadata/v1/project/project-id -H "Metadata-Flavor: Google")
    echo "* * * * * root sh -c 'python3 /root/cloud_publisher.py --url http://127.0.0.1:50052/metrics --project $project_id'" | sudo tee -a /etc/crontab
    sudo service cron reload
}

# Try to mount any volumes if needed. Ignore any errors that might occur.
~/create_filesystem.sh /mnt/data || true

type=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/nativelink-type -H "Metadata-Flavor: Google")

total_total_memory=$(( $(grep MemTotal /proc/meminfo | awk '{print $2}') * 1000 ))

if [ "$type" == "browser" ]; then
    setup_s3_envs

    # We want nativelink to start first so bb_browser can connect to it.
    /usr/local/bin/nativelink /root/browser_proxy.json &

    # Install docker and bb_browser.
    DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y apt docker.io

    # Run BB Browser.
    docker run \
        --rm \
        --net host \
        -v /root/bb_browser_config.json:/root/bb_browser_config.json \
        ghcr.io/buildbarn/bb-browser@sha256:e0eb5e229b83e5d6c4fe5619a1436610e7c5e3d83fbc3ad1346366dd201b52c5 \
        /root/bb_browser_config.json
    wait
elif [ "$type" == "cas" ]; then
    make_certs_and_export_envs
    setup_s3_envs

    # Use ~71% of memory and disk.
    total_avail_memory=$(( $total_total_memory * 10 / 14 ))
    total_avail_disk_size=$(( ${total_disk_size:-0} * 10 / 14 ))

    # 10% goes to AC.
    NATIVELINK_AC_MEMORY_CONTENT_LIMIT=$(( $total_avail_memory / 10 ))
    NATIVELINK_AC_FS_CONTENT_LIMIT=$(( $total_avail_disk_size / 10 ))
    # Remaining goes to CAS.
    NATIVELINK_CAS_MEMORY_CONTENT_LIMIT=$(( $total_avail_memory - $NATIVELINK_AC_MEMORY_CONTENT_LIMIT ))
    NATIVELINK_CAS_FS_CONTENT_LIMIT=$(( $total_avail_disk_size - $NATIVELINK_AC_FS_CONTENT_LIMIT ))

    export NATIVELINK_AC_MEMORY_CONTENT_LIMIT
    export NATIVELINK_AC_FS_CONTENT_LIMIT
    export NATIVELINK_CAS_MEMORY_CONTENT_LIMIT
    export NATIVELINK_CAS_FS_CONTENT_LIMIT

    /usr/local/bin/nativelink /root/cas.json
elif [ "$type" == "scheduler" ]; then
    setup_s3_envs
    make_certs_and_export_envs
    setup_publisher_cron

    # Use ~83% of memory.
    total_avail_memory=$(( $total_total_memory * 10 / 12 ))

    # 10% goes to AC.
    NATIVELINK_AC_MEMORY_CONTENT_LIMIT=$(( $total_avail_memory / 10 ))
    # Remaining goes to CAS.
    NATIVELINK_CAS_MEMORY_CONTENT_LIMIT=$(( $total_avail_memory - $NATIVELINK_AC_MEMORY_CONTENT_LIMIT ))

    export NATIVELINK_AC_MEMORY_CONTENT_LIMIT
    export NATIVELINK_CAS_MEMORY_CONTENT_LIMIT

    /usr/local/bin/nativelink /root/scheduler.json
elif [ "$type" == "worker" ]; then
    setup_s3_envs

    NATIVELINK_SCHEDULER_ENDPOINT=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/nativelink-internal-worker-scheduler-endpoint -H "Metadata-Flavor: Google")
    NATIVELINK_BROWSER_ENDPOINT=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/nativelink-browser-endpoint -H "Metadata-Flavor: Google")

    total_disk_size=$(( $(df /mnt/data | tail -n 1 | awk '{ print $2 }') * 1000 ))

    # We don't cache AC on workers, so ~71% goes to CAS. To leave room for jobs to use disk space too.
    NATIVELINK_CAS_FS_CONTENT_LIMIT=$(( ${total_disk_size:-0} * 10 / 15 ))

    export NATIVELINK_SCHEDULER_ENDPOINT
    export NATIVELINK_BROWSER_ENDPOINT
    export NATIVELINK_CAS_FS_CONTENT_LIMIT

    if ! mountpoint /worker ; then
        mkdir -p /mnt/data
        mkdir -p /worker
        mount --bind /mnt/data /worker
    fi

    /usr/local/bin/nativelink /root/worker.json
else
    echo "Unknown nativelink type: $type" >&2
    echo 1
fi
