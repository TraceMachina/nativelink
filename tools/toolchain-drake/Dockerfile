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

FROM ubuntu:noble-20250925@sha256:728785b59223d755e3e5c5af178fab1be7031f3522c5ccd7a0b32b80d8248123 AS dependencies
RUN apt-get update && DEBIAN_FRONTEND=noninteractive \
    apt-get install --no-install-recommends -y \
        git=1:2.43.0-1ubuntu7.3 \
        ca-certificates=20240203 \
        && apt-get clean \
        && rm -rf /var/lib/apt/lists/* \
        && update-ca-certificates

FROM dependencies AS final

# hadolint ignore=DL3003
RUN git clone https://github.com/RobotLocomotion/drake.git && \
    cd drake && \
    DEBIAN_FRONTEND=noninteractive ./setup/ubuntu/install_prereqs.sh --with-bazel -y
