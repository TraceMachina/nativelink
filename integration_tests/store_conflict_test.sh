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

# This is a test to check the case when CAS and Ac use the same store.

if [[ $UNDER_TEST_RUNNER -ne 1 ]]; then
  echo "This script should be run under run_integration_tests.sh"
  exit 1
fi

set -x

EXIT_CODE=0
sudo docker compose --file docker-compose.test.yml up -d
if perl -e 'alarm shift; exec @ARGV' 30 bash -c 'until sudo docker compose --file docker-compose.test.yml logs | grep -q "CAS and AC use the same store"; do sleep 1; done'
then  
  echo "String 'CAS and AC use the same store' found in the logs."
else
  echo "String 'CAS and AC use the same store' not found in the logs within the given time."
  $EXIT_CODE=1
fi
sudo docker compose --file docker-compose.test.yml rm --stop -f
echo "" # New line.

exit $EXIT_CODE
