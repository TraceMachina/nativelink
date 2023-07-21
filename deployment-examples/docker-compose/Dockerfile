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

# Override this if you want to run on a different version of ubuntu.
ARG OS_VERSION=22.04
# `--compilation_mode` to pass into bazel (eg: opt, dbg, fastbuild).
ARG OPT_LEVEL=opt
# Compiler to use.
ARG CC=clang
# Additional bazel flags.
ARG ADDITIONAL_BAZEL_FLAGS=


# Builder that contains the OS dependencies.
FROM ubuntu:${OS_VERSION} AS builder-deps
RUN apt update && apt-get -y install curl gnupg && \
    curl -sS https://dl.yarnpkg.com/debian/pubkey.gpg | apt-key add - && \
    echo "deb https://dl.yarnpkg.com/debian/ stable main" | tee /etc/apt/sources.list.d/yarn.list && \
    apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
        git \
        pkg-config \
        libssl-dev \
        clang \
        python3 \
        yarn \
        nodejs && \
    yarn global add @bazel/bazelisk@1.12.1 && \
    apt-get remove yarn -y && \
    apt-get autoremove -y && \
    apt-get clean -y
SHELL [ "/bin/bash", "-euo", "pipefail", "-c" ]


# Builder that only contains the downloaded externals.
FROM builder-deps as builder-with-externals
WORKDIR /root/turbo-cache
ARG CC
# Only xfer the minimum needed to query external packages..
COPY [ "WORKSPACE", ".bazel*", "Cargo.*", ".rustfmt.toml", "BUILD", "./" ]
# Download external dependencies. This is to improve docker layer caching.
RUN bazel fetch //...


# Builder that only contains the files related to bazel, but none of the source code.
# This layer improves the cache hits of the following layer, since it'll bypass the next
# layer if only the code files changed.
FROM builder-with-externals as builder-only-bazel-and-build-files
WORKDIR /root/turbo-cache
COPY . .
RUN find . -type f -regextype egrep ! -regex '.*/(BUILD|WORKSPACE|\.bazel.*|Cargo\..*|\.rustfmt\.toml)' -delete && \
    find . -type d -empty -delete


# Builder to get a list of externals and targets rarely changed that take a while to build
# and push them into a file. This action takes a few seconds, but most of the time the
# output file will not change between builds, which dramatically improves our layer cache
# hits.
FROM builder-with-externals AS builder-external-deps
WORKDIR /root/turbo-cache
ARG OPT_LEVEL
ARG CC
# Copy only the BUILD files and other bazel related files.
COPY --from=builder-only-bazel-and-build-files /root/turbo-cache/ .
# Special case for `//proto/... because we have a bit of generated code here that runs under
# a different platform (host platform). Since this folder rarely changes we build it as well
# at this cache layer.
RUN bazel cquery -c ${OPT_LEVEL} \
            'filter("^@", deps(attr(testonly, 0, //...), 1))' \
            --universe_scope=//... \
            --implicit_deps=false \
            --tool_deps=false \
        | cut -d' ' -f1 \
        | sort \
        | uniq > /root/external-targets.txt && \
    echo "//proto/..." >> /root/external-targets.txt


# Builder for our externals and targets rarely changed that take a while to build.
FROM builder-with-externals as builder-externals-built
WORKDIR /root/turbo-cache
ARG OPT_LEVEL
ARG CC
ARG ADDITIONAL_BAZEL_FLAGS
COPY proto/ proto/
COPY --from=builder-external-deps /root/external-targets.txt /root/external-targets.txt
RUN bazel build -c ${OPT_LEVEL} ${ADDITIONAL_BAZEL_FLAGS} --target_pattern_file=/root/external-targets.txt


# Builder to do the remaining build. At this point most of our externals should be built.
FROM builder-externals-built as builder-final
ARG OPT_LEVEL
ARG CC
ARG ADDITIONAL_BAZEL_FLAGS
COPY . .
RUN bazel build -c ${OPT_LEVEL} ${ADDITIONAL_BAZEL_FLAGS} //cas && \
    cp ./bazel-bin/cas/cas /root/turbo-cache-bin


# Go back to a fresh ubuntu container and copy only the compiled binary.
FROM ubuntu:${OS_VERSION}
COPY --from=builder-final /root/turbo-cache-bin /usr/local/bin/turbo-cache
# Install runtime packages.
RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
        libssl-dev
RUN mkdir -p /root/.cache/turbo-cache
EXPOSE 50051/tcp 50052/tcp
CMD ["turbo-cache"]
