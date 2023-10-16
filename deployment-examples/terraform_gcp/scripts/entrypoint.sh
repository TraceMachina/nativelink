#!/bin/bash

set -eu

BASE_GCR="gcr.io/"

function download_container_if_needed() {
  local docker_image="$1"
  local local_image_tag=$(docker images "$docker_image" --format=json | jq -r ".Tag")
  if [ "$local_image_tag" != "" ]; then
    # Nothing to do, it's already local.
    return
  fi
  gcloud auth print-access-token | docker login -u oauth2accesstoken --password-stdin https://gcr.io 2>&1 >/dev/null
  docker pull "$docker_image" -q
  sync
}

if [ "$TURBO_CACHE_DOCKER_IMAGE" == "" ]; then
  echo "TURBO_CACHE_DOCKER_IMAGE env var not set."
  exit 1
fi

if [ "$TURBO_CACHE_MEMORY_MB" == "" ]; then
  echo "TURBO_CACHE_MEMORY_MB env var not set."
  exit 1
fi

download_container_if_needed "${BASE_GCR}${TURBO_CACHE_DOCKER_IMAGE}" > /dev/null

RAND_ID="$(uuidgen)"
ENTRY_SCRIPT="entry_$RAND_ID.sh"
EXIT_CODE_FILE="exit_code_$RAND_ID.sh"
cat <<EOF > "$ENTRY_SCRIPT"
#!/bin/bash
set -euo pipefail

rm -rf "$ENTRY_SCRIPT"

$(export)
cd /work

"\$@"
echo "\$?" > "$EXIT_CODE_FILE"
EOF
chmod +x "$ENTRY_SCRIPT"

set +eu

docker run \
  --name "$RAND_ID" \
  --mount "type=bind,source=$(realpath .),target=/work" \
  --workdir "/work" \
  --memory "${TURBO_CACHE_MEMORY_MB}m" \
  "${BASE_GCR}${TURBO_CACHE_DOCKER_IMAGE}" \
  "/work/$ENTRY_SCRIPT" "$@"
DOCKER_EXIT_CODE="$?"

set -eu

# Remove the docker container when done.
trap "docker rm \"$RAND_ID\" > /dev/null" EXIT

if [ "$DOCKER_EXIT_CODE" != "0" ]; then
  OOM_KILLED=$(docker container inspect "$RAND_ID" -f '{{json .State.OOMKilled}}')
  if [ "$OOM_KILLED" == "true" ]; then
    echo 'Job exceeded the memory allowance. Try adding: `exec_properties = { "memory_mb": "3600" }` (or higher) to the bazel rule.'
    exit 1
  fi
fi

if [ ! -f "$EXIT_CODE_FILE" ]; then
  echo "Docker did not exit properly."
  exit 1
fi

exit "$(cat $EXIT_CODE_FILE)"
