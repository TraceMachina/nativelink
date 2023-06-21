# Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

FROM ubuntu:18.04 AS builder

# Install bazel and needed deps.
RUN apt update && \
    DEBIAN_FRONTEND=noninteractive apt install --no-install-recommends -y \
        npm \
        git \
        pkg-config \
        libssl1.0-dev \
        gcc \
        g++ \
        python3 \
        ca-certificates && \
    npm install -g @bazel/bazelisk

WORKDIR /root/turbo-cache
ADD . .

# Compile `cas` binary.
RUN bazel build --compilation_mode=opt //cas && \
    cp ./bazel-bin/cas/cas /root/turbo-cache-bin

# Go back to a fresh ubutu container and copy the compiled binary and
# install the Chromium build dependencies

FROM ubuntu:18.04 AS chromium-worker

# Install runtime packages. and dependencies of install-build-deps.py, ironically
RUN apt update && \
    DEBIAN_FRONTEND=noninteractive apt install --no-install-recommends -y \
        libssl1.0-dev \
        lsb-release sudo file python3 \
    && rm -rf /var/lib/apt/lists/*

ADD https://chromium.googlesource.com/chromium/src/+/refs/heads/main/build/install-build-deps.py?format=TEXT /tmp/install-build-deps.py.b64

RUN base64 -d /tmp/install-build-deps.py.b64 > /tmp/install-build-deps.py && \
    chmod +x /tmp/install-build-deps.py && \
    /tmp/install-build-deps.py --no-prompt --no-arm --no-chromeos-fonts --no-nacl \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /root/.cache/turbo-cache

EXPOSE 50051/tcp 50052/tcp

COPY --from=builder /root/turbo-cache-bin /usr/local/bin/turbo-cache

CMD ["turbo-cache"]

# Go back to a fresh ubuntu container and copy only the compiled binary.
FROM ubuntu:18.04 AS turbo-cache

# Install runtime packages.
RUN apt update && \
    DEBIAN_FRONTEND=noninteractive apt install --no-install-recommends -y \
        libssl1.0-dev \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /root/.cache/turbo-cache

EXPOSE 50051/tcp 50052/tcp

COPY --from=builder /root/turbo-cache-bin /usr/local/bin/turbo-cache

CMD ["turbo-cache"]
