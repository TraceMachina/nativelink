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
NativeLink Benchmark Visualization Generator

This script processes benchmark results and generates data files for the visualization dashboard.
It aggregates results across commits and creates JSON files that can be loaded by the web interface.

Usage:
  python generate_visualization.py --data-dir=<benchmark_results_dir> --output-dir=<visualization_data_dir>
"""

import argparse
import glob
import json
import os
import sys
from pathlib import Path
from typing import Dict, List, Any


def load_benchmark_results(data_dir: str) -> List[Dict[str, Any]]:
    """Load all benchmark result JSON files from the data directory."""
    results = []
    json_files = glob.glob(os.path.join(data_dir, "*.json"))
    
    for json_file in json_files:
        try:
            with open(json_file, "r") as f:
                data = json.load(f)
                results.append(data)
        except Exception as e:
            print(f"Error loading {json_file}: {e}")
    
    # Sort results by timestamp
    results.sort(key=lambda x: x.get("timestamp", ""))
    return results


def generate_time_series_data(results: List[Dict[str, Any]]) -> Dict[str, Any]:
    """Generate time series data for build time trends."""
    time_series = {
        "commits": [],
        "timestamps": [],
        "remote_cache_only": [],
        "remote_cache_and_execution": [],
    }
    
    for result in results:
        commit = result.get("commit", "unknown")
        timestamp = result.get("timestamp", "")
        metrics = result.get("metrics", {})
        
        cache_only = metrics.get("remote_cache_only", {}).get("build_time", 0)
        cache_exec = metrics.get("remote_cache_and_execution", {}).get("build_time", 0)
        
        time_series["commits"].append(commit[:8])  # Short commit hash
        time_series["timestamps"].append(timestamp)
        time_series["remote_cache_only"].append(cache_only)
        time_series["remote_cache_and_execution"].append(cache_exec)
    
    return time_series


def generate_commit_comparison(results: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Generate commit-by-commit comparison data."""
    comparisons = []
    
    for i, result in enumerate(results):
        commit = result.get("commit", "unknown")
        timestamp = result.get("timestamp", "")
        metrics = result.get("metrics", {})
        
        cache_only = metrics.get("remote_cache_only", {}).get("build_time", 0)
        cache_exec = metrics.get("remote_cache_and_execution", {}).get("build_time", 0)
        
        comparison = {
            "commit": commit[:8],
            "full_commit": commit,
            "timestamp": timestamp,
            "cache_only": cache_only,
            "cache_exec": cache_exec,
        }
        
        # Calculate change from previous commit
        if i > 0:
            prev_result = results[i-1]
            prev_metrics = prev_result.get("metrics", {})
            prev_cache_only = prev_metrics.get("remote_cache_only", {}).get("build_time", 0)
            prev_cache_exec = prev_metrics.get("remote_cache_and_execution", {}).get("build_time", 0)
            
            if prev_cache_only > 0:
                comparison["cache_only_change"] = ((cache_only - prev_cache_only) / prev_cache_only) * 100
            else:
                comparison["cache_only_change"] = 0
                
            if prev_cache_exec > 0:
                comparison["cache_exec_change"] = ((cache_exec - prev_cache_exec) / prev_cache_exec) * 100
            else:
                comparison["cache_exec_change"] = 0
        else:
            comparison["cache_only_change"] = 0
            comparison["cache_exec_change"] = 0
        
        comparisons.append(comparison)
    
    return comparisons


def generate_resource_usage_data(results: List[Dict[str, Any]]) -> Dict[str, Any]:
    """Generate resource usage data across commits."""
    resource_data = {
        "commits": [],
        "cpu": [],
        "memory": [],
    }
    
    for result in results:
        commit = result.get("commit", "unknown")
        metrics = result.get("metrics", {})
        
        # Get system stats from remote_cache_and_execution test (as representative)
        system_stats = metrics.get("remote_cache_and_execution", {}).get("system", {})
        cpu_percent = system_stats.get("avg_cpu_percent", 0)
        memory_percent = system_stats.get("avg_memory_percent", 0)
        
        resource_data["commits"].append(commit[:8])
        resource_data["cpu"].append(cpu_percent)
        resource_data["memory"].append(memory_percent)
    
    return resource_data


def generate_visualization_data(data_dir: str, output_dir: str) -> None:
    """Generate all visualization data files."""
    # Create output directory if it doesn't exist
    Path(output_dir).mkdir(parents=True, exist_ok=True)
    
    # Load benchmark results
    results = load_benchmark_results(data_dir)
    if not results:
        print(f"No benchmark results found in {data_dir}")
        return
    
    # Generate time series data
    time_series = generate_time_series_data(results)
    with open(os.path.join(output_dir, "time_series.json"), "w") as f:
        json.dump(time_series, f, indent=2)
    
    # Generate commit comparison data
    comparisons = generate_commit_comparison(results)
    with open(os.path.join(output_dir, "commit_comparison.json"), "w") as f:
        json.dump(comparisons, f, indent=2)
    
    # Generate resource usage data
    resource_data = generate_resource_usage_data(results)
    with open(os.path.join(output_dir, "resource_usage.json"), "w") as f:
        json.dump(resource_data, f, indent=2)
    
    # Generate latest results data (for dashboard)
    if results:
        latest = results[-1]
        with open(os.path.join(output_dir, "latest.json"), "w") as f:
            json.dump(latest, f, indent=2)
    
    print(f"Generated visualization data in {output_dir}")


def main():
    parser = argparse.ArgumentParser(description="NativeLink Benchmark Visualization Generator")
    parser.add_argument(
        "--data-dir", required=True, help="Directory containing benchmark result JSON files"
    )
    parser.add_argument(
        "--output-dir", required=True, help="Directory to output visualization data files"
    )
    
    args = parser.parse_args()
    generate_visualization_data(args.data_dir, args.output_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())