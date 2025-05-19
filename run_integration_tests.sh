#!/bin/bash
# Copyright 2024 The NativeLink Authors. All rights reserved.
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

if which bazel > /dev/null; then
    echo "Bazel is installed."
else
    echo "Bazel is not installed."
    echo "Bazel needs to be installed to run this integration test script and the instructions are currently here: https://bazel.build/install/"
    exit 1
fi

TEST_PATTERNS=()

while [[ $# -gt 0 ]]; do
    case $1 in
    --help)
        cat << 'EOT'
Runner for integration tests

Usage:
  run_integration_tests.sh [TEST_PATTERNS...]

TEST_PATTERNS: Name of test you wish to execute. Wildcard (*) supported.
               Default: '*'
EOT
        ;;
    -*)
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

if [[ ${#TEST_PATTERNS[@]} -eq 0 ]]; then
    TEST_PATTERNS=("*")
fi

SELF_DIR=$(realpath "$(dirname "$0")")
cd "$SELF_DIR/deployment-examples/docker-compose"

export UNDER_TEST_RUNNER=1

# Ensure our cache locations are empty.
sudo rm -rf ~/.cache/nativelink
mkdir -p ~/.cache/nativelink

# Ensure our docker compose is not running.
sudo docker compose rm --stop -f

export TMPDIR=$HOME/.cache/nativelink/
mkdir -p "$TMPDIR"

if [[ $OSTYPE == "darwin"* ]]; then
    CACHE_DIR=$(mktemp -d "${TMPDIR}nativelink-integration-test")
    export CACHE_DIR
else
    echo "Assumes Linux/WSL"
    CACHE_DIR=$(mktemp -d --tmpdir="$TMPDIR" --suffix="-nativelink-integration-test")
    export CACHE_DIR
fi

export BAZEL_CACHE_DIR="$CACHE_DIR/bazel"
trap 'sudo rm -rf $CACHE_DIR; sudo docker compose rm --stop -f' EXIT

echo "" # New line.

export NATIVELINK_DIR="$CACHE_DIR/nativelink"
mkdir -p "$NATIVELINK_DIR"

for pattern in "${TEST_PATTERNS[@]}"; do
    find "$SELF_DIR/integration_tests/" -name "$pattern" -type f -print0 | grep -v buildstream | while IFS= read -r -d $'\0' fullpath; do
        # Cleanup.
        echo "Cleaning up cache directories NATIVELINK_DIR: $NATIVELINK_DIR"
        echo "Checking for existence of the NATIVELINK_DIR"
        if [ -d "$NATIVELINK_DIR" ]; then
            sudo find "$NATIVELINK_DIR" -delete
        else
            echo "Directory $NATIVELINK_DIR does not exist."
        fi

        bazel --output_base="$BAZEL_CACHE_DIR" clean
        FILENAME=$(basename "$fullpath")
        echo "Running test $FILENAME"
        sudo docker compose up -d
        if perl -e 'alarm shift; exec @ARGV' 30 bash -c 'until sudo docker compose logs | grep -q "Ready, listening on"; do sleep 1; done'; then
            echo "String 'Ready, listening on' found in the logs."
        else
            echo "String 'Ready, listening on' not found in the logs within the given time."
        fi
        set +e
        bash -euo pipefail "$fullpath"
        EXIT_CODE="$?"
        set -e
        if [[ $EXIT_CODE -eq 0 ]]; then
            echo "$FILENAME passed"
        else
            echo "$FILENAME failed with exit code $EXIT_CODE"
            sudo docker compose logs
            exit $EXIT_CODE
        fi
        sudo docker compose rm --stop -f
        echo "" # New line.
    done
done

echo "All tests passed!"
