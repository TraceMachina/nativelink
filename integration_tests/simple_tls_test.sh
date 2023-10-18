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
if [[ $UNDER_TEST_RUNNER -ne 1 ]]; then
  echo "This script should be run under run_integration_tests.sh" 
  exit 1
fi

cd ../../

cd ./deployment-examples/terraform/scripts

chmod +x start_turbo_cache.sh
# Starting LB "turbo-cache-cas-lb" so it can be connected to.
./start_turbo_cache.sh

cd ../

terraform init
terraform apply

# TODO?: Make branches for AWS or GCP or Azure.
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
