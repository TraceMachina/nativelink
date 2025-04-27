#!/bin/bash

# Run NativeLink Enhanced Benchmarks
# This script simplifies running the enhanced benchmarking system

set -e

# Default values
OUTPUT_DIR="./benchmark_results"
VIZ_DIR="./benchmark_viz"
NATIVELINK_URL="https://app.nativelink.com"
RUNS=3
INCREMENTAL=true
NETWORK_STATS=true

# Help message
show_help() {
  echo "Usage: $0 [options] --project=<project_path> --commit=<commit_hash>"
  echo ""
  echo "Required arguments:"
  echo "  --project=<path>       Path to the test project (must be a Bazel workspace)"
  echo "  --commit=<hash>        NativeLink commit hash being benchmarked"
  echo ""
  echo "Optional arguments:"
  echo "  --output-dir=<path>    Directory to store benchmark results (default: $OUTPUT_DIR)"
  echo "  --viz-dir=<path>       Directory to store visualizations (default: $VIZ_DIR)"
  echo "  --nativelink-url=<url> URL for NativeLink service (default: $NATIVELINK_URL)"
  echo "  --api-key=<key>        API key for NativeLink service"
  echo "  --compare-to=<hash>    Previous commit hash to compare against"
  echo "  --runs=<number>        Number of benchmark runs to average (default: $RUNS)"
  echo "  --no-incremental       Disable incremental build tests"
  echo "  --no-network-stats     Disable network statistics collection"
  echo "  --help                 Show this help message"
  exit 1
}

# Parse arguments
PROJECT=""
COMMIT=""
COMPARE_TO=""
API_KEY=""

for arg in "$@"; do
  case $arg in
    --project=*)
      PROJECT="${arg#*=}"
      ;;
    --commit=*)
      COMMIT="${arg#*=}"
      ;;
    --output-dir=*)
      OUTPUT_DIR="${arg#*=}"
      ;;
    --viz-dir=*)
      VIZ_DIR="${arg#*=}"
      ;;
    --nativelink-url=*)
      NATIVELINK_URL="${arg#*=}"
      ;;
    --api-key=*)
      API_KEY="${arg#*=}"
      ;;
    --compare-to=*)
      COMPARE_TO="${arg#*=}"
      ;;
    --runs=*)
      RUNS="${arg#*=}"
      ;;
    --no-incremental)
      INCREMENTAL=false
      ;;
    --no-network-stats)
      NETWORK_STATS=false
      ;;
    --help)
      show_help
      ;;
    *)
      echo "Unknown option: $arg"
      show_help
      ;;
  esac
done

# Check required arguments
if [ -z "$PROJECT" ] || [ -z "$COMMIT" ]; then
  echo "Error: --project and --commit are required arguments"
  show_help
fi

# Check if project directory exists
if [ ! -d "$PROJECT" ]; then
  echo "Error: Project directory '$PROJECT' does not exist"
  exit 1
fi

# Check for Bazel workspace
if [ ! -f "$PROJECT/WORKSPACE" ] && [ ! -f "$PROJECT/WORKSPACE.bazel" ] && [ ! -f "$PROJECT/MODULE.bazel" ]; then
  echo "Error: Project directory '$PROJECT' does not appear to be a Bazel workspace"
  exit 1
fi

# Check for Python dependencies
pip install -r "$(dirname "$0")/requirements.txt"

# Create output directories
mkdir -p "$OUTPUT_DIR"
mkdir -p "$VIZ_DIR"

# Build benchmark command
BENCHMARK_CMD="python3 $(dirname "$0")/enhanced_benchmark.py \
  --project=$PROJECT \
  --commit=$COMMIT \
  --output-dir=$OUTPUT_DIR \
  --nativelink-url=$NATIVELINK_URL \
  --runs=$RUNS"

# Add optional arguments
if [ -n "$API_KEY" ]; then
  BENCHMARK_CMD="$BENCHMARK_CMD \
  --api-key=$API_KEY"
fi

if [ -n "$COMPARE_TO" ]; then
  BENCHMARK_CMD="$BENCHMARK_CMD \
  --compare-to=$COMPARE_TO"
fi

if [ "$INCREMENTAL" = true ]; then
  BENCHMARK_CMD="$BENCHMARK_CMD \
  --incremental"
fi

if [ "$NETWORK_STATS" = true ]; then
  BENCHMARK_CMD="$BENCHMARK_CMD \
  --network-stats"
fi

# Run benchmark
echo "Running enhanced benchmarks..."
echo "$BENCHMARK_CMD"
eval "$BENCHMARK_CMD"

# Generate visualizations
VIZ_CMD="python3 $(dirname "$0")/enhanced_visualize.py \
  --input-dir=$OUTPUT_DIR \
  --output-dir=$VIZ_DIR \
  --html-report=True"

echo "Generating visualizations..."
echo "$VIZ_CMD"
eval "$VIZ_CMD"

echo "Benchmarking complete!"
echo "Results saved to: $OUTPUT_DIR"
echo "Visualizations saved to: $VIZ_DIR"
echo "HTML report: $VIZ_DIR/benchmark_report.html"
