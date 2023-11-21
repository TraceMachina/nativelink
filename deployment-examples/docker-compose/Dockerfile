# Copyright 2022-2023 The Native Link Authors. All rights reserved.
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

# Override this if you want to run on a different version of ubuntu.
ARG OS_VERSION=22.04
# `--compilation_mode` to pass into bazel (eg: opt, dbg, fastbuild).
ARG OPT_LEVEL=opt
# Additional bazel flags.
ARG ADDITIONAL_BAZEL_FLAGS=
# Bash arguments my be passed in here to install any additional dependencies
# needed by the user. Useful if your worker needs specific dependencies installed.
ARG ADDITIONAL_SETUP_WORKER_CMD=


FROM ubuntu:${OS_VERSION} AS dependencies
ARG OS_VERSION
RUN apt update && \
    DEBIAN_FRONTEND=noninteractive apt install --no-install-recommends -y \
        npm \
        git \
        gcc \
        g++ \
        python3 \
        ca-certificates && \
    npm install -g @bazel/bazelisk


# Build the binary.
FROM dependencies AS builder
WORKDIR /root/native-link
ADD . .
ARG OPT_LEVEL
ARG ADDITIONAL_BAZEL_FLAGS
RUN bazel build -c ${OPT_LEVEL} ${ADDITIONAL_BAZEL_FLAGS} //cas && \
    cp ./bazel-bin/cas/cas /root/native-link-bin


# Go back to a fresh ubuntu container and copy only the compiled binary.
FROM ubuntu:${OS_VERSION} as final
COPY --from=builder /root/native-link-bin /usr/local/bin/native-link
# Install any specific dependencies needed by the user.
ARG ADDITIONAL_SETUP_WORKER_CMD
RUN bash -ueo pipefail -c "${ADDITIONAL_SETUP_WORKER_CMD}"

RUN mkdir -p /root/.cache/native-link
EXPOSE 50051/tcp 50052/tcp
CMD ["native-link"]
