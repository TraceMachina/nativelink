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
FROM ubuntu:noble-20250925@sha256:728785b59223d755e3e5c5af178fab1be7031f3522c5ccd7a0b32b80d8248123

# Set shell to bash and enable pipefail
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# Get Ubuntu packages
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
    gcc=4:13.2.0-7ubuntu1 \
    g++=4:13.2.0-7ubuntu1 \
    python3=3.12.3-0ubuntu2.1 \
    curl=8.5.0-2ubuntu10.6 \
    ca-certificates=20240203 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Get Rust
RUN curl --proto "=https" --tlsv1.2 -sSf https://raw.githubusercontent.com/rust-lang/rustup/54dd3d00fd20e64dc522517a0d7be4285570a2f1/rustup-init.sh | sh -s -- -y --default-toolchain=1.79.0

RUN echo "source \"$HOME/.cargo/env\"" >> "$HOME/.bashrc"
