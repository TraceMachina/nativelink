#!/bin/bash
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

# This is an integration test to ensure TLS connections work properly.

# Extract TLS configuration using jq
TLS_CERT_FILE=$(jq -r '.servers[0].tls.cert_file' turbo_cache/config/examples/basic_cas.json)
TLS_KEY_FILE=$(jq -r '.servers[0].tls.key_file' turbo_cache/config/examples/basic_cas.json)

# Define server and addresses (replace with your actual values)
LISTEN_ADDRESS=$(jq -r '.servers[0].listen_address' turbo_cache/config/examples/basic_cas.json)
INVALID_ADDRESS="invalid_remote_server_address:4433"

if [[ $UNDER_TEST_RUNNER -ne 1 ]]; then
  echo "This script should be run under run_integration_tests.sh"
  exit 1
fi

set -x

# Run bazel
bazel --output_base="$BAZEL_CACHE_DIR" test --config self_test //:dummy_test

# Test a successful TLS connection to the success server
# retry a few times as service may take a few seconds to get started
curl --retry 5 --retry-delay 0 --retry-max-time 30 --cert "$TLS_CERT_FILE" --key "$TLS_KEY_FILE" --insecure "$LISTEN_ADDRESS"

# Check if the connection was successful
if [ $? -eq 0 ]; then
  echo "Successfully connected to the success server via TLS."
else
  echo "Failed to connect to the success server via TLS."
  exit 1
fi

# Now, test an unsuccessful TLS connection to an invalid address
curl --cert "$TLS_CERT_FILE" --key "$TLS_KEY_FILE" --insecure "$INVALID_ADDRESS"

# Check if the connection was unsuccessful
if [ $? -ne 0 ]; then
  echo "Failed to connect to the invalid server via TLS, which is expected."
else
  echo "Unexpectedly connected to the invalid server via TLS."
  exit 1
fi
