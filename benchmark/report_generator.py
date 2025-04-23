#!/usr/bin/env python3

import argparse
import json
import os
from pathlib import Path
import matplotlib.pyplot as plt
import pandas as pd
from datetime import datetime

class BenchmarkReportGenerator:
    def __init__(self, results_dir, output_dir):
        self.results_dir = Path(results_dir)
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)
        
    def generate_report(self):
        """Generate a report from benchmark results"""
        # Load all benchmark results
        results = []
        for result_file in self.results_dir.glob("*.json"):
            with open(result_file, 'r') as f:
                data = json.load(f)
                results.append(data)
        
        if not results:
            print("No benchmark results found")
            return
        
        # Sort results by timestamp
        results.sort(key=lambda x: x["metadata"]["timestamp"])
        
        # Create dataframes for different metrics
        cache_only_times = []
        cache_exec_times = []
        commits = []
        timestamps = []
        projects = []
        
        for result in results:
            commits.append(result["metadata"]["commit"][:8])
            timestamps.append(datetime.fromisoformat(result["metadata"]["timestamp"]))
            projects.append(result["metadata"]["project"])
            
            cache_only_times.append(result["results"]["remote_cache_only"]["build_time_seconds"])
            cache_exec_times.append(result["results"]["remote_cache_and_execution"]["build_time_seconds"])
        
        # Create a DataFrame
        df = pd.DataFrame({
            "commit": commits,
            "timestamp": timestamps,
            "project": projects,
            "cache_only_time": cache_only_times,
            "cache_exec_time": cache_exec_times
        })
        
        # Generate plots
        self._generate_time_series_plot(df)
        self._generate_comparison_plot(df)
        
        # Generate HTML report
        self._generate_html_report(df)
        
        print(f"Report generated in {self.output_dir}")
    
    def _generate_time_series_plot(self, df):
        """Generate time series plot of build times"""
        plt.figure(figsize=(12, 6))
        
        for project in df["project"].unique():
            project_df = df[df["project"] == project]
            plt.plot(project_df["timestamp"], project_df["cache_only_time"], 
                     marker='o', linestyle='-', label=f"{project} - Cache Only")
            plt.plot(project_df["timestamp"], project_df["cache_exec_time"], 
                     marker='x', linestyle='--', label=f"{project} - Cache+Execution")
        
        plt.title("NativeLink Build Times Over Time")
        plt.xlabel("Commit Date")
        plt.ylabel("Build Time (seconds)")
        plt.legend()
        plt.grid(True)
        plt.xticks(rotation=45)
        plt.tight_layout()
        
        plt.savefig(self.output_dir / "build_times_over_time.png")
    
    def _generate_comparison_plot(self, df):
        """Generate comparison plot between cache only and cache+execution"""
        plt.figure(figsize=(10, 6))
        
        for project in df["project"].unique():
            project_df = df[df["project"] == project]
            
            x = range(len(project_df))
            width = 0.35
            
            plt.bar([i - width/2 for i in x], project_df["cache_only_time"], 
                    width=width, label=f"{project} - Cache Only")
            plt.bar([i + width/2 for i in x], project_df["cache_exec_time"], 
                    width=width, label=f"{project} - Cache+Execution")
            
            plt.xticks(x, project_df["commit"])
        
        plt.title("Cache Only vs Cache+Execution Build Times")
        plt.xlabel("Commit")
        plt.ylabel("Build Time (seconds)")
        plt.legend()
        plt.grid(True, axis='y')
        plt.tight_layout()
        
        plt.savefig(self.output_dir / "cache_vs_execution.png")
    
    def _generate_html_report(self, df):
        """Generate HTML report"""
        html_content = f"""
        <!DOCTYPE html>
        <html>
        <head>
            <title>NativeLink Benchmark Report</title>
            <style>
                body {{ font-family: Arial, sans-serif; margin: 20px; }}
                h1, h2 {{ color: #333; }}
                table {{ border-collapse: collapse; width: 100%; margin-bottom: 20px; }}
                th, td {{ border: 1px solid #ddd; padding: 8px; text-align: left; }}
                th {{ background-color: #f2f2f2; }}
                tr:nth-child(even) {{ background-color: #f9f9f9; }}
                .chart {{ margin: 20px 0; max-width: 100%; }}
            </style>
        </head>
        <body>
            <h1>NativeLink Benchmark Report</h1>
            <p>Generated on {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}</p>
            
            <h2>Build Time Trends</h2>
            <div class="chart">
                <img src="build_times_over_time.png" alt="Build Times Over Time" style="max-width: 100%;">
            </div>
            
            <h2>Cache Only vs Cache+Execution</h2>
            <div class="chart">
                <img src="cache_vs_execution.png" alt="Cache vs Execution Comparison" style="max-width: 100%;">
            </div>
            
            <h2>Detailed Results</h2>
            <table>
                <tr>
                    <th>Commit</th>
                    <th>Timestamp</th>
                    <th>Project</th>
                    <th>Cache Only (s)</th>
                    <th>Cache+Execution (s)</th>
                    <th>Difference (s)</th>
                    <th>Improvement (%)</th>
                </tr>
        """
        
        for _, row in df.iterrows():
            diff = row["cache_only_time"] - row["cache_exec_time"]
            improvement = (diff / row["cache_only_time"]) * 100 if row["cache_only_time"] > 0 else 0
            
            html_content += f"""
                <tr>
                    <td>{row["commit"]}</td>
                    <td>{row["timestamp"].strftime('%Y-%m-%d %H:%M:%S')}</td>
                    <td>{row["project"]}</td>
                    <td>{row["cache_only_time"]:.2f}</td>
                    <td>{row["cache_exec_time"]:.2f}</td>
                    <td>{diff:.2f}</td>
                    <td>{improvement:.2f}%</td>
                </tr>
            """
        
        html_content += """
            </table>
        </body>
        </html>
        """
        
        with open(self.output_dir / "report.html", 'w') as f:
            f.write(html_content)

def main():
    parser = argparse.ArgumentParser(description='Generate NativeLink benchmark reports')
    parser.add_argument('--results-dir', default='benchmark_results', help='Directory with benchmark results')
    parser.add_argument('--output-dir', default='benchmark_reports', help='Directory to store reports')
    
    args = parser.parse_args()
    
    generator = BenchmarkReportGenerator(args.results_dir, args.output_dir)
    generator.generate_report()

if __name__ == "__main__":
    main()