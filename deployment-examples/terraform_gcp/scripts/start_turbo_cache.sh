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

function make_certs() {
    openssl req -nodes -x509 -sha256 -newkey rsa:2048 -keyout $HOME/key.pem -out $HOME/cert.pem -days 356 -subj '/CN=localhost'
}

# function maybe_setup_docker() {
#     if zfs list tank/docker; then
#         return
#     fi
#     service docker stop
#     rm -rf /var/lib/docker/
#     zfs create \
#         -V 100G \
#         -b 8192 \
#         tank/docker
#     mkfs.ext4 /dev/zvol/tank/docker
#     mount /dev/zvol/tank/docker /var/lib/docker
#     service docker start
#     gcloud auth configure-docker --quiet
# }

# Try to mount any volumes if needed. Ignore any errors that might occur.
~/create_filesystem.sh /mnt/data || true

TYPE=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-type -H "Metadata-Flavor: Google")

TOTAL_TOTAL_MEMORY=$(( $(grep MemTotal /proc/meminfo | awk '{print $2}') * 1000 ))
TOTAL_DISK_SIZE=$(( $(df /mnt/data | tail -n 1 | awk '{ print $2 }') * 1000 ))

export TURBO_CACHE_CAS_S3_BUCKET=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-cas-bucket -H "Metadata-Flavor: Google")
export TURBO_CACHE_AC_S3_BUCKET=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-ac-bucket -H "Metadata-Flavor: Google")

# Import our GCP secret envs.
# source /root/.gcp_secrets
# export AWS_ENDPOINT_URL=https://storage.googleapis.com
# export AWS_SECRET_ACCESS_KEY="$AWS_SECRET_ACCESS_KEY"
# export AWS_ACCESS_KEY_ID="$AWS_ACCESS_KEY_ID"

# maybe_setup_docker

if [ "$TYPE" == "ami_builder" ]; then
    sleep infinity
elif [ "$TYPE" == "cas" ]; then
    make_certs
    export TURBO_CACHE_CERT_FILE=$HOME/cert.pem
    export TURBO_CACHE_KEY_FILE=$HOME/key.pem

    # Use ~71% of memory and disk.
    TOTAL_AVAIL_MEMORY=$(( $TOTAL_TOTAL_MEMORY * 10 / 14 ))
    TOTAL_AVAIL_DISK_SIZE=$(( ${TOTAL_DISK_SIZE:-0} * 10 / 14 ))

    # 10% goes to AC.
    export TURBO_CACHE_AC_MEMORY_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    export TURBO_CACHE_AC_FS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_DISK_SIZE / 10 ))
    # Remaining goes to CAS.
    export TURBO_CACHE_CAS_MEMORY_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $TURBO_CACHE_AC_MEMORY_CONTENT_LIMIT ))
    export TURBO_CACHE_CAS_FS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_DISK_SIZE - $TURBO_CACHE_AC_FS_CONTENT_LIMIT ))

    /usr/local/bin/turbo-cache /root/cas.json
elif [ "$TYPE" == "scheduler" ]; then
    make_certs
    export TURBO_CACHE_CERT_FILE=$HOME/cert.pem
    export TURBO_CACHE_KEY_FILE=$HOME/key.pem

    export TURBO_CACHE_CAS_ENDPOINT=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-internal-cas-endpoint -H "Metadata-Flavor: Google")

    # Use ~83% of memory.
    TOTAL_AVAIL_MEMORY=$(( $TOTAL_TOTAL_MEMORY * 10 / 12 ))
    TOTAL_AVAIL_DISK_SIZE=$(( ${TOTAL_DISK_SIZE:-0} * 10 / 12 ))

    # 10% goes to AC.
    export TURBO_CACHE_AC_MEMORY_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    # Remaining goes to CAS.
    export TURBO_CACHE_CAS_MEMORY_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $TURBO_CACHE_AC_MEMORY_CONTENT_LIMIT ))

    /usr/local/bin/turbo-cache /root/scheduler.json
elif [ "$TYPE" == "worker" ]; then
    gcloud auth configure-docker --quiet
    export TURBO_CACHE_CAS_ENDPOINT=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-internal-cas-endpoint -H "Metadata-Flavor: Google")
    export TURBO_CACHE_SCHEDULER_ENDPOINT=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-internal-scheduler-endpoint -H "Metadata-Flavor: Google")

    # We don't cache AC on workers, so ~71% goes to CAS. To leave room for jobs to use disk space too.
    export TURBO_CACHE_CAS_FS_CONTENT_LIMIT=$(( ${TOTAL_DISK_SIZE:-0} * 10 / 15 ))

    if ! mountpoint /worker ; then
        mkdir -p /mnt/data
        mkdir -p /worker
        mount --bind /mnt/data /worker
    fi

    if ! mountpoint /var/lib/docker ; then
        service docker stop
        umount /var/lib/docker || true
        rm -rf /var/lib/docker/
        mkdir -p /mnt/data/docker
        mkdir -p /var/lib/docker
        mount --bind /mnt/data/docker /var/lib/docker
        service docker start
    fi
    /usr/local/bin/turbo-cache /root/worker.json
fi
