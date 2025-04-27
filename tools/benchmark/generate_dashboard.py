#!/usr/bin/env python3

"""
Generate a web dashboard for NativeLink benchmarks.
Inspired by the Lucene Nightly Benchmarks project.
"""

import argparse
import json
import os
import glob
from datetime import datetime
from pathlib import Path
import matplotlib.pyplot as plt
import matplotlib.dates as mdates
import numpy as np


def parse_args():
    parser = argparse.ArgumentParser(description="Generate NativeLink benchmark dashboard")
    parser.add_argument("--input-dir", required=True, help="Directory with benchmark results")
    parser.add_argument("--output-dir", required=True, help="Directory for dashboard output")
    parser.add_argument(
        "--projects", default="rustlings,googletest", help="Comma-separated list of projects"
    )
    parser.add_argument(
        "--metrics", default="build_time,network_usage", help="Comma-separated list of metrics"
    )
    parser.add_argument("--days", type=int, default=30, help="Number of days to include")
    return parser.parse_args()


def load_benchmark_data(input_dir, projects):
    data = {}
    for project in projects:
        data[project] = []
        pattern = os.path.join(input_dir, f"{project}_*.json")
        for file_path in glob.glob(pattern):
            try:
                with open(file_path, "r") as f:
                    result = json.load(f)
                    data[project].append(result)
            except Exception as e:
                print(f"Error loading {file_path}: {e}")

    # Sort by timestamp
    for project in projects:
        data[project].sort(key=lambda x: x.get("timestamp", ""))

    return data


def generate_trend_charts(data, output_dir, projects, metrics):
    os.makedirs(output_dir, exist_ok=True)

    for project in projects:
        if not data[project]:
            continue

        for test_name in data[project][0].get("metrics", {}):
            for metric in metrics:
                if not any(
                    metric in result.get("metrics", {}).get(test_name, {})
                    for result in data[project]
                ):
                    continue

                dates = []
                values = []
                commits = []

                for result in data[project]:
                    if test_name in result.get("metrics", {}) and metric in result.get(
                        "metrics", {}
                    ).get(test_name, {}):
                        timestamp = result.get("timestamp", "")
                        try:
                            date = datetime.strptime(timestamp, "%Y%m%d_%H%M%S")
                            dates.append(date)
                            values.append(result["metrics"][test_name][metric])
                            commits.append(result.get("commit", "")[:7])
                        except ValueError:
                            print(f"Invalid timestamp format: {timestamp}")

                if not dates:
                    continue

                plt.figure(figsize=(12, 6))
                plt.plot(dates, values, "o-")
                plt.title(f"{project} - {test_name} - {metric}")
                plt.ylabel(metric)
                plt.grid(True)

                # Format x-axis
                plt.gca().xaxis.set_major_formatter(mdates.DateFormatter("%m-%d"))
                plt.gca().xaxis.set_major_locator(mdates.DayLocator(interval=2))
                plt.gcf().autofmt_xdate()

                # Add commit labels to points
                for i, (date, value, commit) in enumerate(zip(dates, values, commits)):
                    plt.annotate(
                        commit,
                        (date, value),
                        textcoords="offset points",
                        xytext=(0, 10),
                        ha="center",
                    )

                # Save the chart
                chart_filename = f"{project}_{test_name}_{metric}.png"
                chart_path = os.path.join(output_dir, chart_filename)
                plt.savefig(chart_path)
                plt.close()


def generate_html(data, output_dir, projects, metrics):
    html = """
    <!DOCTYPE html>
    <html>
    <head>
        <title>NativeLink Performance Dashboard</title>
        <style>
            body { font-family: Arial, sans-serif; margin: 20px; }
            h1, h2, h3 { color: #333; }
            .chart-container { margin: 20px 0; }
            table { border-collapse: collapse; width: 100%; }
            th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }
            th { background-color: #f2f2f2; }
            tr:nth-child(even) { background-color: #f9f9f9; }
            .regression { color: red; }
            .improvement { color: green; }
        </style>
    </head>
    <body>
        <h1>NativeLink Performance Dashboard</h1>
        <p>Last updated: %s</p>

        <h2>Performance Trends</h2>
    """ % datetime.now().strftime(
        "%Y-%m-%d %H:%M:%S"
    )

    # Add trend charts
    for project in projects:
        html += f"<h3>{project}</h3>"

        for test_name in data[project][0].get("metrics", {}) if data[project] else []:
            html += f"<h4>{test_name}</h4>"
            html += "<div class='chart-container'>"

            for metric in metrics:
                chart_filename = f"{project}_{test_name}_{metric}.png"
                if os.path.exists(os.path.join(output_dir, chart_filename)):
                    html += f"<img src='{chart_filename}' alt='{project} {test_name} {metric}' width='800'><br>"

            html += "</div>"

    # Add recent results table
    html += "<h2>Recent Results</h2>"

    for project in projects:
        if not data[project]:
            continue

        html += f"<h3>{project}</h3>"
        html += "<table>"
        html += "<tr><th>Date</th><th>Commit</th>"

        # Add headers for each metric
        for test_name in data[project][0].get("metrics", {}):
            for metric in metrics:
                if any(
                    metric in result.get("metrics", {}).get(test_name, {})
                    for result in data[project]
                ):
                    html += f"<th>{test_name} - {metric}</th>"

        html += "</tr>"

        # Add rows for recent results (last 10)
        for result in data[project][-10:]:
            timestamp = result.get("timestamp", "")
            try:
                date = datetime.strptime(timestamp, "%Y%m%d_%H%M%S").strftime("%Y-%m-%d")
            except ValueError:
                date = timestamp

            commit = result.get("commit", "")[:7]

            html += f"<tr><td>{date}</td><td>{commit}</td>"

            for test_name in data[project][0].get("metrics", {}):
                for metric in metrics:
                    if any(
                        metric in r.get("metrics", {}).get(test_name, {}) for r in data[project]
                    ):
                        value = result.get("metrics", {}).get(test_name, {}).get(metric, "N/A")
                        html += f"<td>{value}</td>"

            html += "</tr>"

        html += "</table>"

    html += """
    </body>
    </html>
    """

    # Write HTML file
    with open(os.path.join(output_dir, "index.html"), "w") as f:
        f.write(html)


def main():
    args = parse_args()
    projects = args.projects.split(",")
    metrics = args.metrics.split(",")

    data = load_benchmark_data(args.input_dir, projects)
    generate_trend_charts(data, args.output_dir, projects, metrics)
    generate_html(data, args.output_dir, projects, metrics)

    print(f"Dashboard generated in {args.output_dir}")


if __name__ == "__main__":
    main()
