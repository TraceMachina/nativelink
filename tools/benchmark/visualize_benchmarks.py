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
NativeLink Benchmark Visualization Tool

This script generates visualizations from benchmark results to help track
performance trends over time and identify regressions between commits.

Usage:
  python visualize_benchmarks.py --input-dir=<benchmark_results_dir> [options]

Options:
  --input-dir       Directory containing benchmark results (default: ./benchmark_results)
  --output-dir      Directory to save visualization outputs (default: ./benchmark_viz)
  --project         Filter results by project name (optional)
  --metric          Metric to visualize (default: build_time)
  --last-n-commits  Show only the last N commits (default: 10)
"""

import argparse
import json
import os
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple

try:
    import matplotlib.pyplot as plt
    import numpy as np
    import pandas as pd
except ImportError:
    print("Please install required packages: pip install matplotlib numpy pandas")
    exit(1)


def load_benchmark_results(input_dir: str, project: Optional[str] = None) -> List[Dict]:
    """Load benchmark results from JSON files."""
    results = []
    pattern = f"{project}_*.json" if project else "*.json"
    
    for file_path in Path(input_dir).glob(pattern):
        with open(file_path, "r") as f:
            try:
                data = json.load(f)
                # Add parsed timestamp for sorting
                timestamp_str = data.get("timestamp", "")
                if timestamp_str:
                    try:
                        data["parsed_timestamp"] = datetime.strptime(timestamp_str, "%Y%m%d_%H%M%S")
                    except ValueError:
                        data["parsed_timestamp"] = datetime.min
                results.append(data)
            except json.JSONDecodeError:
                print(f"Warning: Could not parse {file_path}")
    
    # Sort by timestamp
    results.sort(key=lambda x: x.get("parsed_timestamp", datetime.min))
    return results


def create_time_series_chart(
    results: List[Dict],
    metric: str = "build_time",
    test_names: Optional[List[str]] = None,
    last_n: int = 10,
) -> plt.Figure:
    """Create a time series chart of benchmark results."""
    # Prepare data
    data = []
    
    # Use all test names if none specified
    if not test_names and results:
        test_names = list(results[0].get("metrics", {}).keys())
    
    for result in results[-last_n:]:
        commit = result.get("commit", "unknown")[:7]  # Short commit hash
        timestamp = result.get("timestamp", "")
        
        for test_name in test_names or []:
            if test_name in result.get("metrics", {}):
                test_metrics = result["metrics"][test_name]
                if metric in test_metrics:
                    data.append({
                        "commit": commit,
                        "timestamp": timestamp,
                        "test": test_name,
                        "value": test_metrics[metric],
                    })
    
    if not data:
        print(f"No data found for metric: {metric}")
        fig, ax = plt.subplots(figsize=(10, 6))
        ax.text(0.5, 0.5, f"No data found for metric: {metric}", 
                ha="center", va="center", fontsize=12)
        return fig
    
    # Convert to DataFrame for easier plotting
    df = pd.DataFrame(data)
    
    # Create plot
    fig, ax = plt.subplots(figsize=(12, 7))
    
    for test_name, group in df.groupby("test"):
        ax.plot(group["commit"], group["value"], marker="o", label=test_name)
    
    # Add labels and title
    ax.set_xlabel("Commit")
    ax.set_ylabel(f"{metric.replace('_', ' ').title()}")
    ax.set_title(f"NativeLink Performance: {metric.replace('_', ' ').title()} Over Time")
    
    # Rotate x-axis labels for better readability
    plt.xticks(rotation=45, ha="right")
    
    # Add legend
    ax.legend()
    
    # Add grid
    ax.grid(True, linestyle="--", alpha=0.7)
    
    # Tight layout to ensure everything fits
    fig.tight_layout()
    
    return fig


def create_comparison_chart(
    results: List[Dict],
    metric: str = "build_time",
    test_names: Optional[List[str]] = None,
    last_n: int = 2,
) -> plt.Figure:
    """Create a bar chart comparing the last N benchmark results."""
    # Use only the last N results
    recent_results = results[-last_n:]
    
    if len(recent_results) < 2:
        print("Need at least 2 results for comparison")
        fig, ax = plt.subplots(figsize=(10, 6))
        ax.text(0.5, 0.5, "Need at least 2 results for comparison", 
                ha="center", va="center", fontsize=12)
        return fig
    
    # Use all test names if none specified
    if not test_names and recent_results:
        test_names = list(recent_results[0].get("metrics", {}).keys())
    
    # Prepare data
    data = []
    for result in recent_results:
        commit = result.get("commit", "unknown")[:7]  # Short commit hash
        
        for test_name in test_names or []:
            if test_name in result.get("metrics", {}):
                test_metrics = result["metrics"][test_name]
                if metric in test_metrics:
                    data.append({
                        "commit": commit,
                        "test": test_name,
                        "value": test_metrics[metric],
                    })
    
    if not data:
        print(f"No data found for metric: {metric}")
        fig, ax = plt.subplots(figsize=(10, 6))
        ax.text(0.5, 0.5, f"No data found for metric: {metric}", 
                ha="center", va="center", fontsize=12)
        return fig
    
    # Convert to DataFrame
    df = pd.DataFrame(data)
    
    # Pivot data for grouped bar chart
    pivot_df = df.pivot(index="test", columns="commit", values="value")
    
    # Create plot
    fig, ax = plt.subplots(figsize=(12, 7))
    
    # Plot grouped bar chart
    bar_width = 0.35
    x = np.arange(len(pivot_df.index))
    
    for i, commit in enumerate(pivot_df.columns):
        offset = (i - len(pivot_df.columns)/2 + 0.5) * bar_width
        ax.bar(x + offset, pivot_df[commit], bar_width, label=commit)
    
    # Add labels and title
    ax.set_xlabel("Test")
    ax.set_ylabel(f"{metric.replace('_', ' ').title()}")
    ax.set_title(f"NativeLink Performance Comparison: {metric.replace('_', ' ').title()}")
    
    # Set x-axis ticks
    ax.set_xticks(x)
    ax.set_xticklabels(pivot_df.index)
    
    # Add legend
    ax.legend(title="Commit")
    
    # Add grid
    ax.grid(True, linestyle="--", alpha=0.7, axis="y")
    
    # Add percentage change annotations
    if len(pivot_df.columns) == 2:
        old_commit, new_commit = pivot_df.columns
        for i, test in enumerate(pivot_df.index):
            old_val = pivot_df.loc[test, old_commit]
            new_val = pivot_df.loc[test, new_commit]
            pct_change = ((new_val - old_val) / old_val) * 100
            
            # Position the text above the higher bar
            y_pos = max(old_val, new_val) + 0.05 * max(pivot_df.values.max(), 1)
            
            # Color based on improvement or regression
            color = "green" if pct_change <= 0 else "red"
            
            ax.annotate(f"{pct_change:.1f}%", 
                        xy=(i, y_pos),
                        ha="center",
                        color=color,
                        fontweight="bold")
    
    # Tight layout
    fig.tight_layout()
    
    return fig


def main():
    parser = argparse.ArgumentParser(description="NativeLink Benchmark Visualization Tool")
    parser.add_argument("--input-dir", default="./benchmark_results", 
                        help="Directory containing benchmark results")
    parser.add_argument("--output-dir", default="./benchmark_viz", 
                        help="Directory to save visualization outputs")
    parser.add_argument("--project", help="Filter results by project name")
    parser.add_argument("--metric", default="build_time", 
                        help="Metric to visualize (default: build_time)")
    parser.add_argument("--last-n-commits", type=int, default=10, 
                        help="Show only the last N commits")
    
    args = parser.parse_args()
    
    # Create output directory if it doesn't exist
    os.makedirs(args.output_dir, exist_ok=True)
    
    # Load benchmark results
    results = load_benchmark_results(args.input_dir, args.project)
    
    if not results:
        print(f"No benchmark results found in {args.input_dir}")
        return
    
    print(f"Loaded {len(results)} benchmark results")
    
    # Create time series chart
    time_series_fig = create_time_series_chart(
        results, 
        metric=args.metric,
        last_n=args.last_n_commits,
    )
    
    # Save time series chart
    project_prefix = f"{args.project}_" if args.project else ""
    time_series_path = os.path.join(
        args.output_dir, 
        f"{project_prefix}time_series_{args.metric}.png"
    )
    time_series_fig.savefig(time_series_path)
    print(f"Time series chart saved to {time_series_path}")
    
    # Create comparison chart (last 2 commits)
    comparison_fig = create_comparison_chart(
        results, 
        metric=args.metric,
        last_n=min(2, len(results)),
    )
    
    # Save comparison chart
    comparison_path = os.path.join(
        args.output_dir, 
        f"{project_prefix}comparison_{args.metric}.png"
    )
    comparison_fig.savefig(comparison_path)
    print(f"Comparison chart saved to {comparison_path}")


if __name__ == "__main__":
    main()