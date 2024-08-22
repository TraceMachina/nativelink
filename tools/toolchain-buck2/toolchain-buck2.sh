#!/usr/bin/env bash
# Copyright 2022-2024 The NativeLink Authors. All rights reserved.
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

# Creates a custom toolchain for building https://github.com/facebook/buck2
# source tree and pushing it to Amazon Elastic Container Registry (ECR).

set -xeuo pipefail

ECR=${ECR:?Error: ECR is not set}
ECR_PROFILE=${ECR_PROFILE:?Error: ECR_PROFILE is not set}
ECR_USER=${ECR_USER:?Error: ECR_USER is not set}
ECR_REGION=${ECR_REGION:?Error: ECR_REGION is not set}
BUILDX_NO_CACHE=${BUILDX_NO_CACHE:-true}
ECR_PUBLISH=${ECR_PUBLISH:-true}

SRC_ROOT=$(git rev-parse --show-toplevel)
FLAKE_NIX_FILE="${SRC_ROOT}/flake.nix"
echo "WARNING: This script will modify and revert the flake.nix"
sleep 3

function ecr_login() {
    aws ecr get-login-password --profile ${ECR_PROFILE} --region ${ECR_REGION} | \
    docker login --username ${ECR_USER} --password-stdin ${ECR}
}

# Build a base image for buck2 actions.
# Base image is published to the local docker engine
# from the Dockerfile.
docker buildx build --no-cache=${BUILDX_NO_CACHE} \
    --platform linux/amd64 \
    -t localhost:5001/toolchain-buck2:latest \
    --push \
    ${SRC_ROOT}/tools/toolchain-buck2

# Parse out the repo digests sha hash to be used as image digest.
FULL_IMAGE_PATH=$(docker inspect localhost:5001/toolchain-buck2:latest | jq '.[].RepoDigests[0]')
IMAGE_DIGEST=$(echo $FULL_IMAGE_PATH | awk -F'[@"]' '{print $3}')
if [ -z "$IMAGE_DIGEST" ]; then
    echo "Unable to parse RepoDigests"
    exit 1
fi

# Capture unpatched flake file for test.
ORIGINAL_FLAKE_CONTENT=$(cat "${FLAKE_NIX_FILE}")

# Patch flake.nix with image digest.
sed -i -E "s|imageDigest = \"\"; # DO NOT COMMIT BUCK2 IMAGE_DIGEST VALUE|imageDigest = \"${IMAGE_DIGEST}\"; # DO NOT COMMIT BUCK2 IMAGE_DIGEST VALUE|" "${FLAKE_NIX_FILE}"

# Bail if flake wasn't updated
PATCHED_FLAKE_CONTENT=$(cat "${FLAKE_NIX_FILE}")
if [ "$ORIGINAL_FLAKE_CONTENT" == "$PATCHED_FLAKE_CONTENT" ]; then
    echo "No changes were made to ${FLAKE_NIX_FILE}"
    exit 1
else
    echo "Changes made"
    pushd $SRC_ROOT
    git --no-pager diff "${FLAKE_NIX_FILE}"
    sleep 3
    popd
fi

# Get the sha256 value, this will fail due to empty string in the sha256 field.
set +o pipefail
SHA256_HASH=$(
    nix run .#nativelink-worker-toolchain-buck2.copyTo \
    docker://localhost:5001/nativelink-toolchain-buck2:latest \
    -- --dest-tls-verify=false 2>&1 | \
    grep "got:" | \
    grep -o 'sha256-[^[:space:]]*'
)
set -o pipefail

# Capture unpatched flake file for test.
ORIGINAL_FLAKE_CONTENT=$(cat "${FLAKE_NIX_FILE}")

# Patch flake.nix with sha256 value.
sed -i -E "s|sha256 = \"\"; # DO NOT COMMIT BUCK2 SHA256 VALUE|sha256 = \"${SHA256_HASH}\"; # DO NOT COMMIT BUCK2 SHA256 VALUE|" "${FLAKE_NIX_FILE}"

# Bail if flake wasn't updated.
PATCHED_FLAKE_CONTENT=$(cat "${FLAKE_NIX_FILE}")
if [ "$ORIGINAL_FLAKE_CONTENT" == "$PATCHED_FLAKE_CONTENT" ]; then
    echo "No changes were made to ${FLAKE_NIX_FILE}"
    exit 1
else
    echo "Changes made"
    pushd $SRC_ROOT
    git --no-pager diff "${FLAKE_NIX_FILE}"
    sleep 3
    popd
fi

# Add worker specific files and configurations.
nix run .#nativelink-worker-toolchain-buck2.copyTo \
    docker://localhost:5001/nativelink-toolchain-buck2:latest \
    -- \
    --dest-tls-verify=false

# Publish image to ECR.
if [ "$ECR_PUBLISH" = "true" ]; then
    ecr_login
    nix run .#nativelink-worker-toolchain-buck2.copyTo ${ECR}
else
    echo "Skipping ECR publishing"
fi

# Restore changes.
git restore "${FLAKE_NIX_FILE}"
