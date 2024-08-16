#!/usr/bin/env bash

set -xeuo pipefail

ECR=${ECR:?Error: ECR is not set}
ECR_PROFILE=${ECR_PROFILE:?Error: ECR_PROFILE is not set}
ECR_USER=${ECR_USER:?Error: ECR_USER is not set}
ECR_REGION=${ECR_REGION:?Error: ECR_REGION is not set}
BUILDX_NO_CACHE=${BUILDX_NO_CACHE:-true}

SRC_ROOT=$(git rev-parse --show-toplevel)
FLAKE_NIX_FILE="${SRC_ROOT}/flake.nix"
echo "WARNING: This script will modify and revert the flake.nix"
sleep 3

function ecr_login() {
    aws ecr get-login-password --profile ${ECR_PROFILE} --region ${ECR_REGION} | docker login --username ${ECR_USER} --password-stdin ${ECR}
}

# Build a base image for drake actions.
docker buildx build --no-cache=${BUILDX_NO_CACHE} \
    --platform linux/amd64 \
    -t localhost:5001/toolchain-drake:latest \
    --push \
    ${SRC_ROOT}/tools/toolchain-drake

# Parse out the repo digests sha hash to be used as image digest.
FULL_IMAGE_PATH=$(docker inspect localhost:5001/toolchain-drake:latest | jq '.[].RepoDigests[0]')
IMAGE_DIGEST=$(echo $FULL_IMAGE_PATH | awk -F'[@"]' '{print $3}')
if [ -z "$IMAGE_DIGEST" ]; then
    echo "Unable to parse RepoDigests"
    exit 1
fi

# Capture unpatched flake file for test.
ORIGINAL_FLAKE_CONTENT=$(cat "${FLAKE_NIX_FILE}")

# Patch flake.nix with image digest.
sed -i -E "s|imageDigest = \"\"; # DO NOT COMMIT DRAKE IMAGE_DIGEST VALUE|imageDigest = \"${IMAGE_DIGEST}\"; # DO NOT COMMIT DRAKE IMAGE_DIGEST VALUE|" "${FLAKE_NIX_FILE}"

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
    nix run .#nativelink-worker-toolchain-drake.copyTo docker://localhost:5001/nativelink-toolchain-drake:latest -- --dest-tls-verify=false 2>&1 |
    grep "got:" |
    grep -o 'sha256-[^[:space:]]*'
)
set -o pipefail

# Capture unpatched flake file for test.
ORIGINAL_FLAKE_CONTENT=$(cat "${FLAKE_NIX_FILE}")

# Patch flake.nix with sha256 value.
sed -i -E "s|sha256 = \"\"; # DO NOT COMMIT DRAKE SHA256 VALUE|sha256 = \"${SHA256_HASH}\"; # DO NOT COMMIT DRAKE SHA256 VALUE|" "${FLAKE_NIX_FILE}"

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

# Wrap it with nativelink to turn it into a worker.
nix run .#nativelink-worker-toolchain-drake.copyTo \
    docker://localhost:5001/nativelink-toolchain-drake:latest \
    -- \
    --dest-tls-verify=false

# Pull in to local docker and tag.
docker pull localhost:5001/nativelink-toolchain-drake:latest
docker tag localhost:5001/nativelink-toolchain-drake:latest ${ECR}

# Push to ECR.
ecr_login
docker push ${ECR}

# Restore changes.
git restore "${FLAKE_NIX_FILE}"
