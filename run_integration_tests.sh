#!/bin/bash
# Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

set -euo pipefail

if [[ $EUID -eq 0 ]]; then
  echo "This script should not be run as root."
  exit 1
fi

TEST_PATTERNS=()

while [[ $# -gt 0 ]]; do
  case $1 in
    --help)
      echo <<'EOT'
Runner for integration tests

Usage:
  run_integration_tests.sh [TEST_PATTERNS...]

TEST_PATTERNS: Name of test you wish to execute. Wildcard (*) supported.
               Default: '*'
EOT
      ;;
    -*|--*)
      echo "Unknown option $1"
      exit 1
      ;;
    *)
      TEST_PATTERNS+=("$1")
      shift # past argument
      ;;
  esac
done

if ! docker --version; then
  echo "This script must be run as root due to docker permission issues (try with 'sudo')"
  exit 1
fi

if [[ "${#TEST_PATTERNS[@]}" -eq 0 ]]; then
  TEST_PATTERNS=("*")
fi

SELF_DIR=$(realpath $(dirname $0))
cd "$SELF_DIR/deployment-examples/docker-compose"

export UNDER_TEST_RUNNER=1

# Ensure our cache locations are empty.
sudo rm -rf ~/.cache/turbo-cache
mkdir -p ~/.cache/turbo-cache

# Ensure our docker compose is not running.
sudo docker-compose rm --stop -f

echo "What operating system OSTYPE: $OSTYPE"

export TMPDIR=$HOME/.cache/turbo-cache/
mkdir -p "$TMPDIR"

if [[ "$OSTYPE" == "linux-gnu"* ]]; then
  export CACHE_DIR=$(mktemp -d --tmpdir="$TMPDIR" --suffix="-turbo-cache-integration-test")
elif [[ "$OSTYPE" == "darwin"* ]]; then
  # Create a temporary directory using mktemp with a random template, then add a suffix.
  export CACHE_DIR=$(mktemp -d "${TMPDIR}turbo-cache-integration-test")
else
  # Unknown OS
  echo "Unable to detect operating system. Assuming the Linux/WSL syntax will work."
  export CACHE_DIR=$(mktemp -d --tmpdir="$TMPDIR" --suffix="-turbo-cache-integration-test")
fi

export BAZEL_CACHE_DIR="$CACHE_DIR/bazel"
trap "sudo rm -rf $CACHE_DIR; sudo docker-compose rm --stop -f" EXIT

echo "" # New line.

DID_FAIL=0

export TURBO_CACHE_DIR="$CACHE_DIR/turbo-cache"
mkdir -p "$TURBO_CACHE_DIR"

for pattern in "${TEST_PATTERNS[@]}"; do
  find "$SELF_DIR/integration_tests/" -name "$pattern" -type f -print0 | while IFS= read -r -d $'\0' fullpath; do
    # Cleanup.
    echo "Cleaning up cache directories TURBOC_CACHE_DIR: $TURBO_CACHE_DIR"
    echo "Checking for existince of the TURBO_CACHE_DIR"
    if [ -d "$TURBO_CACHE_DIR" ]; then
      sudo find "$TURBO_CACHE_DIR" -delete # add for linux
    else
      echo "Directory $TURBO_CACHE_DIR does not exist."
    fi

    bazel --output_base="$BAZEL_CACHE_DIR" clean
    FILENAME=$(basename $fullpath)
    echo "Running test $FILENAME"
    sudo docker-compose up -d
    set +e
    bash -euo pipefail "$fullpath"
    EXIT_CODE="$?"
    set -e
    if [[ $EXIT_CODE -eq 0 ]]; then
      echo "$FILENAME passed"
    else
      echo "$FILENAME failed with exit code $EXIT_CODE"
      sudo docker-compose logs
      exit $EXIT_CODE
    fi
    sudo docker-compose rm --stop -f
    echo "" # New line.
  done
done

echo "All tests passed!"
