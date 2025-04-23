# Enhanced NativeLink Benchmarking System

This directory contains an enhanced benchmarking system for NativeLink performance on a per-commit basis, inspired by the [Lucene Nightly Benchmarks](https://blog.mikemccandless.com/2011/04/catching-slowdowns-in-lucene.html) project.

## Overview

The enhanced benchmarking system measures and tracks NativeLink's performance across different commits, inspired by the [Apache Lucene Nightly Benchmarks](https://blog.mikemccandless.com/2011/04/catching-slowdowns-in-lucene.html) project.

The enhanced benchmarking system measures and tracks NativeLink's performance across different commits, allowing developers to identify performance regressions or improvements. The system focuses on several use cases:

1. Build with remote cache only
2. Build with remote cache and execution
3. Clean build with remote execution
4. Incremental build with remote cache and execution

Additionally, the system collects detailed metrics on:
- Network usage (bytes sent/received)
- CPU utilization
- Memory usage
- Cache hit/miss rates

## How It Works

The benchmarking system works by:

1. Running builds of test projects (Rust and C++ codebases) using Bazel with NativeLink
2. Measuring comprehensive performance metrics including build time, resource usage, and network I/O
3. Storing results in a structured format for historical comparison
4. Comparing results between commits to detect performance changes
5. Generating visualizations and HTML reports to track performance trends
6. Reporting significant regressions or improvements

## Usage

### Enhanced Benchmark Tool

You can run the enhanced benchmarking tool with:

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
```

### Arguments

- `--project`: Path to the test project (must be a Bazel workspace)
- `--commit`: NativeLink commit hash being benchmarked
- `--output-dir`: Directory to store benchmark results (default: ./benchmark_results)
- `--nativelink-url`: URL for NativeLink service (default: https://app.nativelink.com)
- `--api-key`: API key for NativeLink service
- `--compare-to`: Previous commit hash to compare against (optional)
- `--runs`: Number of benchmark runs to average (default: 3)
- `--incremental`: Enable incremental build tests (default: True)
- `--network-stats`: Enable network statistics collection (default: True)

### Enhanced Visualization Tool

To visualize benchmark results:

```bash
python3 tools/benchmark/enhanced_visualize.py \
  --input-dir=./benchmark_results \
  --output-dir=./benchmark_viz \
  --project=<project-name> \
  --metric=build_time \
  --last-n-commits=10 \
  --html-report=True \
  --compare-commits=<commit1>,<commit2>
```

### Visualization Arguments

- `--input-dir`: Directory containing benchmark results (default: ./benchmark_results)
- `--output-dir`: Directory to save visualization outputs (default: ./benchmark_viz)
- `--project`: Filter results by project name (optional)
- `--metric`: Metric to visualize (default: build_time)
- `--last-n-commits`: Show only the last N commits (default: 10)
- `--html-report`: Generate HTML dashboard report (default: True)
- `--compare-commits`: Compare specific commits (comma-separated hashes)

## Automated Benchmarking

The enhanced benchmarking system can be integrated into the CI pipeline by updating the existing GitHub Actions workflow (`.github/workflows/benchmark.yaml`). The workflow can be modified to:

1. Run on each push to main and on pull requests
2. Check out test projects (currently rustlings and googletest)
3. Run enhanced benchmarks against the current commit
4. Compare results with the previous commit
5. Generate visualizations and HTML reports
6. Upload results and visualizations as artifacts
7. Post a summary comment with key metrics and links to visualizations on pull requests

## Interpreting Results

Benchmark results are stored in both JSON and CSV formats. The enhanced JSON format includes:

```json
{
  "commit": "commit-hash",
  "timestamp": "YYYYMMDD_HHMMSS",
  "project": "project-name",
  "metrics": {
    "remote_cache_only": {
      "build_time": 10.5,
      "avg_cpu_percent": 45.2,
      "max_cpu_percent": 78.5,
      "avg_memory_percent": 32.1,
      "max_memory_percent": 45.6,
      "bytes_sent": 1024000,
      "bytes_recv": 2048000,
      "total_bytes": 3072000,
      "cache_hits": 120,
      "cache_misses": 5
    },
    "remote_cache_and_execution": {
      "build_time": 5.2,
      "avg_cpu_percent": 25.1,
      "max_cpu_percent": 45.3,
      "avg_memory_percent": 28.4,
      "max_memory_percent": 35.2,
      "bytes_sent": 512000,
      "bytes_recv": 1024000,
      "total_bytes": 1536000,
      "cache_hits": 125,
      "cache_misses": 0,
      "remote_executions": 15
    },
    "clean_remote_execution": {
      "build_time": 8.7,
      "avg_cpu_percent": 30.5,
      "max_cpu_percent": 55.2,
      "avg_memory_percent": 30.1,
      "max_memory_percent": 40.3,
      "bytes_sent": 768000,
      "bytes_recv": 1536000,
      "total_bytes": 2304000,
      "remote_executions": 140
    },
    "incremental_build": {
      "build_time": 2.1,
      "avg_cpu_percent": 20.3,
      "max_cpu_percent": 35.1,
      "avg_memory_percent": 25.2,
      "max_memory_percent": 30.4,
      "bytes_sent": 256000,
      "bytes_recv": 512000,
      "total_bytes": 768000,
      "cache_hits": 135,
      "cache_misses": 1,
      "remote_executions": 2
    }
  }
}
```

The enhanced visualization tool generates:

1. Time series charts for tracking metrics over time
2. Comparison charts for comparing metrics between commits
3. Heatmaps for identifying performance hotspots
4. HTML dashboard reports with summary tables and all visualizations

When comparing results, the system calculates absolute and percentage differences. A warning is generated for significant regressions (for example, >5% slowdown in build time).

## Adding New Test Projects

To add a new test project:

1. Ensure the project uses Bazel as its build system
2. Update the GitHub workflow to check out and benchmark the new project
3. Consider the project's build complexity and stability when interpreting results

## Future Improvements

- Add more sophisticated metrics collection (disk I/O, detailed network analysis)
- Implement a persistent web dashboard for visualizing performance trends
- Add support for more complex build scenarios (for example, specific targets, different toolchains)
- Integrate with notification systems for significant regressions (Slack, email)
- Add support for comparing performance across different NativeLink configurations
- Implement statistical analysis to identify significant changes vs. normal variation

## Key Metrics Tracked
- Build duration (clean and incremental)
- Cache hit ratios
- Network transfer statistics
- Resource utilization (CPU/Memory)