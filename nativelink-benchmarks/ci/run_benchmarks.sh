#!/bin/bash

# Copyright 2023 The NativeLink Authors. All rights reserved.
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

# This script runs the NativeLink benchmarks in CI for each commit

set -euo pipefail

# Configuration
OUTPUT_DIR="./benchmark_results"
NATIVELINK_URL="https://app.nativelink.com"
TEST_PROJECTS_DIR="./test_projects"
RUNS=3

# Get the current commit hash
CURRENT_COMMIT=$(git rev-parse HEAD)

# Get the previous commit hash
PREVIOUS_COMMIT=$(git rev-parse HEAD~1)

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Install dependencies
pip install -r requirements.txt

# Run benchmarks for each test project
for project in "$TEST_PROJECTS_DIR"/*; do
  if [ -d "$project" ]; then
    project_name=$(basename "$project")
    echo "Running benchmarks for $project_name..."
    
    python benchmark_runner.py \
      --project="$project" \
      --commit="$CURRENT_COMMIT" \
      --output-dir="$OUTPUT_DIR" \
      --nativelink-url="$NATIVELINK_URL" \
      --compare-to="$PREVIOUS_COMMIT" \
      --runs="$RUNS"
  fi
done

# Generate visualization
echo "Generating visualization..."
python generate_visualization.py --data-dir="$OUTPUT_DIR" --output-dir="./visualization/data"

# If running in GitHub Actions, deploy the visualization
if [ -n "${GITHUB_ACTIONS:-}" ]; then
  echo "Deploying visualization to GitHub Pages..."
  # Add deployment steps here
fi

echo "Benchmarks completed successfully!"