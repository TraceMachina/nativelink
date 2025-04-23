#!/usr/bin/env python3

import argparse
import json
import os
from pathlib import Path
import matplotlib.pyplot as plt
import pandas as pd
from typing import Dict, List, Optional

def load_results(results_dir: str) -> List[Dict]:
    """Load all benchmark results from the specified directory."""
    results = []
    for file_path in Path(results_dir).glob("*.json"):
        with open(file_path, "r") as f:
            try:
                data = json.load(f)
                results.append(data)
            except json.JSONDecodeError:
                print(f"Error parsing {file_path}")
    
    # Sort by timestamp
    results.sort(key=lambda x: x.get("timestamp", ""))
    return results

def generate_time_series_chart(results: List[Dict], project: str, test_name: str, metric: str, output_dir: str):
    """Generate a time series chart for a specific metric."""
    data = []
    for result in results:
        if result.get("project") == project:
            if test_name in result.get("metrics", {}):
                if metric in result.get("metrics", {}).get(test_name, {}):
                    data.append({
                        "commit": result.get("commit", "")[:7],  # Short commit hash
                        "timestamp": result.get("timestamp", ""),
                        "value": result.get("metrics", {}).get(test_name, {}).get(metric, 0)
                    })
    
    if not data:
        return
    
    df = pd.DataFrame(data)
    
    plt.figure(figsize=(12, 6))
    plt.plot(df["timestamp"], df["value"], marker='o')
    plt.title(f"{project} - {test_name} - {metric}")
    plt.xlabel("Timestamp")
    plt.ylabel(metric)
    plt.xticks(rotation=45)
    plt.tight_layout()
    
    # Create output directory if it doesn't exist
    os.makedirs(output_dir, exist_ok=True)
    
    # Save the chart
    chart_path = os.path.join(output_dir, f"{project}_{test_name}_{metric}.png")
    plt.savefig(chart_path)
    plt.close()
    
    return chart_path

def generate_html_report(results: List[Dict], output_dir: str):
    """Generate an HTML report with all charts."""
    projects = set(result.get("project", "") for result in results)
    
    html = """
    <!DOCTYPE html>
    <html>
    <head>
        <title>NativeLink Benchmark Results</title>
        <style>
            body { font-family: Arial, sans-serif; margin: 20px; }
            h1, h2, h3 { color: #333; }
            .chart-container { margin-bottom: 30px; }
            img { max-width: 100%; border: 1px solid #ddd; }
        </style>
    </head>
    <body>
        <h1>NativeLink Benchmark Results</h1>
    """
    
    for project in projects:
        html += f"<h2>{project}</h2>"
        
        # Get all test names for this project
        test_names = set()
        for result in results:
            if result.get("project") == project:
                test_names.update(result.get("metrics", {}).keys())
        
        for test_name in test_names:
            html += f"<h3>{test_name}</h3>"
            
            # Get all metrics for this test
            metrics = set()
            for result in results:
                if result.get("project") == project:
                    if test_name in result.get("metrics", {}):
                        metrics.update(result.get("metrics", {}).get(test_name, {}).keys())
            
            for metric in metrics:
                chart_path = generate_time_series_chart(results, project, test_name, metric, output_dir)
                if chart_path:
                    relative_path = os.path.basename(chart_path)
                    html += f"""
                    <div class="chart-container">
                        <h4>{metric}</h4>
                        <img src="{relative_path}" alt="{project} {test_name} {metric}">
                    </div>
                    """
    
    html += """
    </body>
    </html>
    """
    
    # Save the HTML report
    report_path = os.path.join(output_dir, "index.html")
    with open(report_path, "w") as f:
        f.write(html)
    
    return report_path

def main():
    parser = argparse.ArgumentParser(description="Generate visualizations for NativeLink benchmark results")
    parser.add_argument("--results-dir", default="./benchmark_results", help="Directory containing benchmark results")
    parser.add_argument("--output-dir", default="./benchmark_viz", help="Directory to store visualizations")
    
    args = parser.parse_args()
    
    results = load_results(args.results_dir)
    if not results:
        print(f"No benchmark results found in {args.results_dir}")
        return
    
    report_path = generate_html_report(results, args.output_dir)
    print(f"HTML report generated: {report_path}")

if __name__ == "__main__":
    main()