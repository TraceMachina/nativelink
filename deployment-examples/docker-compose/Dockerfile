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


# Builder that contains the OS dependencies.
FROM ubuntu:22.04 AS builder-deps
RUN apt update && \
    DEBIAN_FRONTEND=noninteractive apt install --no-install-recommends -y \
        npm \
        git \
        pkg-config \
        libssl-dev \
        clang \
        python3 && \
    npm install -g @bazel/bazelisk
ARG CC=clang


# Builder that only contains the downloaded externals.
FROM builder-deps as builder-with-externals
WORKDIR /root/turbo-cache
# Only xfer the minimum needed to query external packages..
ADD WORKSPACE .
ADD .bazel* .
ADD Cargo.* .
ADD .rustfmt.toml .
ADD BUILD .
# Download external dependencies. This is to improve docker layer caching.
RUN bazel fetch //...


# Builder that only contains the files related to bazel, but none of the source code.
# This layer improves the cache hits of the following layer, since it'll bypass the next
# layer if only the code files changed.
FROM builder-with-externals as builder-only-bazel-and-build-files
WORKDIR /root/turbo-cache
RUN find . -type f -regextype egrep ! -regex '.*/(BUILD|WORKSPACE|\.bazel.*|Cargo\..*|\.rustfmt\.toml)' -delete && \
    find . -type d -empty -delete


# Builder to get a list of externals and targets rarely changed that take a while to build
# and push them into a file. This action takes a few seconds, but most of the time the
# output file will not change between builds, which dramatically improves our layer cache
# hits.
FROM builder-only-bazel-and-build-files AS builder-external-deps
WORKDIR /root/turbo-cache
RUN bazel cquery -c opt \
        'filter("^@", deps(//..., 1))' \
        --universe_scope=//... \
        --implicit_deps=false \
        --tool_deps=false \
    | cut -d' ' -f1 \
    | sort \
    | uniq > /root/external-deps-targets.txt
# Special case here, since `@com_google_protobuf//:protoc` takes a bit of time to build
# and must be built on host machine, we generate our protos file, since they rarely
# change and will allow us to save a lot of time if the target is cached.
RUN echo "//proto:dummy_rule_used_to_improve_docker_layer_caching" >> /root/external-deps-targets.txt


# Builder for our externals and targets rarely changed that take a while to build.
FROM builder-with-externals as builder-externals-built
WORKDIR /root/turbo-cache
ADD proto/BUILD proto/BUILD
COPY --from=builder-external-deps /root/external-deps-targets.txt /root/external-deps-targets.txt
RUN bazel build -c opt --target_pattern_file=/root/external-deps-targets.txt


# Builder to do the remaining build. At this point most of our externals should be built.
FROM builder-externals-built as builder
ADD . .
RUN bazel build -c opt //cas && \
    cp ./bazel-bin/cas/cas /root/turbo-cache-bin


# Go back to a fresh ubuntu container and copy only the compiled binary.
FROM ubuntu:22.04
COPY --from=builder /root/turbo-cache-bin /usr/local/bin/turbo-cache
# Install runtime packages.
RUN apt update && \
    DEBIAN_FRONTEND=noninteractive apt install --no-install-recommends -y \
        libssl-dev
RUN mkdir -p /root/.cache/turbo-cache
EXPOSE 50051/tcp 50052/tcp
CMD ["turbo-cache"]
