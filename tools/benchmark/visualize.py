#!/usr/bin/env python3

import argparse
import json
import os
import glob
from datetime import datetime
import matplotlib.pyplot as plt
import matplotlib.dates as mdates
import numpy as np
from pathlib import Path

def parse_args():
    parser = argparse.ArgumentParser(description="Generate visualizations for NativeLink benchmarks")
    parser.add_argument("--input-dir", required=True, help="Directory with benchmark results")
    parser.add_argument("--output-dir", required=True, help="Directory for visualization output")
    parser.add_argument("--commit", help="Current commit hash")
    parser.add_argument("--compare-to", help="Previous commit hash to compare against")
    parser.add_argument("--project", help="Project name filter")
    parser.add_argument("--days", type=int, default=30, help="Number of days to include in trend charts")
    return parser.parse_args()

def load_benchmark_data(input_dir, project=None, days=30):
    """Load benchmark data from JSON files."""
    data = {}
    
    # Get all JSON files in the input directory
    pattern = os.path.join(input_dir, "*.json")
    if project:
        pattern = os.path.join(input_dir, f"{project}_*.json")
    
    for file_path in glob.glob(pattern):
        try:
            with open(file_path, "r") as f:
                result = json.load(f)
            
            project_name = result.get("project", "unknown")
            if project_name not in data:
                data[project_name] = []
            
            data[project_name].append(result)
        except Exception as e:
            print(f"Error loading {file_path}: {e}")
    
    # Sort by timestamp
    for project_name in data:
        data[project_name].sort(key=lambda x: x.get("timestamp", ""))
        
        # Filter by date if requested
        if days > 0:
            cutoff = datetime.now().timestamp() - (days * 24 * 60 * 60)
            data[project_name] = [
                r for r in data[project_name]
                if datetime.strptime(r.get("timestamp", "19700101_000000"), "%Y%m%d_%H%M%S").timestamp() > cutoff
            ]
    
    return data

def generate_trend_charts(data, output_dir):
    """Generate trend charts for benchmark metrics."""
    os.makedirs(output_dir, exist_ok=True)
    
    for project_name, project_data in data.items():
        if not project_data:
            continue
        
        # Create project directory
        project_dir = os.path.join(output_dir, project_name)
        os.makedirs(project_dir, exist_ok=True)
        
        # Extract timestamps and commits
        timestamps = []
        commits = []
        
        for result in project_data:
            timestamp = result.get("timestamp", "")
            try:
                timestamps.append(datetime.strptime(timestamp, "%Y%m%d_%H%M%S"))
                commits.append(result.get("commit", "")[:7])
            except ValueError:
                print(f"Invalid timestamp format: {timestamp}")
        
        # Generate charts for each benchmark type and metric
        for benchmark_type in project_data[0].get("metrics", {}).keys():
            # Create benchmark type directory
            benchmark_dir = os.path.join(project_dir, benchmark_type)
            os.makedirs(benchmark_dir, exist_ok=True)
            
            # Get all metrics for this benchmark type
            metrics = set()
            for result in project_data:
                if benchmark_type in result.get("metrics", {}):
                    for key in result["metrics"][benchmark_type].keys():
                        if not isinstance(result["metrics"][benchmark_type][key], dict):
                            metrics.add(key)
            
            # Generate chart for each metric
            for metric in metrics:
                values = []
                
                for result in project_data:
                    if (benchmark_type in result.get("metrics", {}) and
                        metric in result["metrics"][benchmark_type]):
                        values.append(result["metrics"][benchmark_type][metric])
                    else:
                        values.append(None)
                
                # Skip if no values
                if not any(v is not None for v in values):
                    continue
                
                # Create chart
                plt.figure(figsize=(12, 6))
                
                # Plot values
                plt.plot(timestamps, values, "o-")
                
                # Add title and labels
                plt.title(f"{project_name} - {benchmark_type} - {metric}")
                plt.ylabel(metric)
                plt.grid(True)
                
                # Format x-axis
                plt.gca().xaxis.set_major_formatter(mdates.DateFormatter("%m-%d"))
                plt.gca().xaxis.set_major_locator(mdates.DayLocator(interval=2))
                plt.gcf().autofmt_xdate()
                
                # Add commit labels to points
                for i, (timestamp, value, commit) in enumerate(zip(timestamps, values, commits)):
                    if value is not None:
                        plt.annotate(commit, (timestamp, value),
                                    textcoords="offset points",
                                    xytext=(0, 10),
                                    ha="center")
                
                # Save chart
                chart_file = os.path.join(benchmark_dir, f"{metric}.png")
                plt.savefig(chart_file)
                plt.close()
                
                print(f"Generated chart: {chart_file}")

def generate_comparison_charts(data, output_dir, commit, compare_to):
    """Generate comparison charts for two specific commits."""
    if not commit or not compare_to:
        return
    
    os.makedirs(output_dir, exist_ok=True)
    
    for project_name, project_data in data.items():
        if not project_data:
            continue
        
        # Find data for the specified commits
        current_data = None
        previous_data = None
        
        for result in project_data:
            if result.get("commit", "") == commit:
                current_data = result
            elif result.get("commit", "") == compare_to:
                previous_data = result
        
        if not current_data or not previous_data:
            print(f"Could not find data for both commits for project {project_name}")
            continue
        
        # Create project directory
        project_dir = os.path.join(output_dir, project_name)
        os.makedirs(project_dir, exist_ok=True)
        
        # Generate comparison charts for each benchmark type
        for benchmark_type in current_data.get("metrics", {}).keys():
            if benchmark_type not in previous_data.get("metrics", {}):
                continue
            
            # Create benchmark type directory
            benchmark_dir = os.path.join(project_dir, benchmark_type)
            os.makedirs(benchmark_dir, exist_ok=True)
            
            # Get all metrics for this benchmark type
            metrics = set()
            for key in current_data["metrics"][benchmark_type].keys():
                if not isinstance(current_data["metrics"][benchmark_type][key], dict):
                    metrics.add(key)
            
            # Generate chart for each metric
            for metric in metrics:
                if metric not in previous_data["metrics"][benchmark_type]:
                    continue
                
                current_value = current_data["metrics"][benchmark_type][metric]
                previous_value = previous_data["metrics"][benchmark_type][metric]
                
                # Create chart
                plt.figure(figsize=(8, 6))
                
                # Plot values
                labels = [f"Previous ({compare_to[:7]})", f"Current ({commit[:7]})"]
                values = [previous_value, current_value]
                
                plt.bar(labels, values)
                
                # Add title and labels
                plt.title(f"{project_name} - {benchmark_type} - {metric}")
                plt.ylabel(metric)
                
                # Add value labels
                for i, v in enumerate(values):
                    plt.text(i, v, f"{v:.2f}", ha="center", va="bottom")
                
                # Save chart
                chart_file = os.path.join(benchmark_dir, f"{metric}_comparison.png")
                plt.savefig(chart_file)
                plt.close()
                
                print(f"Generated comparison chart: {chart_file}")

def generate_html_dashboard(data, output_dir, commit=None, compare_to=None):
    """Generate HTML dashboard for benchmark results."""
    os.makedirs(output_dir, exist_ok=True)
    
    # Generate index.html
    with open(os.path.join(output_dir, "index.html"), "w") as f:
        f.write("""<!DOCTYPE html>
<html>
<head>
    <title>NativeLink Benchmark Dashboard</title>
    <style>
        body { font-family: Arial, sans-serif; margin: 20px; }
        h1, h2, h3 { color: #333; }
        .project { margin-bottom: 30px; }
        .benchmark { margin-bottom: 20px; }
        .chart { margin: 10px 0; }
        table { border-collapse: collapse; width: 100%; }
        th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }
        th { background-color: #f2f2f2; }
        tr:nth-child(even) { background-color: #f9f9f9; }
        .regression { color: red; }
        .improvement { color: green; }
    </style>
</head>
<body>
    <h1>NativeLink Benchmark Dashboard</h1>
    <p>Last updated: %s</p>
""" % datetime.now().strftime("%Y-%m-%d %H:%M:%S"))
        
        # Add links to project pages
        f.write("    <h2>Projects</h2>\n    <ul>\n")
        for project_name in data.keys():
            f.write(f'        <li><a href="{project_name}/index.html">{project_name}</a></li>\n')
        f.write("    </ul>\n")
        
        f.write("</body>\n</html>")
    
    # Generate project pages
    for project_name, project_data in data.items():
        if not project_data:
            continue
        
        # Create project directory
        project_dir = os.path.join(output_dir, project_name)
        os.makedirs(project_dir, exist_ok=True)
        
        # Generate project index.html
        # Around line 267-282 - Fixed string formatting and variable reference
        html_content = (  # Start of multi-line string
            f"""
            <!DOCTYPE html>
            <html>
            <head>
                <title>Benchmark Results</title>
                <script src="https://cdn.plot.ly/plotly-latest.min.js"></script>
            </head>
            <body>
                <div id="chart-container"></div>
            </body>
            </html>
            """
        )  # Added closing parenthesis
        with open(os.path.join(project_dir, "index.html"), "w") as f:
            f.write(html_content)  # Use the properly formatted content