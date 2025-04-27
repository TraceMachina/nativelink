# NativeLink Benchmarking System Guide

## Overview

The NativeLink benchmarking system measures and tracks performance across different commits, allowing developers to identify performance regressions or improvements. The system focuses on several key use cases:

1. **Build with remote cache only** - Tests how NativeLink performs when used solely as a remote cache
2. **Build with remote cache and execution** - Tests combined performance of remote caching and execution
3. **Clean build with remote execution** - Tests pure remote execution performance without caching
4. **Incremental build** - Tests performance when making small changes to a previously built project

## How It Works

The benchmarking system:

1. Runs builds of test projects (small, medium, and large codebases) using Bazel with NativeLink
2. Measures comprehensive performance metrics including:
   - Build time
   - Resource usage (CPU, memory)
   - Network I/O
   - Cache hit/miss rates
3. Stores results in a structured format for historical comparison
4. Compares results between commits to detect performance changes
5. Generates visualizations and HTML reports to track performance trends
6. Reports significant regressions or improvements

## Running Benchmarks Manually

You can run the benchmarking tool manually with:

```bash
python3 tools/benchmark/enhanced_benchmark.py \
  --project=/path/to/test/project \
  --commit=<commit-hash> \
  --nativelink-url=https://app.nativelink.com \
  --api-key=<your-api-key> \
  --output-dir=./benchmark_results \
  --compare-to=<previous-commit-hash> \
  --runs=3 \
  --incremental=True \
  --network-stats=True
