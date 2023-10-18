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

LOAD_BALANCER_ADDRESS="$(aws elbv2 describe-load-balancers --names "turbo-cache-cas-lb" --query 'LoadBalancers[0].DNSName' --output text):443"

# Test a successful TLS connection to the load balancer.
# Retry a few times as service may take a few seconds to get started.
curl --retry 5 --retry-delay 0 --retry-max-time 30 --insecure "$LOAD_BALANCER_ADDRESS"

# Check if the connection was successful.
if [ $? -eq 0 ]; then
  echo "Successfully connected to the load balancer via TLS."
else
  echo "Failed to connect to the load balancer via TLS."
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
