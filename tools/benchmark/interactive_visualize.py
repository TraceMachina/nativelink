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
Interactive NativeLink Benchmark Visualization Tool

This script generates interactive visualizations from benchmark results to help track
performance trends over time and identify regressions between commits.

Features:
- Interactive time series charts with zoom, pan, and hover capabilities
- Annotations for significant changes and events
- Comparison charts between commits
- Dashboard-style HTML report with interactive elements
- Support for all metrics collected by enhanced_benchmark.py

Usage:
  python interactive_visualize.py --input-dir=<benchmark_results_dir> [options]

Options:
  --input-dir       Directory containing benchmark results (default: ./benchmark_results)
  --output-dir      Directory to save visualization outputs (default: ./benchmark_viz)
  --project         Filter results by project name (optional)
  --metric          Metric to visualize (default: build_time)
  --last-n-commits  Show only the last N commits (default: 10)
  --html-report     Generate HTML dashboard report (default: True)
  --compare-commits Compare specific commits (comma-separated hashes)
  --annotations     Path to JSON file with annotations for significant changes
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
    import plotly.express as px
    import plotly.graph_objects as go
    from plotly.subplots import make_subplots
    import plotly.io as pio
except ImportError:
    print("Please install required packages: pip install matplotlib numpy pandas seaborn plotly")
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


def load_annotations(annotations_file: Optional[str]) -> Dict[str, List[Dict]]:
    """Load annotations for significant changes from a JSON file."""
    if not annotations_file or not os.path.exists(annotations_file):
        return {}

    try:
        with open(annotations_file, "r") as f:
            return json.load(f)
    except (json.JSONDecodeError, IOError) as e:
        print(f"Warning: Could not load annotations file: {e}")
        return {}


def get_available_metrics(results: List[Dict]) -> Dict[str, Set[str]]:
    """Get all available metrics from the results."""
    metrics_by_test = {}

    for result in results:
        for test_name, metrics in result.get("metrics", {}).items():
            if test_name not in metrics_by_test:
                metrics_by_test[test_name] = set()
            metrics_by_test[test_name].update(metrics.keys())

    return metrics_by_test


def create_interactive_time_series(
    results: List[Dict],
    metric: str = "build_time",
    test_names: Optional[List[str]] = None,
    last_n: int = 10,
    title_prefix: str = "NativeLink Performance",
    annotations: Optional[Dict] = None,
) -> go.Figure:
    """Create an interactive time series chart of benchmark results."""
    # Prepare data
    data = []

    # Use all test names if none specified
    if not test_names and results:
        test_names = list(results[0].get("metrics", {}).keys())

    for result in results[-last_n:]:
        commit = result.get("commit", "unknown")[:7]  # Short commit hash
        timestamp = result.get("timestamp", "")
        commit_date = result.get("parsed_timestamp", datetime.min)

        for test_name in test_names or []:
            if test_name in result.get("metrics", {}):
                test_metrics = result["metrics"][test_name]
                if metric in test_metrics:
                    data.append(
                        {
                            "commit": commit,
                            "timestamp": timestamp,
                            "date": commit_date,
                            "test": test_name,
                            "value": test_metrics[metric],
                        }
                    )

    if not data:
        print(f"No data found for metric: {metric}")
        fig = go.Figure()
        fig.add_annotation(
            text=f"No data found for metric: {metric}",
            xref="paper",
            yref="paper",
            x=0.5,
            y=0.5,
            showarrow=False,
        )
        return fig

    # Convert to DataFrame for easier plotting
    df = pd.DataFrame(data)

    # Create interactive plot
    fig = go.Figure()

    for test_name, group in df.groupby("test"):
        fig.add_trace(
            go.Scatter(
                x=group["commit"],
                y=group["value"],
                mode="lines+markers",
                name=test_name,
                hovertemplate="<b>%{x}</b><br>" + f"{metric}: %{{y:.2f}}<br>" + "<extra></extra>",
            )
        )

    # Add annotations if provided
    if annotations and metric in annotations:
        for annotation in annotations[metric]:
            commit = annotation.get("commit", "")
            if commit in df["commit"].values:
                # Find the y-value for this commit (use the first test if multiple)
                y_value = df[df["commit"] == commit]["value"].iloc[0]

                fig.add_annotation(
                    x=commit,
                    y=y_value,
                    text=annotation.get("text", ""),
                    showarrow=True,
                    arrowhead=1,
                    arrowsize=1,
                    arrowwidth=2,
                    arrowcolor="#636363",
                    ax=0,
                    ay=-40,
                )

    # Add labels and title
    metric_name = metric.replace("_", " ").title()
    fig.update_layout(
        title=f"{title_prefix}: {metric_name} Over Time",
        xaxis_title="Commit",
        yaxis_title=metric_name,
        hovermode="closest",
        legend=dict(orientation="h", yanchor="bottom", y=1.02, xanchor="right", x=1),
        margin=dict(l=50, r=50, t=80, b=50),
        # Add instructions for interactive features
        annotations=[
            dict(
                text="Click and drag to zoom; shift + click and drag to scroll after zooming; hover over an annotation to see details",
                showarrow=False,
                xref="paper",
                yref="paper",
                x=0,
                y=1.1,
                xanchor="left",
                yanchor="top",
                font=dict(size=10, color="gray"),
            )
        ],
    )

    # Enable zoom and pan
    fig.update_xaxes(tickangle=45)

    return fig


def create_interactive_comparison_chart(
    results: List[Dict],
    metric: str = "build_time",
    test_names: Optional[List[str]] = None,
    last_n: int = 2,
    specific_commits: Optional[List[str]] = None,
) -> go.Figure:
    """Create an interactive bar chart comparing benchmark results."""
    # Filter results by specific commits if provided
    if specific_commits:
        filtered_results = [
            r
            for r in results
            if r.get("commit", "")[:7] in specific_commits
            or any(r.get("commit", "").startswith(c) for c in specific_commits)
        ]
        if len(filtered_results) >= 2:
            results = filtered_results

    # Use only the last N results
    recent_results = results[-last_n:]

    if len(recent_results) < 2:
        print("Need at least 2 results for comparison")
        fig = go.Figure()
        fig.add_annotation(
            text="Need at least 2 results for comparison",
            xref="paper",
            yref="paper",
            x=0.5,
            y=0.5,
            showarrow=False,
        )
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
                    data.append(
                        {
                            "commit": commit,
                            "test": test_name,
                            "value": test_metrics[metric],
                        }
                    )

    if not data:
        print(f"No data found for metric: {metric}")
        fig = go.Figure()
        fig.add_annotation(
            text=f"No data found for metric: {metric}",
            xref="paper",
            yref="paper",
            x=0.5,
            y=0.5,
            showarrow=False,
        )
        return fig

    # Convert to DataFrame
    df = pd.DataFrame(data)

    # Create interactive plot
    fig = go.Figure()

    # Add bars for each commit
    for commit in df["commit"].unique():
        commit_data = df[df["commit"] == commit]
        fig.add_trace(
            go.Bar(
                x=commit_data["test"],
                y=commit_data["value"],
                name=commit,
                hovertemplate="<b>%{x}</b><br>"
                + f"{metric}: %{{y:.2f}}<br>"
                + "<extra>%{fullData.name}</extra>",
            )
        )

    # Add percentage change annotations if there are exactly 2 commits
    if len(df["commit"].unique()) == 2:
        old_commit, new_commit = sorted(df["commit"].unique())

        for test in df["test"].unique():
            old_val = df[(df["commit"] == old_commit) & (df["test"] == test)]["value"].iloc[0]
            new_val = df[(df["commit"] == new_commit) & (df["test"] == test)]["value"].iloc[0]
            pct_change = ((new_val - old_val) / old_val) * 100 if old_val != 0 else 0

            # Determine if this is an improvement or regression
            is_time_metric = any(term in metric for term in ["time", "duration", "latency"])
            is_improvement = (pct_change <= 0) if is_time_metric else (pct_change >= 0)
            color = "green" if is_improvement else "red"

            fig.add_annotation(
                x=test,
                y=max(old_val, new_val),
                text=f"{pct_change:.1f}%",
                showarrow=False,
                font=dict(color=color, size=12, family="Arial Black"),
                yshift=10,
            )

    # Add labels and title
    metric_name = metric.replace("_", " ").title()
    fig.update_layout(
        title=f"NativeLink Performance Comparison: {metric_name}",
        xaxis_title="Test",
        yaxis_title=metric_name,
        barmode="group",
        legend=dict(orientation="h", yanchor="bottom", y=1.02, xanchor="right", x=1),
        margin=dict(l=50, r=50, t=80, b=50),
    )

    return fig


def create_interactive_heatmap(
    results: List[Dict],
    test_names: Optional[List[str]] = None,
    metrics: Optional[List[str]] = None,
    last_n: int = 5,
) -> go.Figure:
    """Create an interactive heatmap showing performance changes across commits."""
    # Use only the last N results
    recent_results = results[-last_n:]

    if len(recent_results) < 2:
        print("Need at least 2 results for heatmap")
        fig = go.Figure()
        fig.add_annotation(
            text="Need at least 2 results for heatmap",
            xref="paper",
            yref="paper",
            x=0.5,
            y=0.5,
            showarrow=False,
        )
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
    perf_metrics = [
        m for m in metrics if any(term in m for term in ["time", "memory", "cpu", "bytes", "cache"])
    ]

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
                        data.append(
                            {
                                "commit": commit,
                                "test": test_name,
                                "metric": metric,
                                "value": result["metrics"][test_name][metric],
                            }
                        )

    if not data:
        print("No data found for heatmap")
        fig = go.Figure()
        fig.add_annotation(
            text="No data found for heatmap",
            xref="paper",
            yref="paper",
            x=0.5,
            y=0.5,
            showarrow=False,
        )
        return fig

    # Convert to DataFrame
    df = pd.DataFrame(data)

    # Calculate percentage changes relative to first commit
    pivot_df = df.pivot_table(index=["test", "metric"], columns="commit", values="value")
    first_commit = pivot_df.columns[0]

    # Calculate percentage changes
    pct_change_df = pivot_df.copy()
    for col in pivot_df.columns[1:]:
        pct_change_df[col] = (
            (pivot_df[col] - pivot_df[first_commit]) / pivot_df[first_commit]
        ) * 100

    pct_change_df[first_commit] = 0  # First commit has 0% change

    # Reset index for plotting
    pct_change_df = pct_change_df.reset_index()

    # Melt the DataFrame for heatmap
    melted_df = pd.melt(
        pct_change_df,
        id_vars=["test", "metric"],
        value_vars=pct_change_df.columns[2:],
        var_name="commit",
        value_name="pct_change",
    )

    # Create labels for heatmap cells
    melted_df["text"] = melted_df["pct_change"].apply(lambda x: f"{x:.1f}%")

    # Create combined index for y-axis
    melted_df["test_metric"] = melted_df["test"] + " - " + melted_df["metric"]

    # Create interactive heatmap
    fig = px.imshow(
        melted_df.pivot(index="test_metric", columns="commit", values="pct_change"),
        labels=dict(x="Commit", y="Test - Metric", color="% Change"),
        x=melted_df["commit"].unique(),
        y=melted_df["test_metric"].unique(),
        color_continuous_scale=["green", "white", "red"],
        color_continuous_midpoint=0,
        text_auto=".1f",
        aspect="auto",
    )

    fig.update_layout(
        title="Performance Changes Across Commits (%)",
        margin=dict(l=50, r=50, t=80, b=50),
    )

    return fig


def generate_interactive_html_report(
    results: List[Dict],
    output_dir: str,
    project: Optional[str] = None,
    last_n: int = 10,
    annotations: Optional[Dict] = None,
) -> str:
    """Generate an interactive HTML dashboard report of benchmark results."""
    if not results:
        return ""

    # Create output directory if it doesn't exist
    os.makedirs(output_dir, exist_ok=True)

    # Get available metrics
    metrics_by_test = get_available_metrics(results)

    # Generate charts for each test and metric
    chart_divs = []

    # Time series charts
    for test_name, metrics in metrics_by_test.items():
        for metric in metrics:
            # Skip metrics that are likely not useful for time series
            if metric in ["peak_memory"]:
                continue

            fig = create_interactive_time_series(
                results,
                metric=metric,
                test_names=[test_name],
                last_n=last_n,
                annotations=annotations,
            )

            # Generate div for this chart
            chart_div = f"<div id='{test_name}_{metric}_chart' class='chart-container'>"
            chart_div += (
                f"<div class='chart-title'>{test_name} - {metric.replace('_', ' ').title()}</div>"
            )
            chart_div += pio.to_html(fig, full_html=False, include_plotlyjs="cdn")
            chart_div += "</div>"

            chart_divs.append(chart_div)

    # Comparison charts for build_time
    for test_name in metrics_by_test.keys():
        if "build_time" in metrics_by_test[test_name]:
            fig = create_interactive_comparison_chart(
                results,
                metric="build_time",
                test_names=[test_name],
                last_n=min(5, len(results)),
            )

            # Generate div for this chart
            chart_div = f"<div id='{test_name}_build_time_comparison' class='chart-container'>"
            chart_div += f"<div class='chart-title'>{test_name} - Build Time Comparison</div>"
            chart_div += pio.to_html(fig, full_html=False, include_plotlyjs="cdn")
            chart_div += "</div>"

            chart_divs.append(chart_div)

    # Heatmap
    fig = create_interactive_heatmap(
        results,
        last_n=min(5, len(results)),
    )

    # Generate div for heatmap
    heatmap_div = "<div id='performance_heatmap' class='chart-container'>"
    heatmap_div += "<div class='chart-title'>Performance Changes Heatmap</div>"
    heatmap_div += pio.to_html(fig, full_html=False, include_plotlyjs="cdn")
    heatmap_div += "</div>"

    chart_divs.append(heatmap_div)

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
        "        .summary { background-color: #f5f5f5; padding: 15px; border-radius: 5px; margin-bottom: 20px; }",
        "        table { border-collapse: collapse; width: 100%; margin-bottom: 20px; }",
        "        th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }",
        "        th { background-color: #f2f2f2; }",
        "        tr:nth-child(even) { background-color: #f9f9f9; }",
        "        .improvement { color: green; }",
        "        .regression { color: red; }",
        "        .nav { position: sticky; top: 0; background: white; padding: 10px; border-bottom: 1px solid #ddd; z-index: 100; }",
        "        .nav-links { display: flex; gap: 20px; }",
        "        .nav-link { cursor: pointer; color: #0066cc; }",
        "        .nav-link:hover { text-decoration: underline; }",
        "    </style>",
        "</head>",
        "<body>",
        f"    <h1>NativeLink Benchmark Results{' for ' + project if project else ''}</h1>",
        "    <div class='summary'>",
        f"        <p><strong>Generated:</strong> {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}</p>",
        f"        <p><strong>Number of Results:</strong> {len(results)}</p>",
        f"        <p><strong>Latest Commit:</strong> {results[-1]['commit'] if results else 'N/A'}</p>",
        "    </div>",
    ]

    # Add summary table
    if len(results) >= 2:
        latest = results[-1]
        previous = results[-2]

        html_content.extend(
            [
                "    <h2>Performance Summary</h2>",
                "    <table>",
                "        <tr>",
                "            <th>Test</th>",
                "            <th>Metric</th>",
                "            <th>Current Value</th>",
                "            <th>Previous Value</th>",
                "            <th>Change</th>",
                "        </tr>",
            ]
        )

        for test_name, metrics in latest.get("metrics", {}).items():
            for metric, value in metrics.items():
                if (
                    test_name in previous.get("metrics", {})
                    and metric in previous["metrics"][test_name]
                ):
                    prev_value = previous["metrics"][test_name][metric]
                    change = ((value - prev_value) / prev_value) * 100 if prev_value != 0 else 0

                    # Determine if this is an improvement or regression
                    is_time_metric = any(term in metric for term in ["time", "duration", "latency"])
                    is_improvement = (change <= 0) if is_time_metric else (change >= 0)
                    change_class = "improvement" if is_improvement else "regression"

                    html_content.append(f"        <tr>")
                    html_content.append(f"            <td>{test_name}</td>")
                    html_content.append(f"            <td>{metric.replace('_', ' ').title()}</td>")
                    html_content.append(f"            <td>{value:.2f}</td>")
                    html_content.append(f"            <td>{prev_value:.2f}</td>")
                    html_content.append(
                        f"            <td class='{change_class}'>{change:.2f}%</td>"
                    )
                    html_content.append(f"        </tr>")

        html_content.append("    </table>")

    # Add navigation
    html_content.append("    <div class='nav'>")
    html_content.append("        <div class='nav-links'>")
    html_content.append(
        "            <span class='nav-link' onclick='document.getElementById(\"performance_summary\").scrollIntoView({behavior: \"smooth\"})'>Summary</span>"
    )
    html_content.append(
        "            <span class='nav-link' onclick='document.getElementById(\"performance_charts\").scrollIntoView({behavior: \"smooth\"})'>Charts</span>"
    )
    html_content.append(
        "            <span class='nav-link' onclick='document.getElementById(\"performance_heatmap\").scrollIntoView({behavior: \"smooth\"})'>Heatmap</span>"
    )
    html_content.append("        </div>")
    html_content.append("    </div>")

    # Add charts
    html_content.append("    <h2 id='performance_charts'>Performance Charts</h2>")
    html_content.extend(chart_divs)

    html_content.extend(
        [
            "    <script>",
            "        // Add any custom JavaScript here",
            "    </script>",
            "</body>",
            "</html>",
        ]
    )

    # Write HTML file
    project_prefix = f"{project}_" if project else ""
    html_path = os.path.join(output_dir, f"{project_prefix}interactive_benchmark_report.html")

    with open(html_path, "w") as f:
        f.write("\n".join(html_content))

    return html_path


def create_sample_annotations_file(output_path: str) -> None:
    """Create a sample annotations file to demonstrate the format."""
    sample_annotations = {
        "build_time": [
            {
                "commit": "abc1234",
                "text": "Added parallel execution",
                "description": "Implemented parallel execution for build tasks",
            },
            {
                "commit": "def5678",
                "text": "Cache optimization",
                "description": "Improved cache hit rate with better fingerprinting",
            },
        ],
        "memory_usage": [
            {
                "commit": "ghi9012",
                "text": "Memory leak fixed",
                "description": "Fixed memory leak in worker process",
            }
        ],
    }

    with open(output_path, "w") as f:
        json.dump(sample_annotations, f, indent=2)

    print(f"Sample annotations file created at {output_path}")


def main():
    parser = argparse.ArgumentParser(
        description="Interactive NativeLink Benchmark Visualization Tool"
    )
    parser.add_argument(
        "--input-dir", default="./benchmark_results", help="Directory containing benchmark results"
    )
    parser.add_argument(
        "--output-dir", default="./benchmark_viz", help="Directory to save visualization outputs"
    )
    parser.add_argument("--project", help="Filter results by project name")
    parser.add_argument(
        "--metric", default="build_time", help="Metric to visualize (default: build_time)"
    )
    parser.add_argument(
        "--last-n-commits", type=int, default=10, help="Show only the last N commits"
    )
    parser.add_argument(
        "--html-report", action="store_true", default=True, help="Generate HTML dashboard report"
    )
    parser.add_argument(
        "--compare-commits", help="Compare specific commits (comma-separated hashes)"
    )
    parser.add_argument(
        "--annotations", help="Path to JSON file with annotations for significant changes"
    )
    parser.add_argument(
        "--create-sample-annotations", action="store_true", help="Create a sample annotations file"
    )

    args = parser.parse_args()

    # Create sample annotations file if requested
    if args.create_sample_annotations:
        sample_path = os.path.join(args.output_dir, "sample_annotations.json")
        create_sample_annotations_file(sample_path)
        return

    # Create output directory if it doesn't exist
    os.makedirs(args.output_dir, exist_ok=True)

    # Load benchmark results
    results = load_benchmark_results(args.input_dir, args.project)

    if not results:
        print(f"No benchmark results found in {args.input_dir}")
        return

    print(f"Loaded {len(results)} benchmark results")

    # Load annotations if provided
    annotations = load_annotations(args.annotations)

    # Parse specific commits if provided
    specific_commits = None
    if args.compare_commits:
        specific_commits = [c.strip() for c in args.compare_commits.split(",")]

    # Create interactive time series chart
    time_series_fig = create_interactive_time_series(
        results, metric=args.metric, last_n=args.last_n_commits, annotations=annotations
    )

    # Save time series chart as HTML
    project_prefix = f"{args.project}_" if args.project else ""
    time_series_path = os.path.join(
        args.output_dir, f"{project_prefix}time_series_{args.metric}.html"
    )
    time_series_fig.write_html(time_series_path)
    print(f"Interactive time series chart saved to {time_series_path}")

    # Create comparison chart
    comparison_fig = create_interactive_comparison_chart(
        results,
        metric=args.metric,
        last_n=min(5, len(results)),
        specific_commits=specific_commits,
    )

    # Save comparison chart as HTML
    comparison_path = os.path.join(
        args.output_dir, f"{project_prefix}comparison_{args.metric}.html"
    )
    comparison_fig.write_html(comparison_path)
    print(f"Interactive comparison chart saved to {comparison_path}")

    # Create heatmap
    heatmap_fig = create_interactive_heatmap(
        results,
        last_n=min(5, len(results)),
    )

    # Save heatmap as HTML
    heatmap_path = os.path.join(args.output_dir, f"{project_prefix}performance_heatmap.html")
    heatmap_fig.write_html(heatmap_path)
    print(f"Interactive performance heatmap saved to {heatmap_path}")

    # Generate HTML report if requested
    if args.html_report:
        html_path = generate_interactive_html_report(
            results,
            args.output_dir,
            project=args.project,
            last_n=args.last_n_commits,
            annotations=annotations,
        )
        if html_path:
            print(f"Interactive HTML report generated at {html_path}")


if __name__ == "__main__":
    main()
