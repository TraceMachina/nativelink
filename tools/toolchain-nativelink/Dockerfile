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

FROM ubuntu:22.04@sha256:09506232a8004baa32c47d68f1e5c307d648fdd59f5e7eaa42aaf87914100db3

# Set shell to bash and enable pipefail
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# Get Ubuntu packages
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
    gcc=4:11.2.0-1ubuntu1 \
    g++=4:11.2.0-1ubuntu1 \
    python3=3.10.6-1~22.04 \
    python3-minimal=3.10.6-1~22.04 \
    libpython3-stdlib=3.10.6-1~22.04 \
    curl=7.81.0-1ubuntu1.20 \
    ca-certificates=20240203~22.04.1 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Get Rust
RUN curl --proto "=https" --tlsv1.2 -sSf https://raw.githubusercontent.com/rust-lang/rustup/54dd3d00fd20e64dc522517a0d7be4285570a2f1/rustup-init.sh | sh -s -- -y --default-toolchain=1.79.0

RUN echo "source \"$HOME/.cargo/env\"" >> "$HOME/.bashrc"
