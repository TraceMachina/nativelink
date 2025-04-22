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
NativeLink Benchmarking Tool

This script implements a benchmarking system for NativeLink on a per-commit basis,
inspired by the Lucene Nightly Benchmarks project. It measures build performance
metrics for specified test projects using Bazel with different NativeLink configurations.

The benchmarks track:
1. Build with remote cache only
2. Build with remote cache and execution

Results are stored in a structured format that allows for tracking performance
trends over time and detecting regressions between commits.

Usage:
  python benchmark.py --project=<project_path> --commit=<commit_hash> [options]

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
import subprocess
import sys
import time
from pathlib import Path
from typing import Dict, List, Optional, Tuple


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
        json_path = os.path.join(
            output_dir, f"{self.project}_{self.commit}_{self.timestamp}.json"
        )
        with open(json_path, "w") as f:
            f.write(self.to_json())

        # Save as CSV
        csv_path = os.path.join(
            output_dir, f"{self.project}_{self.commit}_{self.timestamp}.csv"
        )
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


def clean_bazel_workspace(project_dir: str):
    """Clean the Bazel workspace to ensure a fresh build."""
    print("Cleaning Bazel workspace...")
    run_command(["bazel", "clean", "--expunge"], cwd=project_dir)


def run_benchmark(
    project_dir: str,
    test_name: str,
    bazel_args: List[str],
    runs: int = 3,
) -> Dict[str, float]:
    """Run a benchmark and return metrics."""
    print(f"Running benchmark: {test_name}")
    
    build_times = []
    peak_memories = []
    
    for i in range(runs):
        print(f"  Run {i+1}/{runs}")
        clean_bazel_workspace(project_dir)
        
        # Measure time and memory usage
        start_time = time.time()
        cmd = ["bazel", "build", "//..."] + bazel_args
        stdout, stderr, return_code = run_command(cmd, cwd=project_dir)
        end_time = time.time()
        
        if return_code != 0:
            print(f"Error running benchmark: {stderr}")
            continue
        
        build_time = end_time - start_time
        build_times.append(build_time)
        
        # Extract memory usage from Bazel output if available
        # This is a simplified approach - in practice, you might need to parse Bazel's output
        # or use a more sophisticated method to measure memory usage
        peak_memory = 0  # Placeholder
        peak_memories.append(peak_memory)
        
        print(f"  Build time: {build_time:.2f}s")
    
    # Calculate average metrics
    avg_build_time = sum(build_times) / len(build_times) if build_times else 0
    avg_peak_memory = sum(peak_memories) / len(peak_memories) if peak_memories else 0
    
    return {
        "build_time": avg_build_time,
        "peak_memory": avg_peak_memory,
    }


def compare_results(current: BenchmarkResult, previous: BenchmarkResult) -> Dict:
    """Compare current benchmark results with previous results."""
    comparison = {}
    
    for test_name, current_metrics in current.metrics.items():
        comparison[test_name] = {}
        
        if test_name in previous.metrics:
            prev_metrics = previous.metrics[test_name]
            
            for metric_name, current_value in current_metrics.items():
                if metric_name in prev_metrics:
                    prev_value = prev_metrics[metric_name]
                    diff = current_value - prev_value
                    percent_change = (diff / prev_value) * 100 if prev_value != 0 else 0
                    
                    comparison[test_name][metric_name] = {
                        "current": current_value,
                        "previous": prev_value,
                        "diff": diff,
                        "percent_change": percent_change,
                    }
    
    return comparison


def load_previous_result(output_dir: str, project: str, commit: str) -> Optional[BenchmarkResult]:
    """Load previous benchmark results for comparison."""
    result_files = list(Path(output_dir).glob(f"{project}_{commit}_*.json"))
    
    if not result_files:
        return None
    
    # Use the most recent result if multiple exist
    latest_file = sorted(result_files)[-1]
    
    with open(latest_file, "r") as f:
        data = json.load(f)
        
    result = BenchmarkResult(data["commit"], data["timestamp"], data["project"])
    result.metrics = data["metrics"]
    
    return result


def main():
    parser = argparse.ArgumentParser(description="NativeLink Benchmarking Tool")
    parser.add_argument("--project", required=True, help="Path to the test project")
    parser.add_argument("--commit", required=True, help="NativeLink commit hash being benchmarked")
    parser.add_argument("--output-dir", default="./benchmark_results", help="Directory to store benchmark results")
    parser.add_argument("--nativelink-url", default="https://app.nativelink.com", help="URL for NativeLink service")
    parser.add_argument("--api-key", help="API key for NativeLink service")
    parser.add_argument("--compare-to", help="Previous commit hash to compare against")
    parser.add_argument("--runs", type=int, default=3, help="Number of benchmark runs to average")
    
    args = parser.parse_args()
    
    # Validate project directory
    project_dir = os.path.abspath(args.project)
    if not os.path.isdir(project_dir):
        print(f"Error: Project directory '{project_dir}' does not exist")
        sys.exit(1)
    
    # Check for Bazel workspace
    if not any(os.path.exists(os.path.join(project_dir, f)) for f in ["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel"]):
        print(f"Error: Project directory '{project_dir}' does not appear to be a Bazel workspace")
        sys.exit(1)
    
    # Create timestamp for this benchmark run
    timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    
    # Extract project name from directory
    project_name = os.path.basename(project_dir)
    
    # Initialize results
    results = BenchmarkResult(args.commit, timestamp, project_name)
    
    # Define benchmark configurations
    benchmarks = [
        {
            "name": "remote_cache_only",
            "args": [
                f"--remote_cache={args.nativelink_url}",
                f"--remote_header=x-nativelink-api-key={args.api_key}" if args.api_key else "",
            ],
        },
        {
            "name": "remote_cache_and_execution",
            "args": [
                f"--remote_cache={args.nativelink_url}",
                f"--remote_executor={args.nativelink_url}",
                f"--remote_header=x-nativelink-api-key={args.api_key}" if args.api_key else "",
                "--jobs=200",
            ],
        },
    ]
    
    # Run benchmarks
    for benchmark in benchmarks:
        # Filter out empty args
        args_list = [arg for arg in benchmark["args"] if arg]
        metrics = run_benchmark(
            project_dir,
            benchmark["name"],
            args_list,
            runs=args.runs,
        )
        
        for metric_name, value in metrics.items():
            results.add_metric(benchmark["name"], metric_name, value)
    
    # Save results
    results.save_to_file(args.output_dir)
    print(f"Benchmark results saved to {args.output_dir}")
    
    # Compare with previous results if requested
    if args.compare_to:
        previous_results = load_previous_result(args.output_dir, project_name, args.compare_to)
        
        if previous_results:
            comparison = compare_results(results, previous_results)
            
            print("\nComparison with previous results:")
            for test_name, metrics in comparison.items():
                print(f"\n{test_name}:")
                for metric_name, values in metrics.items():
                    print(f"  {metric_name}:")
                    print(f"    Current: {values['current']:.2f}")
                    print(f"    Previous: {values['previous']:.2f}")
                    print(f"    Difference: {values['diff']:.2f} ({values['percent_change']:.2f}%)")
                    
                    # Alert on significant regressions (e.g., >5% slowdown)
                    if metric_name == "build_time" and values["percent_change"] > 5:
                        print(f"    WARNING: Performance regression detected!")
        else:
            print(f"No previous results found for commit {args.compare_to}")


if __name__ == "__main__":
    main()