#!/usr/bin/env bash

set -xeuo pipefail

ECR=${ECR:?Error: ECR is not set}
ECR_PROFILE=${ECR_PROFILE:?Error: ECR_PROFILE is not set}
ECR_USER=${ECR_USER:?Error: ECR_USER is not set}
ECR_REGION=${ECR_REGION:?Error: ECR_REGION is not set}
BUILDX_NO_CACHE=${BUILDX_NO_CACHE:-true}

function ecr_login() {
    aws ecr get-login-password --profile ${ECR_PROFILE} --region ${ECR_REGION} | docker login --username ${ECR_USER} --password-stdin ${ECR}.dkr.ecr.${ECR_REGION}.amazonaws.com
}

# Check OS and calculate the SHA256 hash of the Dockerfile
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    IMAGE_TAG=$(sha256sum 'Dockerfile' | awk '{print $1}')
elif [[ "$OSTYPE" == "darwin"* ]]; then
    IMAGE_TAG=$(shasum -a 256 'Dockerfile' | awk '{print $1}')
else
    echo "Unsupported OS"
    exit 1
fi

# Build the Docker image and tag it with the hash
docker buildx build --no-cache=${BUILDX_NO_CACHE} --platform linux/amd64 -t "${ECR}.dkr.ecr.${ECR_REGION}.amazonaws.com/nativelink-rbe:$IMAGE_TAG" -f 'Dockerfile' .

ecr_login
docker push ${ECR}.dkr.ecr.${ECR_REGION}.amazonaws.com/nativelink-rbe:$IMAGE_TAG

# Output the tag of the built image
echo "Docker image tagged as ${ECR}.dkr.ecr.${ECR_REGION}.amazonaws.com/nativelink-rbe:$IMAGE_TAG"
