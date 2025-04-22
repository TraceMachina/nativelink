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
Enhanced NativeLink Benchmark Visualization Tool

This script generates advanced visualizations from benchmark results to help track
performance trends over time and identify regressions between commits.

Features:
- Time series charts for all metrics
- Comparison charts between commits
- Heatmaps for identifying performance hotspots
- Dashboard-style HTML report generation
- Support for all metrics collected by enhanced_benchmark.py

Usage:
  python enhanced_visualize.py --input-dir=<benchmark_results_dir> [options]

Options:
  --input-dir       Directory containing benchmark results (default: ./benchmark_results)
  --output-dir      Directory to save visualization outputs (default: ./benchmark_viz)
  --project         Filter results by project name (optional)
  --metric          Metric to visualize (default: build_time)
  --last-n-commits  Show only the last N commits (default: 10)
  --html-report     Generate HTML dashboard report (default: True)
  --compare-commits Compare specific commits (comma-separated hashes)
"""

import argparse
import json
import os
import re
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional, Tuple, Any, Set

try:
    import matplotlib.pyplot as plt
    import numpy as np
    import pandas as pd
    import seaborn as sns
    from matplotlib.colors import LinearSegmentedColormap
except ImportError:
    print("Please install required packages: pip install matplotlib numpy pandas seaborn")
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


def get_available_metrics(results: List[Dict]) -> Dict[str, Set[str]]:
    """Get all available metrics from the results."""
    metrics_by_test = {}
    
    for result in results:
        for test_name, metrics in result.get("metrics", {}).items():
            if test_name not in metrics_by_test:
                metrics_by_test[test_name] = set()
            metrics_by_test[test_name].update(metrics.keys())
    
    return metrics_by_test


def create_time_series_chart(
    results: List[Dict],
    metric: str = "build_time",
    test_names: Optional[List[str]] = None,
    last_n: int = 10,
    title_prefix: str = "NativeLink Performance",
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
        ax.plot(group["commit"], group["value"], marker="o", label=test_name, linewidth=2)
    
    # Add labels and title
    metric_name = metric.replace("_", " ").title()
    ax.set_xlabel("Commit", fontsize=12)
    ax.set_ylabel(metric_name, fontsize=12)
    ax.set_title(f"{title_prefix}: {metric_name} Over Time", fontsize=14)
    
    # Rotate x-axis labels for better readability
    plt.xticks(rotation=45, ha="right", fontsize=10)
    plt.yticks(fontsize=10)
    
    # Add legend
    ax.legend(fontsize=10)
    
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
    specific_commits: Optional[List[str]] = None,
) -> plt.Figure:
    """Create a bar chart comparing benchmark results."""
    # Filter results by specific commits if provided
    if specific_commits:
        filtered_results = [r for r in results if r.get("commit", "")[:7] in specific_commits or 
                           any(r.get("commit", "").startswith(c) for c in specific_commits)]
        if len(filtered_results) >= 2:
            results = filtered_results
    
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
    
    # Use a color palette
    colors = plt.cm.tab10(np.linspace(0, 1, len(pivot_df.columns)))
    
    for i, (commit, color) in enumerate(zip(pivot_df.columns, colors)):
        offset = (i - len(pivot_df.columns)/2 + 0.5) * bar_width
        ax.bar(x + offset, pivot_df[commit], bar_width, label=commit, color=color, alpha=0.8)
    
    # Add labels and title
    metric_name = metric.replace("_", " ").title()
    ax.set_xlabel("Test", fontsize=12)
    ax.set_ylabel(metric_name, fontsize=12)
    ax.set_title(f"NativeLink Performance Comparison: {metric_name}", fontsize=14)
    
    # Set x-axis ticks
    ax.set_xticks(x)
    ax.set_xticklabels(pivot_df.index, fontsize=10)
    
    # Add legend
    ax.legend(title="Commit", fontsize=10)
    
    # Add grid
    ax.grid(True, linestyle="--", alpha=0.7, axis="y")
    
    # Add percentage change annotations
    if len(pivot_df.columns) == 2:
        old_commit, new_commit = pivot_df.columns
        for i, test in enumerate(pivot_df.index):
            old_val = pivot_df.loc[test, old_commit]
            new_val = pivot_df.loc[test, new_commit]
            pct_change = ((new_val - old_val) / old_val) * 100 if old_val != 0 else 0
            
            # Position the text above the higher bar
            y_pos = max(old_val, new_val) + 0.05 * max(pivot_df.values.max(), 1)
            
            # Color based on improvement or regression
            # For build_time and similar metrics, lower is better
            is_time_metric = any(term in metric for term in ["time", "duration", "latency"])
            is_improvement = (pct_change <= 0) if is_time_metric else (pct_change >= 0)
            color = "green" if is_improvement else "red"
            
            ax.annotate(f"{pct_change:.1f}%", 
                        xy=(i, y_pos),
                        ha="center",
                        color=color,
                        fontweight="bold")
    
    # Tight layout
    fig.tight_layout()
    
    return fig


def create_heatmap(
    results: List[Dict],
    test_names: Optional[List[str]] = None,
    metrics: Optional[List[str]] = None,
    last_n: int = 5,
) -> plt.Figure:
    """Create a heatmap showing performance changes across commits."""
    # Use only the last N results
    recent_results = results[-last_n:]
    
    if len(recent_results) < 2:
        print("Need at least 2 results for heatmap")
        fig, ax = plt.subplots(figsize=(10, 6))
        ax.text(0.5, 0.5, "Need at least 2 results for heatmap", 
                ha="center", va="center", fontsize=12)
        return fig
    
    # Use all test names if none specified
    if not test_names and recent_results:
        test_names = list(recent_results[0].get("metrics", {}).keys())
    
    # Get all available metrics if none specified
    if not metrics:
        metrics_set = set()
        for result in recent_results:
            for test_name in test_names:
                if test_name in result.get("metrics", {}):
                    metrics_set.update(result["metrics"][test_name].keys())
        metrics = list(metrics_set)
    
    # Filter metrics to common performance metrics
    perf_metrics = [m for m in metrics if any(term in m for term in 
                   ["time", "memory", "cpu", "bytes", "cache"])]
    
    if not perf_metrics:
        perf_metrics = metrics[:5]  # Use first 5 metrics if no performance metrics found
    
    # Prepare data for heatmap
    data = []
    
    for result in recent_results:
        commit = result.get("commit", "unknown")[:7]  # Short commit hash
        
        for test_name in test_names:
            if test_name in result.get("metrics", {}):
                for metric in perf_metrics:
                    if metric in result["metrics"][test_name]:
                        data.append({
                            "commit": commit,
                            "test": test_name,
                            "metric": metric,
                            "value": result["metrics"][test_name][metric],
                        })
    
    if not data:
        print("No data found for heatmap")
        fig, ax = plt.subplots(figsize=(10, 6))
        ax.text(0.5, 0.5, "No data found for heatmap", 
                ha="center", va="center", fontsize=12)
        return fig
    
    # Convert to DataFrame
    df = pd.DataFrame(data)
    
    # Pivot data for heatmap
    # We'll create a multi-index heatmap with tests and metrics
    pivot_df = df.pivot_table(
        index=["test", "metric"], 
        columns="commit", 
        values="value"
    )
    
    # Calculate percentage changes relative to first commit
    first_commit = pivot_df.columns[0]
    pct_change_df = pivot_df.copy()
    
    for col in pivot_df.columns[1:]:
        pct_change_df[col] = ((pivot_df[col] - pivot_df[first_commit]) / pivot_df[first_commit]) * 100
    
    pct_change_df[first_commit] = 0  # First commit has 0% change
    
    # Create plot
    fig, ax = plt.subplots(figsize=(12, len(pct_change_df) * 0.4 + 2))
    
    # Custom colormap: green for improvements, red for regressions
    # For time metrics, negative change (green) is good
    # For other metrics, it depends on the metric
    cmap = LinearSegmentedColormap.from_list(
        "green_white_red", 
        [(0, "green"), (0.5, "white"), (1, "red")]
    )
    
    # Plot heatmap
    sns.heatmap(
        pct_change_df,
        ax=ax,
        cmap=cmap,
        center=0,
        annot=True,
        fmt=".1f",
        linewidths=0.5,
        cbar_kws={"label": "% Change"}
    )
    
    # Add labels and title
    ax.set_title("Performance Changes Across Commits (%)", fontsize=14)
    
    # Adjust y-axis labels to show both test and metric
    ax.set_yticklabels([f"{idx[0]}\n{idx[1]}" for idx in pct_change_df.index], fontsize=10)
    
    # Rotate x-axis labels for better readability
    plt.xticks(rotation=45, ha="right", fontsize=10)
    
    # Tight layout
    fig.tight_layout()
    
    return fig


def generate_html_report(
    results: List[Dict],
    output_dir: str,
    project: Optional[str] = None,
    last_n: int = 10,
) -> str:
    """Generate an HTML dashboard report of benchmark results."""
    if not results:
        return ""
    
    # Create output directory if it doesn't exist
    os.makedirs(output_dir, exist_ok=True)
    
    # Get available metrics
    metrics_by_test = get_available_metrics(results)
    
    # Generate charts for each test and metric
    chart_paths = []
    
    # Time series charts
    for test_name, metrics in metrics_by_test.items():
        for metric in metrics:
            # Skip metrics that are likely not useful for time series
            if metric in ["peak_memory"]:
                continue
                
            fig = create_time_series_chart(
                results,
                metric=metric,
                test_names=[test_name],
                last_n=last_n,
            )
            
            # Save chart
            project_prefix = f"{project}_" if project else ""
            chart_filename = f"{project_prefix}{test_name}_{metric}_time_series.png"
            chart_path = os.path.join(output_dir, chart_filename)
            fig.savefig(chart_path)
            plt.close(fig)
            
            chart_paths.append((chart_path, f"{test_name} - {metric.replace('_', ' ').title()}"))
    
    # Comparison charts for build_time
    for test_name in metrics_by_test.keys():
        if "build_time" in metrics_by_test[test_name]:
            fig = create_comparison_chart(
                results,
                metric="build_time",
                test_names=[test_name],
                last_n=min(5, len(results)),
            )
            
            # Save chart
            project_prefix = f"{project}_" if project else ""
            chart_filename = f"{project_prefix}{test_name}_build_time_comparison.png"
            chart_path = os.path.join(output_dir, chart_filename)
            fig.savefig(chart_path)
            plt.close(fig)
            
            chart_paths.append((chart_path, f"{test_name} - Build Time Comparison"))
    
    # Heatmap
    fig = create_heatmap(
        results,
        last_n=min(5, len(results)),
    )
    
    # Save heatmap
    project_prefix = f"{project}_" if project else ""
    heatmap_filename = f"{project_prefix}performance_heatmap.png"
    heatmap_path = os.path.join(output_dir, heatmap_filename)
    fig.savefig(heatmap_path)
    plt.close(fig)
    
    chart_paths.append((heatmap_path, "Performance Changes Heatmap"))
    
    # Generate HTML
    html_content = [
        "<!DOCTYPE html>",
        "<html>",
        "<head>",
        "    <title>NativeLink Benchmark Results</title>",
        "    <style>",
        "        body { font-family: Arial, sans-serif; margin: 20px; }",
        "        h1 { color: #333; }",
        "        .chart-container { margin-bottom: 30px; }",
        "        .chart-title { font-size: 18px; font-weight: bold; margin-bottom: 10px; }",
        "        .chart { max-width: 100%; box-shadow: 0 0 10px rgba(0,0,0,0.1); }",
        "        .summary { background-color: #f5f5f5; padding: 15px; border-radius: 5px; margin-bottom: 20px; }",
        "        table { border-collapse: collapse; width: 100%; margin-bottom: 20px; }",
        "        th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }",
        "        th { background-color: #f2f2f2; }",
        "        tr:nth-child(even) { background-color: #f9f9f9; }",
        "    </style>",
        "</head>",
        "<body>",
        f"    <h1>NativeLink Benchmark Results{' for ' + project if project else ''}</h1>",
        "    <div class='summary'>",
        f"        <p><strong>Generated:</strong> {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}</p>",
        f"        <p><strong>Number of Results:</strong> {len(results)}</p>",
        f"        <p><strong>Latest Commit:</strong> {results[-1]['commit'] if results else 'N/A'}</p>",
        "    </div>",
        "    <h2>Performance Summary</h2>",
    ]
    
    # Add summary table
    if len(results) >= 2:
        latest = results[-1]
        previous = results[-2]
        
        html_content.extend([
            "    <table>",
            "        <tr>",
            "            <th>Test</th>",
            "            <th>Metric</th>",
            "            <th>Current Value</th>",
            "            <th>Previous Value</th>",
            "            <th>Change</th>",
            "        </tr>",
        ])
        
        for test_name, metrics in latest.get("metrics", {}).items():
            for metric, value in metrics.items():
                if test_name in previous.get("metrics", {}) and metric in previous["metrics"][test_name]:
                    prev_value = previous["metrics"][test_name][metric]
                    change = ((value - prev_value) / prev_value) * 100 if prev_value != 0 else 0
                    
                    # Determine if this is an improvement or regression
                    is_time_metric = any(term in metric for term in ["time", "duration", "latency"])
                    is_improvement = (change <= 0) if is_time_metric else (change >= 0)
                    change_class = "improvement" if is_improvement else "regression"
                    change_color = "green" if is_improvement else "red"
                    
                    html_content.append(f"        <tr>")
                    html_content.append(f"            <td>{test_name}</td>")
                    html_content.append(f"            <td>{metric.replace('_', ' ').title()}</td>")
                    html_content.append(f"            <td>{value:.2f}</td>")
                    html_content.append(f"            <td>{prev_value:.2f}</td>")
                    html_content.append(f"            <td style='color: {change_color}'>{change:.2f}%</td>")
                    html_content.append(f"        </tr>")
        
        html_content.append("    </table>")
    
    # Add charts
    html_content.append("    <h2>Performance Charts</h2>")
    
    for chart_path, chart_title in chart_paths:
        # Get relative path for HTML
        rel_path = os.path.basename(chart_path)
        
        html_content.extend([
            "    <div class='chart-container'>",
            f"        <div class='chart-title'>{chart_title}</div>",
            f"        <img class='chart' src='{rel_path}' alt='{chart_title}'>",
            "    </div>",
        ])
    
    html_content.extend([
        "</body>",
        "</html>",
    ])
    
    # Write HTML file
    project_prefix = f"{project}_" if project else ""
    html_path = os.path.join(output_dir, f"{project_prefix}benchmark_report.html")
    
    with open(html_path, "w") as f:
        f.write("\n".join(html_content))
    
    return html_path


def main():
    parser = argparse.ArgumentParser(description="Enhanced NativeLink Benchmark Visualization Tool")
    parser.add_argument("--input-dir", default="./benchmark_results", 
                        help="Directory containing benchmark results")
    parser.add_argument("--output-dir", default="./benchmark_viz", 
                        help="Directory to save visualization outputs")
    parser.add_argument("--project", help="Filter results by project name")
    parser.add_argument("--metric", default="build_time", 
                        help="Metric to visualize (default: build_time)")
    parser.add_argument("--last-n-commits", type=int, default=10, 
                        help="Show only the last N commits")
    parser.add_argument("--html-report", action="store_true", default=True,
                        help="Generate HTML dashboard report")
    parser.add_argument("--compare-commits", 
                        help="Compare specific commits (comma-separated hashes)")
    
    args = parser.parse_args()
    
    # Create output directory if it doesn't exist
    os.makedirs(args.output_dir, exist_ok=True)
    
    # Load benchmark results
    results = load_benchmark_results(args.input_dir, args.project)
    
    if not results:
        print(f"No benchmark results found in {args.input_dir}")
        return
    
    print(f"Loaded {len(results)} benchmark results")
    
    # Parse specific commits if provided
    specific_commits = None
    if args.compare_commits:
        specific_commits = [c.strip() for c in args.compare_commits.split(",")]
    
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
    plt.close(time_series_fig)
    
    # Create comparison chart
    comparison_fig = create_comparison_chart(
        results, 
        metric=args.metric,
        last_n=min(5, len(results)),
        specific_commits=specific_commits,
    )
    
    # Save comparison chart
    comparison_path = os.path.join(
        args.output_dir, 
        f"{project_prefix}comparison_{args.metric}.png"
    )
    comparison_fig.savefig(comparison_path)
    print(f"Comparison chart saved to {comparison_path}")
    plt.close(comparison_fig)
    
    # Create heatmap
    heatmap_fig = create_heatmap(
        results,
        last_n=min(5, len(results)),
    )
    
    # Save heatmap
    heatmap_path = os.path.join(
        args.output_dir, 
        f"{project_prefix}performance_heatmap.png"
    )
    heatmap_fig.savefig(heatmap_path)
    print(f"Performance heatmap saved to {heatmap_path}")
    plt.close(heatmap_fig)
    
    # Generate HTML report if requested
    if args.html_report:
        html_path = generate_html_report(
            results,
            args.output_dir,
            project=args.project,
            last_n=args.last_n_commits,
        )
        if html_path:
            print(f"HTML report generated at {html_path}")


if __name__ == "__main__":
    main()