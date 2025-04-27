# NativeLink Benchmarking System

This directory contains tools for benchmarking NativeLink performance on a per-commit basis, inspired by the [Lucene Nightly Benchmarks](https://blog.mikemccandless.com/2011/04/catching-slowdowns-in-lucene.html) project.

## Overview

The benchmarking system measures and tracks NativeLink's performance across different commits, allowing developers to identify performance regressions or improvements. The system focuses on two primary use cases:

1. Build with remote cache only
2. Build with remote cache and execution

## How It Works

The benchmarking system works by:

1. Running builds of test projects (Rust and C++ codebases) using Bazel with NativeLink
2. Measuring key performance metrics like build time and resource usage
3. Storing results in a structured format for historical comparison
4. Comparing results between commits to detect performance changes
5. Reporting significant regressions or improvements

## Usage

### Manual Execution

You can run the benchmarking tool manually with:

```bash
python3 tools/benchmark/benchmark.py \
  --project=/path/to/test/project \
  --commit=<commit-hash> \
  --nativelink-url=https://app.nativelink.com \
  --api-key=<your-api-key> \
  --output-dir=./benchmark_results \
  --compare-to=<previous-commit-hash> \
  --runs=3
```

### Arguments

- `--project`: Path to the test project (must be a Bazel workspace)
- `--commit`: NativeLink commit hash being benchmarked
- `--output-dir`: Directory to store benchmark results (default: ./benchmark_results)
- `--nativelink-url`: URL for NativeLink service (default: https://app.nativelink.com)
- `--api-key`: API key for NativeLink service
- `--compare-to`: Previous commit hash to compare against (optional)
- `--runs`: Number of benchmark runs to average (default: 3)

## Automated Benchmarking

The benchmarking system is integrated into the CI pipeline via a GitHub Actions workflow (`.github/workflows/benchmark.yaml`). This workflow automatically:

1. Runs on each push to main and on pull requests
2. Checks out test projects (currently rustlings and googletest)
3. Runs benchmarks against the current commit
4. Compares results with the previous commit
5. Uploads results as artifacts
6. Posts a summary comment on pull requests

## Interpreting Results

Benchmark results are stored in both JSON and CSV formats. The JSON format includes:

```json
{
  "commit": "commit-hash",
  "timestamp": "YYYYMMDD_HHMMSS",
  "project": "project-name",
  "metrics": {
    "remote_cache_only": {
      "build_time": 10.5,
      "peak_memory": 1024.0
    },
    "remote_cache_and_execution": {
      "build_time": 5.2,
      "peak_memory": 512.0
    }
  }
}
```

When comparing results, the system calculates absolute and percentage differences. A warning is generated for significant regressions (e.g., >5% slowdown in build time).

## Adding New Test Projects

To add a new test project:

1. Ensure the project uses Bazel as its build system
2. Update the GitHub workflow to check out and benchmark the new project
3. Consider the project's build complexity and stability when interpreting results

## Future Improvements

- Add more sophisticated metrics collection (CPU usage, network I/O)
- Implement a web dashboard for visualizing performance trends
- Add support for more complex build scenarios
- Integrate with notification systems for significant regressions
