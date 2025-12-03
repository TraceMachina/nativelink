# Copyright 2022-2025 The NativeLink Authors. All rights reserved.
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

# Current supported Ubuntu version, Noble Numbat aka 24.04 LTS
# Locked down to a specific revision to avoid issues with package versions
ARG OS_VERSION=noble-20250925@sha256:728785b59223d755e3e5c5af178fab1be7031f3522c5ccd7a0b32b80d8248123
# `--compilation_mode` to pass into bazel (eg: opt, dbg, fastbuild).
ARG OPT_LEVEL=opt
# Additional bazel flags.
ARG ADDITIONAL_BAZEL_FLAGS=
# Bash arguments may be passed in here to install any additional dependencies
# needed by the user. Useful if your worker needs specific dependencies installed.
ARG ADDITIONAL_SETUP_WORKER_CMD=

FROM ubuntu:${OS_VERSION} AS dependencies
ARG OS_VERSION
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive \
    apt-get install --no-install-recommends -y \
        npm=9.2.0~ds1-2 \
        git=1:2.43.0-1ubuntu7.3 \
        gcc=4:13.2.0-7ubuntu1 \
        g++=4:13.2.0-7ubuntu1 \
        python3=3.12.3-0ubuntu2.1 \
        ca-certificates=20240203 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && npm install -g @bazel/bazelisk@1.25.0

# Build the binary.
FROM dependencies AS builder
WORKDIR /root/nativelink
COPY . .
ARG OPT_LEVEL
ARG ADDITIONAL_BAZEL_FLAGS
# Add startup options to prevent Bazel server issues on ARM64
RUN bazel \
    --batch \
    --output_base=/tmp/bazel_output \
    build -c ${OPT_LEVEL} ${ADDITIONAL_BAZEL_FLAGS} nativelink && \
    cp ./bazel-bin/nativelink /root/nativelink-bin

# Go back to a fresh ubuntu container and copy only the compiled binary.
FROM ubuntu:${OS_VERSION} AS final
ARG OS_VERSION
COPY --from=builder /root/nativelink-bin /usr/local/bin/nativelink

ARG ADDITIONAL_SETUP_WORKER_CMD

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl=8.5.0-2ubuntu10.6 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && bash -ueo pipefail -c "${ADDITIONAL_SETUP_WORKER_CMD}" \
    && mkdir -p /root/.cache/nativelink

EXPOSE 50051/tcp 50052/tcp
CMD ["nativelink"]
