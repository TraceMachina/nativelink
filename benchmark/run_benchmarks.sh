#!/bin/bash

set -e

# Directory setup
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RESULTS_DIR="${REPO_ROOT}/benchmark_results"
REPORTS_DIR="${REPO_ROOT}/benchmark_reports"

# Create directories
mkdir -p "${RESULTS_DIR}"
mkdir -p "${REPORTS_DIR}"

# Install dependencies if needed
pip3 install matplotlib pandas

# Get the latest commits (limit to last 5 for testing)
cd "${REPO_ROOT}"
COMMITS=$(git log --pretty=format:"%H" -n 5)

# Run benchmarks for each commit
for COMMIT in ${COMMITS}; do
    echo "Running benchmark for commit ${COMMIT}"
    python3 "${SCRIPT_DIR}/benchmark_runner.py" \
        --repo-path "${REPO_ROOT}" \
        --output-dir "${RESULTS_DIR}" \
        --commit "${COMMIT}" \
        --project "rustlings" \
        --nativelink-url "https://app.nativelink.com"
done

# Generate report
python3 "${SCRIPT_DIR}/report_generator.py" \
    --results-dir "${RESULTS_DIR}" \
    --output-dir "${REPORTS_DIR}"

echo "Benchmarking complete. Reports available in ${REPORTS_DIR}"