#!/bin/bash
# Copyright 2026 The NativeLink Authors. All rights reserved.
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

# Sanity check for REAPI content-defined chunking: uploads a large blob via
# SpliceBlob (Bazel --experimental_remote_cache_chunking), verifies the
# server registered a chunk layout, then re-fetches the blob from the remote
# cache via the chunked download path and checks it is byte-identical.

if [[ $UNDER_TEST_RUNNER -ne 1 ]]; then
    echo "This script should be run under run_integration_tests.sh"
    exit 1
fi
set -x

CHUNKING_FLAGS=(--config self_test --experimental_remote_cache_chunking)
EXPECTED_SHA=$(seq 1 1000000 | sha256sum | awk '{print $1}')
# The test runner's working directory is not the workspace root, so resolve
# the output location through bazel itself.
ARTIFACT="$(bazel --output_base="$BAZEL_CACHE_DIR" info "${CHUNKING_FLAGS[@]}" bazel-bin)/chunking_test_artifact.txt"

# First build executes the action locally and uploads the ~6.9MB output as
# chunks (SpliceBlob).
bazel --output_base="$BAZEL_CACHE_DIR" build "${CHUNKING_FLAGS[@]}" //:chunking_test_artifact
FIRST_SHA=$(sha256sum "$ARTIFACT" | awk '{print $1}')
if [[ $FIRST_SHA != "$EXPECTED_SHA" ]]; then
    echo "Expected locally built artifact to have sha $EXPECTED_SHA, got $FIRST_SHA."
    exit 1
fi

# The server must have registered a chunk layout for the spliced blob. The
# index store is mounted from the host by docker-compose.
CHUNK_INDEX_DIR="${NATIVELINK_DIR:-$HOME/.cache/nativelink}/content_path-chunk-index"
if [[ -z $(find "$CHUNK_INDEX_DIR" -type f 2> /dev/null) ]]; then
    echo "Expected a chunk layout in $CHUNK_INDEX_DIR after a chunked upload."
    echo "SpliceBlob was likely not used; check that the server advertises"
    echo "chunking support and that bazel supports the chunking flag."
    exit 1
fi

# Clean our local cache and re-fetch from the remote cache through the
# chunked download path.
bazel --output_base="$BAZEL_CACHE_DIR" clean
OUTPUT=$(bazel --output_base="$BAZEL_CACHE_DIR" build "${CHUNKING_FLAGS[@]}" //:chunking_test_artifact 2>&1)
if [[ ! $OUTPUT =~ 'remote cache hit' ]]; then
    echo "Expected second bazel run to be a remote cache hit."
    echo "STDOUT:"
    echo "$OUTPUT"
    exit 1
fi
SECOND_SHA=$(sha256sum "$ARTIFACT" | awk '{print $1}')
if [[ $SECOND_SHA != "$EXPECTED_SHA" ]]; then
    echo "Artifact fetched through the chunked download path is corrupt:"
    echo "expected sha $EXPECTED_SHA, got $SECOND_SHA."
    exit 1
fi
