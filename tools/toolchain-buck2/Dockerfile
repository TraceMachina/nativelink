# Copyright 2022-2024 The NativeLink Authors. All rights reserved.
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

# https://hub.docker.com/layers/library/ubuntu/noble-20250925/images/sha256-78281ac7684a7caf02348780a1b5de85844548a3cc0505df924de98380a0eeea
FROM ubuntu:noble-20250925@sha256:728785b59223d755e3e5c5af178fab1be7031f3522c5ccd7a0b32b80d8248123 AS dependencies
RUN apt-get update && DEBIAN_FRONTEND=noninteractive \
    apt-get install -y --no-install-recommends \
        git=1:2.43.0-1ubuntu7.3 \
        ca-certificates=20240203 \
        curl=8.5.0-2ubuntu10.6 \
        python3=3.12.3-0ubuntu2.1 \
        && apt-get clean \
        && rm -rf /var/lib/apt/lists/* \
        && update-ca-certificates

RUN curl -L https://go.dev/dl/go1.23.0.linux-amd64.tar.gz -o go1.23.0.linux-amd64.tar.gz && \
    rm -rf /usr/local/go && \
    tar -C /usr/local -xzf go1.23.0.linux-amd64.tar.gz && \
    rm go1.23.0.linux-amd64.tar.gz

# hadolint ignore=SC1091,DL4006
RUN curl -sL -o nix-installer https://install.determinate.systems/nix/nix-installer-x86_64-linux && \
    chmod +x nix-installer && \
    ./nix-installer install linux --init none --no-confirm && \
    . /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh && \
    nix develop github:facebook/buck2/d76c189ed6092d7b53506b9411241680923d593b && \
    echo "export PATH=\"$(nix develop github:facebook/buck2/d76c189ed6092d7b53506b9411241680923d593b --command env | grep '^PATH=' | cut -d '=' -f 2-):/usr/local/go/bin\"" > /etc/profile.d/set_path.sh
