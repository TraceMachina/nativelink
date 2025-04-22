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
Enhanced NativeLink Benchmarking Tool

This script implements an improved benchmarking system for NativeLink on a per-commit basis,
inspired by the Lucene Nightly Benchmarks project. It measures build performance
metrics for specified test projects using Bazel with different NativeLink configurations.

The benchmarks track:
1. Build with remote cache only
2. Build with remote cache and execution
3. Clean build with remote execution
4. Incremental build with remote cache and execution
5. Network usage metrics
6. CPU and memory utilization

Results are stored in a structured format that allows for tracking performance
trends over time and detecting regressions between commits.

Usage:
  python enhanced_benchmark.py --project=<project_path> --commit=<commit_hash> [options]

Options:
  --project         Path to the test project (Rust or C++ with Bazel)
  --commit          NativeLink commit hash being benchmarked
  --output-dir      Directory to store benchmark results (default: ./benchmark_results)
  --nativelink-url  URL for NativeLink service (default: https://app.nativelink.com)
  --api-key         API key for NativeLink service
  --compare-to      Previous commit hash to compare against (optional)
  --runs            Number of benchmark runs to average (default: 3)
  --incremental     Enable incremental build tests (default: True)
  --network-stats   Enable network statistics collection (default: True)
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
            "total_bytes": (self.end_bytes_sent - self.start_bytes_sent) + 
                          (self.end_bytes_recv - self.start_bytes_recv)
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
        if hasattr(self, 'monitor_thread'):
            self.monitor_thread.join(timeout=1.0)
            
    def get_stats(self) -> Dict[str, float]:
        """Get system resource usage statistics."""
        if not self.cpu_percent:
            return {"avg_cpu_percent": 0, "max_cpu_percent": 0, 
                    "avg_memory_percent": 0, "max_memory_percent": 0}
            
        return {
            "avg_cpu_percent": sum(self.cpu_percent) / len(self.cpu_percent),
            "max_cpu_percent": max(self.cpu_percent),
            "avg_memory_percent": sum(self.memory_percent) / len(self.memory_percent),
            "max_memory_percent": max(self.memory_percent)
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


def make_incremental_change(project_dir: str) -> bool:
    """Make a small change to the project to test incremental builds.
    
    Returns True if a change was successfully made, False otherwise.
    """
    # Try to find a source file to modify
    source_files = []
    
    # Look for C++ files
    for ext in [".cc", ".cpp", ".cxx", ".h", ".hpp"]:
        source_files.extend(list(Path(project_dir).glob(f"**/*{ext}")))
    
    # Look for Rust files
    source_files.extend(list(Path(project_dir).glob("**/*.rs")))
    
    if not source_files:
        print("Could not find any source files to modify for incremental build test")
        return False
    
    # Choose a file to modify
    file_to_modify = source_files[0]
    print(f"Making incremental change to {file_to_modify}")
    
    # Read the file
    with open(file_to_modify, "r") as f:
        content = f.read()
    
    # Add a comment at the end
    timestamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    new_content = content + f"\n// Modified for incremental build test at {timestamp}\n"
    
    # Write the file back
    with open(file_to_modify, "w") as f:
        f.write(new_content)
    
    return True


def extract_bazel_metrics(stdout: str, stderr: str) -> Dict[str, float]:
    """Extract additional metrics from Bazel output."""
    metrics = {}
    
    # Extract build time from stdout
    build_time_match = re.search(r'Elapsed time: ([0-9.]+)s', stdout)
    if build_time_match:
        metrics["bazel_reported_time"] = float(build_time_match.group(1))
    
    # Extract cache hits/misses if available
    cache_hits_match = re.search(r'([0-9]+) cache hits', stdout)
    if cache_hits_match:
        metrics["cache_hits"] = int(cache_hits_match.group(1))
    
    cache_misses_match = re.search(r'([0-9]+) cache misses', stdout)
    if cache_misses_match:
        metrics["cache_misses"] = int(cache_misses_match.group(1))
    
    # Extract remote execution stats if available
    remote_exec_match = re.search(r'([0-9]+) remote', stdout)
    if remote_exec_match:
        metrics["remote_executions"] = int(remote_exec_match.group(1))
    
    return metrics


def run_benchmark(
    project_dir: str,
    test_name: str,
    bazel_args: List[str],
    runs: int = 3,
    collect_network_stats: bool = True,
) -> Dict[str, float]:
    """Run a benchmark and return metrics."""
    print(f"Running benchmark: {test_name}")
    
    build_times = []
    system_stats_list = []
    network_stats_list = []
    bazel_metrics_list = []
    
    for i in range(runs):
        print(f"  Run {i+1}/{runs}")
        clean_bazel_workspace(project_dir)
        
        # Initialize stats collectors
        system_stats = SystemStats()
        network_stats = NetworkStats() if collect_network_stats else None
        
        # Start monitoring
        system_stats.start_monitoring()
        if network_stats:
            network_stats.start_monitoring()
        
        # Measure time and run build
        start_time = time.time()
        cmd = ["bazel", "build", "//..."] + bazel_args
        stdout, stderr, return_code = run_command(cmd, cwd=project_dir)
        end_time = time.time()
        
        # Stop monitoring
        system_stats.stop_monitoring()
        if network_stats:
            network_stats.stop_monitoring()
        
        if return_code != 0:
            print(f"Error running benchmark: {stderr}")
            continue
        
        # Calculate metrics
        build_time = end_time - start_time
        build_times.append(build_time)
        
        # Get system stats
        system_stats_list.append(system_stats.get_stats())
        
        # Get network stats
        if network_stats:
            network_stats_list.append(network_stats.get_stats())
        
        # Extract metrics from Bazel output
        bazel_metrics = extract_bazel_metrics(stdout, stderr)
        bazel_metrics_list.append(bazel_metrics)
        
        print(f"  Build time: {build_time:.2f}s")
    
    # Calculate average metrics
    metrics = {}
    
    # Build time
    if build_times:
        metrics["build_time"] = sum(build_times) / len(build_times)
    
    # System stats
    if system_stats_list:
        for key in ["avg_cpu_percent", "max_cpu_percent", "avg_memory_percent", "max_memory_percent"]:
            values = [stats.get(key, 0) for stats in system_stats_list]
            if values:
                metrics[key] = sum(values) / len(values)
    
    # Network stats
    if network_stats_list:
        for key in ["bytes_sent", "bytes_recv", "total_bytes"]:
            values = [stats.get(key, 0) for stats in network_stats_list]
            if values:
                metrics[key] = sum(values) / len(values)
    
    # Bazel metrics
    if bazel_metrics_list:
        for key in set().union(*[metrics.keys() for metrics in bazel_metrics_list]):
            values = [metrics.get(key, 0) for metrics in bazel_metrics_list if key in metrics]
            if values:
                metrics[key] = sum(values) / len(values)
    
    return metrics


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
    parser = argparse.ArgumentParser(description="Enhanced NativeLink Benchmarking Tool")
    # Add GitHub-specific parameters
    parser.add_argument("--github-run-id", help="GitHub Actions run ID for correlation")
    parser.add_argument("--github-sha", help="Git commit SHA being benchmarked")
    
    # Add regression detection threshold
    parser.add_argument("--regression-threshold", type=float, default=5.0,
                        help="Percentage threshold for performance regression alerts")
    parser.add_argument("--project", required=True, help="Path to the test project")
    parser.add_argument("--commit", required=True, help="NativeLink commit hash being benchmarked")
    parser.add_argument("--output-dir", default="./benchmark_results", help="Directory to store benchmark results")
    parser.add_argument("--nativelink-url", default="https://app.nativelink.com", help="URL for NativeLink service")
    parser.add_argument("--api-key", help="API key for NativeLink service")
    parser.add_argument("--compare-to", help="Previous commit hash to compare against")
    parser.add_argument("--runs", type=int, default=3, help="Number of benchmark runs to average")
    parser.add_argument("--incremental", action="store_true", default=True, help="Enable incremental build tests")
    parser.add_argument("--network-stats", action="store_true", default=True, help="Enable network statistics collection")
    
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
        {
            "name": "clean_remote_execution",
            "args": [
                f"--remote_executor={args.nativelink_url}",
                f"--remote_header=x-nativelink-api-key={args.api_key}" if args.api_key else "",
                "--jobs=200",
                "--noremote_accept_cached",  # Don't use cache for this test
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
            collect_network_stats=args.network_stats,
        )
        
        for metric_name, value in metrics.items():
            results.add_metric(benchmark["name"], metric_name, value)
    
    # Run incremental build benchmark if enabled
    if args.incremental:
        # Make a small change to the project
        if make_incremental_change(project_dir):
            # Run incremental build with remote cache and execution
            incremental_args = [
                f"--remote_cache={args.nativelink_url}",
                f"--remote_executor={args.nativelink_url}",
                f"--remote_header=x-nativelink-api-key={args.api_key}" if args.api_key else "",
                "--jobs=200",
            ]
            
            # Filter out empty args
            incremental_args = [arg for arg in incremental_args if arg]
            
            metrics = run_benchmark(
                project_dir,
                "incremental_build",
                incremental_args,
                runs=args.runs,
                collect_network_stats=args.network_stats,
            )
            
            for metric_name, value in metrics.items():
                results.add_metric("incremental_build", metric_name, value)
    
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
                    print(f"    Diff: {values['diff']:.2f} ({values['percent_change']:.2f}%)")
                    
                    # Highlight significant regressions
                    if values['percent_change'] > 5 and metric_name == "build_time":
                        print(f"    WARNING: Performance regression detected!")
        else:
            print(f"No previous results found for commit {args.compare_to}")

    # Add GitHub Actions workflow integration
    if os.getenv("GITHUB_ACTIONS") == "true":
        github_output = os.getenv("GITHUB_OUTPUT")
        with open(github_output, "a") as f:
            f.write(f"benchmark-results={json.dumps(results.to_dict())}\n")

if __name__ == "__main__":
    main()