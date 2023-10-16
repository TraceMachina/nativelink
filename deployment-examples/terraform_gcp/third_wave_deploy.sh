#!/bin/bash

sudo DEBIAN_FRONTEND=noninteractive apt-get update
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y gcc g++ lld libssl-dev pkg-config docker.io jq
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
git clone https://github.com/TraceMachina/turbo-cache.git
cd turbo-cache
cargo build --release --bin cas
sudo mv ./target/release/cas /usr/local/bin/turbo-cache

sudo cp ./deployment-examples/terraform/scripts/scheduler.json /root/scheduler.json
sudo cp ./deployment-examples/terraform/scripts/cas.json /root/cas.json
sudo cp ./deployment-examples/terraform/scripts/worker.json /root/worker.json
sudo cp ./deployment-examples/terraform/scripts/start_turbo_cache.sh /root/start_turbo_cache.sh
sudo cp ./deployment-examples/terraform/scripts/create_filesystem.sh /root/create_filesystem.sh

cat <<'EOF' | sudo tee /root/start_turbo_cache.sh > /dev/null
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

TYPE=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-type -H "Metadata-Flavor: Google")

TOTAL_AVAIL_MEMORY=$(( $(grep MemFree /proc/meminfo | awk '{print $2}') * 10000 / 12 ))

# These environmental variables are used inside the json file resolution.
# export TURBO_CACHE_AWS_REGION="$AWS_REGION"
# export TURBO_CACHE_AWS_S3_CAS_BUCKET=$(echo "$TAGS"| jq -r '.Tags[] | select(.Key == "turbo_cache:s3_cas_bucket").Value')

# TYPE=$(echo "$TAGS"| jq -r '.Tags[] | select(.Key == "turbo_cache:instance_type").Value')
# if [ "$TYPE" == "ami_builder" ]; then
#     # This instance type is special. It is not used in production, but due to terraform being unable
#     # to delete/stop instances, we wait for this instance to restart after the AMI is created then
#     # just terminate it.
#     aws ec2 terminate-instances --region "$AWS_REGION" --instance-ids "$INSTANCE_ID"
#     sleep 30 # This is so the service doesn't try to run this script again immediately.
if [ "$TYPE" == "cas" ]; then
    # 10% goes to action cache.
    export TURBO_CACHE_AC_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    # 5% goes to the index memory cache.
    export TURBO_CACHE_CAS_INDEX_CACHE_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 20 ))
    # 85% goes to the content memory cache.
    export TURBO_CACHE_CAS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $TURBO_CACHE_AC_CONTENT_LIMIT - $TURBO_CACHE_CAS_INDEX_CACHE_LIMIT ))

    /usr/local/bin/turbo-cache /root/cas.json
elif [ "$TYPE" == "scheduler" ]; then
    # 10% goes to action cache.
    export TURBO_CACHE_AC_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 10 ))
    # 5% goes to the index memory cache.
    export TURBO_CACHE_CAS_INDEX_CACHE_LIMIT=$(( $TOTAL_AVAIL_MEMORY / 20 ))
    # 85% goes to the content memory cache.
    export TURBO_CACHE_CAS_CONTENT_LIMIT=$(( $TOTAL_AVAIL_MEMORY - $TURBO_CACHE_AC_CONTENT_LIMIT - $TURBO_CACHE_CAS_INDEX_CACHE_LIMIT ))

    /usr/local/bin/turbo-cache /root/scheduler.json
elif [ "$TYPE" == "worker" ]; then
    export SCHEDULER_ENDPOINT=$(curl http://metadata.google.internal/computeMetadata/v1/instance/attributes/turbo-cache-scheduler -H "Metadata-Flavor: Google")
    DISK_SIZE=$(df /mnt/data | tail -n 1 | awk '{ print $2 }')

    # This is used inside the "worker.json" file.
    export MAX_WORKER_DISK_USAGE=$(( DISK_SIZE * 100 / 125  )) # Leave about 20% disk space for active tasks.

    mkdir -p /mnt/data
    mkdir -p /worker
    mount --bind /mnt/data /worker
    /usr/local/bin/turbo-cache /root/worker.json
fi
EOF

sudo chmod +x /root/start_turbo_cache.sh

cat <<'EOT' | sudo tee /etc/systemd/system/turbo-cache.service > /dev/null
[Unit]
Description=Turbo Cache remote execution system
After=network.target

[Service]
Type=simple
Restart=always
RestartSec=1
User=root
ExecStart=/root/start_turbo_cache.sh

[Install]
WantedBy=multi-user.target
EOT

# sudo systemctl enable turbo-cache

sync

# Now create image "tubo-cache-image" as disk
