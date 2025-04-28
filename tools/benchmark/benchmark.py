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
3. Clean build with remote execution
4. Incremental build with remote cache and execution
5. Network usage metrics
6. CPU and memory utilization

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
  --incremental     Enable incremental build tests (default: True)
  --network-stats   Enable network statistics collection (default: True)
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
        self.network_stats: Dict[str, Dict[str, float]] = {}
        self.system_stats: Dict[str, Dict[str, float]] = {}

    def add_metric(self, test_name: str, metric_name: str, value: float):
        """Add a metric value to the results."""
        if test_name not in self.metrics:
            self.metrics[test_name] = {}
        self.metrics[test_name][metric_name] = value
        
    def add_network_stats(self, test_name: str, stats: Dict[str, float]):
        """Add network statistics to the results."""
        if test_name not in self.network_stats:
            self.network_stats[test_name] = {}
        self.network_stats[test_name].update(stats)
        
    def add_system_stats(self, test_name: str, stats: Dict[str, float]):
        """Add system statistics to the results."""
        if test_name not in self.system_stats:
            self.system_stats[test_name] = {}
        self.system_stats[test_name].update(stats)

    def to_dict(self) -> Dict:
        """Convert results to a dictionary."""
        return {
            "commit_hash": self.commit,
            "date": datetime.datetime.now().strftime("%Y-%m-%d"),
            "build_remote_cache_time_sec": self.metrics.get("remote_cache", {}).get("build_time", 0.0),
            "build_remote_execution_time_sec": self.metrics.get("remote_execution", {}).get("build_time", 0.0),
            "metrics": self.metrics,
            "network_stats": self.network_stats,
            "system_stats": self.system_stats,
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

        # Optionally upload to cloud storage if configured
        if os.environ.get("UPLOAD_TO_CLOUD") == "true":
            self.upload_to_cloud(json_path)
            
        # Optionally commit to git repository if configured
        if os.environ.get("COMMIT_TO_GIT") == "true":
            self.commit_to_git(json_path)

    def upload_to_cloud(self, file_path: str):
        """Upload benchmark results to cloud storage."""
        try:
            import boto3
            
            s3 = boto3.client('s3',
                aws_access_key_id=os.environ.get('AWS_ACCESS_KEY_ID'),
                aws_secret_access_key=os.environ.get('AWS_SECRET_ACCESS_KEY'),
                region_name=os.environ.get('AWS_REGION'))
                
            bucket_name = os.environ.get('AWS_BUCKET_NAME')
            if bucket_name:
                s3.upload_file(
                    file_path,
                    bucket_name,
                    f"benchmark_results/{os.path.basename(file_path)}"
                )
                print(f"Successfully uploaded {file_path} to cloud storage")
        except ImportError:
            print("boto3 not installed, skipping cloud upload")
        except Exception as e:
            print(f"Failed to upload to cloud storage: {e}")
            
    def commit_to_git(self, file_path: str):
        """Commit benchmark results to git repository."""
        try:
            import git
            
            repo = git.Repo(os.path.dirname(os.path.dirname(file_path)))
            repo.git.add(file_path)
            repo.index.commit(f"Add benchmark results: {os.path.basename(file_path)}")
            print(f"Successfully committed {file_path} to git repository")
            
            if os.environ.get("PUSH_TO_REMOTE") == "true":
                origin = repo.remote(name="origin")
                origin.push()
                print(f"Pushed benchmark results to remote repository")
        except ImportError:
            print("gitpython not installed, skipping git commit")
        except Exception as e:
            print(f"Failed to commit to git repository: {e}")


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
    args: Optional[argparse.Namespace] = None) -> Dict[str, float]:
    """Run a benchmark and return metrics."""
    print(f"Running benchmark: {test_name}")

    build_times = []
    peak_memories = []
    cache_hit_rates = []
    action_counts = []
    remote_execution_times = []

    for i in range(runs):
        print(f"  Run {i+1}/{runs}")
        clean_bazel_workspace(project_dir)

        # Measure time and memory usage
        start_time = time.time()
        cmd = ["bazel", "build", "--experimental_remote_grpc_log=./bazel_remote.log", "//..."] + bazel_args
        stdout, stderr, return_code = run_command(cmd, cwd=project_dir)
        end_time = time.time()

        if return_code != 0:
            print(f"Error running benchmark: {stderr}")
            continue

        build_time = end_time - start_time
        build_times.append(build_time)

        # Parse Bazel output for detailed metrics
        cache_hits = 0
        total_actions = 0
        remote_exec_time = 0
        
        # Parse remote execution log if available
        log_path = os.path.join(project_dir, "bazel_remote.log")
        if os.path.exists(log_path):
            with open(log_path) as f:
                for line in f:
                    if "cache hit" in line:
                        cache_hits += 1
                    if "action completed" in line:
                        total_actions += 1
                        if "remote execution" in line:
                            match = re.search(r"execution_time: (\d+\.\d+)s", line)
                            if match:
                                remote_exec_time += float(match.group(1))
            
            if total_actions > 0:
                cache_hit_rates.append(cache_hits / total_actions)
                action_counts.append(total_actions)
                remote_execution_times.append(remote_exec_time)
            
            os.remove(log_path)

        print(f"  Build time: {build_time:.2f}s")

    # Calculate average metrics
    avg_build_time = sum(build_times) / len(build_times) if build_times else 0
    avg_cache_hit_rate = sum(cache_hit_rates) / len(cache_hit_rates) if cache_hit_rates else 0
    avg_action_count = sum(action_counts) / len(action_counts) if action_counts else 0
    avg_remote_exec_time = sum(remote_execution_times) / len(remote_execution_times) if remote_execution_times else 0

    return {
        "build_time": avg_build_time,
        "cache_hit_rate": avg_cache_hit_rate,
        "total_actions": avg_action_count,
        "remote_execution_time": avg_remote_exec_time,
        "timestamp": datetime.datetime.now().isoformat(),
        "bazel_args": " ".join(bazel_args),
        "build_mode": "remote_cache" if "--remote_cache" in bazel_args and "--remote_executor" not in bazel_args else "remote_execution" if "--remote_executor" in bazel_args else "local",
        "commit_hash": args.commit if args and hasattr(args, 'commit') else "",
        "project_name": os.path.basename(args.project) if args and hasattr(args, 'project') else ""
    }


def parse_build_logs(project_path: str, bazel_args: List[str]) -> Dict[str, float]:
    """Parse Bazel build logs to extract performance metrics."""
    log_path = os.path.join(project_path, "bazel.log")
    metrics = {
        "cache_hit_rate": 0.0,
        "total_actions": 0,
        "remote_execution_time": 0.0
    }
    
    if os.path.exists(log_path):
        with open(log_path, 'r') as f:
            for line in f:
                if "INFO: " in line:
                    if "cache hit rate" in line:
                        metrics["cache_hit_rate"] = float(line.split("cache hit rate: ")[1].split("%")[0]) / 100
                    elif "total actions" in line:
                        metrics["total_actions"] = int(line.split("total actions: ")[1].split()[0])
                    elif "remote execution" in line and "time" in line:
                        metrics["remote_execution_time"] = float(line.split("time: ")[1].split()[0])
    
    return metrics

def run_test(test_type: str, project_path: str, result: BenchmarkResult, args: argparse.Namespace):
    """Run a single benchmark test."""
    bazel_args = []
    if not hasattr(args, 'commit') or not hasattr(args, 'project'):
        raise ValueError("Missing required arguments: commit and project must be provided")
    
    if test_type == "remote_cache_only":
        bazel_args = ["--remote_cache=grpc://localhost:8980", "build", "//..."]
    elif test_type == "remote_cache_execution":
        bazel_args = ["--remote_executor=grpc://localhost:8980", "--remote_cache=grpc://localhost:8980", "build", "//..."]
    elif test_type == "clean_remote_execution":
        bazel_args = ["--remote_executor=grpc://localhost:8980", "--remote_cache=grpc://localhost:8980", "--noremote_accept_cached", "build", "//..."]
    elif test_type == "incremental_build":
        bazel_args = ["--remote_executor=grpc://localhost:8980", "--remote_cache=grpc://localhost:8980", "build", "//..."]
    
    # Run bazel command
    start_time = time.time()
    subprocess.run(["bazel"] + bazel_args, cwd=project_path, check=True)
    build_time = time.time() - start_time
    
    # Parse build logs and collect metrics
    metrics = parse_build_logs(project_path, bazel_args)
    metrics["build_time"] = build_time
    
    # Add metrics to result
    result.add_metric(test_type, "build_time", build_time)
    for metric_name, value in metrics.items():
        if metric_name != "build_time":
            result.add_metric(test_type, metric_name, value)

def run_benchmark_tests(args):
    """Run the benchmark with given arguments."""
    timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    result = BenchmarkResult(args.commit, timestamp, os.path.basename(args.project))

    # Run benchmarks
    for i in range(args.runs):
        print(f"Running benchmark iteration {i+1}/{args.runs}")
        
        # Remote cache only
        run_test("remote_cache_only", args.project, result, args)
        
        # Remote cache and execution
        run_test("remote_cache_execution", args.project, result, args)
        
        if args.incremental:
            # Clean build with remote execution
            run_test("clean_remote_execution", args.project, result)
            
            # Incremental build with remote cache and execution
            run_test("incremental_build", args.project, result)
        
        if args.network_stats:
            # Collect network statistics
            network_stats = get_network_stats()
            result.add_network_stats("network", network_stats)
            
            # Collect system statistics
            system_stats = get_system_stats()
            result.add_system_stats("system", system_stats)
    
    return result


import re
import argparse
import psutil
from typing import Dict

def get_network_stats() -> Dict[str, float]:
    """Collect network interface statistics."""
    stats = {}
    net_io = psutil.net_io_counters()
    stats["bytes_sent"] = net_io.bytes_sent
    stats["bytes_recv"] = net_io.bytes_recv
    stats["packets_sent"] = net_io.packets_sent
    stats["packets_recv"] = net_io.packets_recv
    return stats
    
def get_system_stats() -> Dict[str, float]:
    """Collect system resource utilization statistics."""
    stats = {}
    stats["cpu_percent"] = psutil.cpu_percent()
    stats["memory_percent"] = psutil.virtual_memory().percent
    stats["disk_usage"] = psutil.disk_usage('/').percent
    return stats

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


def parse_args():
    parser = argparse.ArgumentParser(description="NativeLink Benchmarking Tool")
    parser.add_argument("--project", required=True, help="Path to the test project")
    parser.add_argument("--commit", required=True, help="NativeLink commit hash being benchmarked")
    parser.add_argument(
        "--output-dir", default="./benchmark_results", help="Directory to store benchmark results"
    )
    parser.add_argument(
        "--nativelink-url", default="https://app.nativelink.com", help="URL for NativeLink service"
    )
    parser.add_argument("--api-key", help="API key for NativeLink service")
    parser.add_argument("--compare-to", help="Previous commit hash to compare against")
    parser.add_argument("--runs", type=int, default=3, help="Number of benchmark runs to average")
    parser.add_argument("--incremental", type=bool, default=True, help="Run incremental build tests")
    parser.add_argument("--network-stats", type=bool, default=True, help="Collect network statistics")
    return parser.parse_args()

def main():
    """Main entry point for the benchmark script."""
    args = parse_args()
    
    # Validate project directory
    if not os.path.isdir(args.project):
        print(f"Error: Project directory {args.project} does not exist")
        sys.exit(1)
    
    # Check for required dependencies if network stats enabled
    if args.network_stats:
        try:
            import psutil
        except ImportError:
            print("Error: psutil package required for network stats. Install with: pip install psutil")
            sys.exit(1)
    
    # Run benchmarks
    result = run_benchmark_tests(args)
    
    # Save results
    result.save_to_file(args.output_dir)
    
    # Compare with previous results if specified
    if args.compare_to:
        previous_result = load_previous_result(args.output_dir, args.compare_to, args.project)
        if previous_result:
            comparison = compare_results(result, previous_result)
            print("Comparison results:", json.dumps(comparison, indent=2))
    
    print("Benchmark completed successfully.")

if __name__ == "__main__":
    main()
