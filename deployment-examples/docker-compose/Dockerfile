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
ARG BAZELISK_URL=https://github.com/bazelbuild/bazelisk/releases/download/v1.25.0/bazelisk-linux-amd64
ARG BAZELISK_SHA256=fd8fdff418a1758887520fa42da7e6ae39aefc788cf5e7f7bb8db6934d279fc4
# `--compilation_mode` to pass into bazel (eg: opt, dbg, fastbuild).
ARG OPT_LEVEL=opt
# Additional bazel flags.
ARG ADDITIONAL_BAZEL_FLAGS=
# Bash arguments may be passed in here to install any additional dependencies
# needed by the user. Useful if your worker needs specific dependencies installed.
ARG ADDITIONAL_SETUP_WORKER_CMD=

FROM ubuntu:noble-20250925@sha256:728785b59223d755e3e5c5af178fab1be7031f3522c5ccd7a0b32b80d8248123 AS dependencies
ARG BAZELISK_URL
ARG BAZELISK_SHA256
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive \
    apt-get install --no-install-recommends -y \
        git=1:2.43.0-1ubuntu7.3 \
        gcc=4:13.2.0-7ubuntu1 \
        g++=4:13.2.0-7ubuntu1 \
        python3=3.12.3-0ubuntu2.1 \
        ca-certificates=20240203 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -c 'import hashlib, pathlib, stat, sys, urllib.request; data = urllib.request.urlopen(sys.argv[1], timeout=60).read(); digest = hashlib.sha256(data).hexdigest(); assert digest == sys.argv[2], f"sha256 mismatch: {digest}"; path = pathlib.Path("/usr/local/bin/bazel"); path.write_bytes(data); path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)' "$BAZELISK_URL" "$BAZELISK_SHA256"

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
FROM ubuntu:noble-20250925@sha256:728785b59223d755e3e5c5af178fab1be7031f3522c5ccd7a0b32b80d8248123 AS final
COPY --from=builder /root/nativelink-bin /usr/local/bin/nativelink

ARG ADDITIONAL_SETUP_WORKER_CMD

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl=8.5.0-2ubuntu10.9 \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && bash -ueo pipefail -c "${ADDITIONAL_SETUP_WORKER_CMD}" \
    && mkdir -p /root/.cache/nativelink

EXPOSE 50051/tcp 50052/tcp
CMD ["nativelink"]
