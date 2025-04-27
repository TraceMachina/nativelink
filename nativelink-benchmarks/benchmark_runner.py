#!/usr/bin/env python3

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

"""
NativeLink Per-Commit Benchmarking System

This script implements a benchmarking system for NativeLink on a per-commit basis,
inspired by the Lucene Nightly Benchmarks project. It measures build performance
metrics for specified test projects using Bazel with different NativeLink configurations.

The benchmarks track:
1. Build with remote cache only
2. Build with remote cache and execution

Results are stored in a structured format that allows for tracking performance
trends over time and detecting regressions between commits.

Usage:
  python benchmark_runner.py --project=<project_path> --commit=<commit_hash> [options]

Options:
  --project         Path to the test project (Rust or C++ with Bazel)
  --commit          NativeLink commit hash being benchmarked
  --output-dir      Directory to store benchmark results (default: ./benchmark_results)
  --nativelink-url  URL for NativeLink service (default: https://app.nativelink.com)
  --api-key         API key for NativeLink service
  --compare-to      Previous commit hash to compare against (optional)
  --runs            Number of benchmark runs to average (default: 3)
"""

import argparse
import csv
import datetime
import json
import os
import re
import subprocess
import sys
import time
import psutil
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any


class NetworkStats:
    """Class to track network statistics."""

    def __init__(self):
        self.start_bytes_sent = 0
        self.start_bytes_recv = 0
        self.end_bytes_sent = 0
        self.end_bytes_recv = 0

    def start_monitoring(self):
        """Start monitoring network usage."""
        net_io = psutil.net_io_counters()
        self.start_bytes_sent = net_io.bytes_sent
        self.start_bytes_recv = net_io.bytes_recv

    def stop_monitoring(self):
        """Stop monitoring network usage."""
        net_io = psutil.net_io_counters()
        self.end_bytes_sent = net_io.bytes_sent
        self.end_bytes_recv = net_io.bytes_recv

    def get_stats(self) -> Dict[str, int]:
        """Get network usage statistics."""
        return {
            "bytes_sent": self.end_bytes_sent - self.start_bytes_sent,
            "bytes_recv": self.end_bytes_recv - self.start_bytes_recv,
            "total_bytes": (self.end_bytes_sent - self.start_bytes_sent)
            + (self.end_bytes_recv - self.start_bytes_recv),
        }


class SystemStats:
    """Class to track system resource usage."""

    def __init__(self):
        self.cpu_percent = []
        self.memory_percent = []
        self.monitoring = False
        self.interval = 0.5  # seconds

    def start_monitoring(self):
        """Start monitoring system resources."""
        self.cpu_percent = []
        self.memory_percent = []
        self.monitoring = True

        # Start monitoring in a separate thread
        import threading

        self.monitor_thread = threading.Thread(target=self._monitor_resources)
        self.monitor_thread.daemon = True
        self.monitor_thread.start()

    def _monitor_resources(self):
        """Monitor system resources at regular intervals."""
        while self.monitoring:
            self.cpu_percent.append(psutil.cpu_percent(interval=None))
            self.memory_percent.append(psutil.virtual_memory().percent)
            time.sleep(self.interval)

    def stop_monitoring(self):
        """Stop monitoring system resources."""
        self.monitoring = False
        if hasattr(self, "monitor_thread"):
            self.monitor_thread.join(timeout=1.0)

    def get_stats(self) -> Dict[str, float]:
        """Get system resource usage statistics."""
        if not self.cpu_percent:
            return {
                "avg_cpu_percent": 0,
                "max_cpu_percent": 0,
                "avg_memory_percent": 0,
                "max_memory_percent": 0,
            }

        return {
            "avg_cpu_percent": sum(self.cpu_percent) / len(self.cpu_percent),
            "max_cpu_percent": max(self.cpu_percent),
            "avg_memory_percent": sum(self.memory_percent) / len(self.memory_percent),
            "max_memory_percent": max(self.memory_percent),
        }


class BenchmarkResult:
    """Class to store benchmark results."""

    def __init__(self, commit: str, timestamp: str, project: str):
        self.commit = commit
        self.timestamp = timestamp
        self.project = project
        self.metrics: Dict[str, Dict[str, float]] = {}

    def add_metric(self, test_name: str, metric_name: str, value: float):
        """Add a metric value to the results."""
        if test_name not in self.metrics:
            self.metrics[test_name] = {}
        self.metrics[test_name][metric_name] = value

    def to_dict(self) -> Dict:
        """Convert results to a dictionary."""
        return {
            "commit": self.commit,
            "timestamp": self.timestamp,
            "project": self.project,
            "metrics": self.metrics,
        }

    def to_json(self) -> str:
        """Convert results to a JSON string."""
        return json.dumps(self.to_dict(), indent=2)

    def save_to_file(self, output_dir: str):
        """Save results to JSON and CSV files."""
        # Create output directory if it doesn't exist
        Path(output_dir).mkdir(parents=True, exist_ok=True)

        # Save as JSON
        json_path = os.path.join(output_dir, f"{self.project}_{self.commit}_{self.timestamp}.json")
        with open(json_path, "w") as f:
            f.write(self.to_json())

        # Save as CSV
        csv_path = os.path.join(output_dir, f"{self.project}_{self.commit}_{self.timestamp}.csv")
        with open(csv_path, "w", newline="") as f:
            writer = csv.writer(f)
            writer.writerow(["Test", "Metric", "Value"])
            for test_name, metrics in self.metrics.items():
                for metric_name, value in metrics.items():
                    writer.writerow([test_name, metric_name, value])


def run_command(cmd: List[str], cwd: Optional[str] = None) -> Tuple[str, str, int]:
    """Run a command and return stdout, stderr, and return code."""
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=cwd,
    )
    stdout, stderr = process.communicate()
    return stdout, stderr, process.returncode


def run_bazel_benchmark(
    project_path: str,
    target: str,
    bazelrc_options: List[str],
    clean_first: bool = False,
) -> Tuple[float, Dict[str, Any]]:
    """Run a Bazel benchmark and return the execution time and stats."""
    # Initialize stats collectors
    network_stats = NetworkStats()
    system_stats = SystemStats()

    # Clean if requested
    if clean_first:
        run_command(["bazel", "clean", "--expunge"], cwd=project_path)

    # Start monitoring
    network_stats.start_monitoring()
    system_stats.start_monitoring()

    # Start timing
    start_time = time.time()

    # Run the build
    cmd = ["bazel", "build"] + bazelrc_options + [target]
    stdout, stderr, returncode = run_command(cmd, cwd=project_path)

    # End timing
    end_time = time.time()
    execution_time = end_time - start_time

    # Stop monitoring
    network_stats.stop_monitoring()
    system_stats.stop_monitoring()

    # Collect stats
    stats = {
        "execution_time": execution_time,
        "network": network_stats.get_stats(),
        "system": system_stats.get_stats(),
        "returncode": returncode,
        "stdout": stdout,
        "stderr": stderr,
    }

    return execution_time, stats


def run_benchmarks(
    project_path: str,
    commit: str,
    nativelink_url: str,
    api_key: Optional[str] = None,
    runs: int = 3,
) -> BenchmarkResult:
    """Run all benchmarks for a project and commit."""
    timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    project_name = os.path.basename(project_path)
    result = BenchmarkResult(commit, timestamp, project_name)

    # Common Bazel options
    common_options = [
        "--noshow_progress",
        "--noshow_loading_progress",
        "--color=no",
    ]

    # Target to build (use the project's main target)
    target = "//..."

    # 1. Build with remote cache only
    print("Running benchmark: Build with remote cache only")
    cache_options = common_options + [
        f"--remote_cache={nativelink_url}",
    ]
    if api_key:
        cache_options.append(f"--remote_header=Authorization=Bearer {api_key}")

    cache_times = []
    for i in range(runs):
        print(f"  Run {i+1}/{runs}")
        # Clean first to ensure we're testing cache performance
        time_taken, stats = run_bazel_benchmark(project_path, target, cache_options, clean_first=True)
        cache_times.append(time_taken)
        print(f"  Time: {time_taken:.2f}s")

    avg_cache_time = sum(cache_times) / len(cache_times)
    result.add_metric("remote_cache_only", "build_time", avg_cache_time)

    # 2. Build with remote cache and execution
    print("Running benchmark: Build with remote cache and execution")
    exec_options = cache_options + [
        f"--remote_executor={nativelink_url}",
    ]

    exec_times = []
    for i in range(runs):
        print(f"  Run {i+1}/{runs}")
        # Clean first to ensure consistent state
        time_taken, stats = run_bazel_benchmark(project_path, target, exec_options, clean_first=True)
        exec_times.append(time_taken)
        print(f"  Time: {time_taken:.2f}s")

    avg_exec_time = sum(exec_times) / len(exec_times)
    result.add_metric("remote_cache_and_execution", "build_time", avg_exec_time)

    return result


def main():
    parser = argparse.ArgumentParser(description="NativeLink Benchmarking Tool")
    parser.add_argument(
        "--project", required=True, help="Path to the test project (Rust or C++ with Bazel)"
    )
    parser.add_argument(
        "--commit", required=True, help="NativeLink commit hash being benchmarked"
    )
    parser.add_argument(
        "--output-dir",
        default="./benchmark_results",
        help="Directory to store benchmark results",
    )
    parser.add_argument(
        "--nativelink-url",
        default="https://app.nativelink.com",
        help="URL for NativeLink service",
    )
    parser.add_argument("--api-key", help="API key for NativeLink service")
    parser.add_argument(
        "--compare-to", help="Previous commit hash to compare against (optional)"
    )
    parser.add_argument(
        "--runs", type=int, default=3, help="Number of benchmark runs to average"
    )

    args = parser.parse_args()

    # Validate project path
    if not os.path.isdir(args.project):
        print(f"Error: Project directory '{args.project}' does not exist.")
        return 1

    # Check if project has Bazel
    if not os.path.isfile(os.path.join(args.project, "WORKSPACE")) and not os.path.isfile(
        os.path.join(args.project, "WORKSPACE.bazel")
    ):
        print(
            f"Error: Project directory '{args.project}' does not appear to be a Bazel project."
        )
        return 1

    # Run benchmarks
    try:
        result = run_benchmarks(
            args.project, args.commit, args.nativelink_url, args.api_key, args.runs
        )
        result.save_to_file(args.output_dir)
        print(f"Benchmark results saved to {args.output_dir}")
        print("Summary:")
        print(f"  Remote cache only: {result.metrics['remote_cache_only']['build_time']:.2f}s")
        print(
            f"  Remote cache and execution: {result.metrics['remote_cache_and_execution']['build_time']:.2f}s"
        )
        return 0
    except Exception as e:
        print(f"Error running benchmarks: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())