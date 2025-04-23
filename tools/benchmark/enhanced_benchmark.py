#!/usr/bin/env python3

import argparse
import json
import os
import subprocess
import time
import datetime
import statistics
import sys
import re
from pathlib import Path

def parse_args():
    parser = argparse.ArgumentParser(description="Run NativeLink benchmarks")
    parser.add_argument("--github-run-id", help="GitHub Actions run ID")
    parser.add_argument("--github-sha", help="GitHub commit SHA")
    parser.add_argument("--regression-threshold", type=float, default=10.0, 
                        help="Percentage threshold for regression detection")
    parser.add_argument("--project", required=True, help="Path to the project to benchmark")
    parser.add_argument("--commit", required=True, help="Current commit hash")
    parser.add_argument("--output-dir", default="./benchmark_results", 
                        help="Directory to store benchmark results")
    parser.add_argument("--nativelink-url", default="https://app.nativelink.com", 
                        help="NativeLink service URL")
    parser.add_argument("--api-key", help="NativeLink API key")
    parser.add_argument("--compare-to", help="Previous commit hash to compare against")
    parser.add_argument("--runs", type=int, default=3, help="Number of benchmark runs")
    parser.add_argument("--incremental", action="store_true", help="Run incremental build benchmarks")
    parser.add_argument("--network-stats", action="store_true", help="Collect network statistics")
    
    return parser.parse_args()

def run_command(cmd, cwd=None, env=None):
    """Run a command and return its output and execution time."""
    print(f"Running command: {' '.join(cmd)}")
    start_time = time.time()
    
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=cwd,
        env=env
    )
    
    stdout, stderr = process.communicate()
    end_time = time.time()
    
    execution_time = end_time - start_time
    return {
        "stdout": stdout,
        "stderr": stderr,
        "exit_code": process.returncode,
        "execution_time": execution_time
    }

def parse_bazel_output(output):
    """Parse Bazel output to extract metrics."""
    metrics = {}
    
    # Extract build time
    build_time_match = re.search(r'Elapsed time: (\d+\.\d+)s', output)
    if build_time_match:
        metrics["build_time"] = float(build_time_match.group(1))
    
    # Extract cache hits
    cache_hits_match = re.search(r'(\d+) remote cache hit', output)
    if cache_hits_match:
        metrics["cache_hits"] = int(cache_hits_match.group(1))
    
    # Extract cache misses
    cache_misses_match = re.search(r'(\d+) remote cache miss', output)
    if cache_misses_match:
        metrics["cache_misses"] = int(cache_misses_match.group(1))
    
    # Calculate cache hit rate if both hits and misses are available
    if "cache_hits" in metrics and "cache_misses" in metrics:
        total = metrics["cache_hits"] + metrics["cache_misses"]
        if total > 0:
            metrics["cache_hit_rate"] = (metrics["cache_hits"] / total) * 100
    
    return metrics

def collect_network_stats(start_time, end_time):
    """Collect network statistics during benchmark execution."""
    # This is a placeholder. In a real implementation, you would use
    # platform-specific tools to collect network statistics.
    return {
        "bytes_sent": 0,
        "bytes_received": 0,
        "packets_sent": 0,
        "packets_received": 0
    }

def run_benchmark(args, benchmark_type, bazel_args):
    """Run a single benchmark."""
    results = []
    
    for run in range(args.runs):
        print(f"Running {benchmark_type} benchmark (run {run+1}/{args.runs})...")
        
        # Clean Bazel cache if needed
        if benchmark_type != "incremental":
            clean_cmd = ["bazel", "clean", "--expunge"]
            run_command(clean_cmd, cwd=args.project)
        
        # Prepare environment
        env = os.environ.copy()
        if args.api_key:
            env["NATIVELINK_API_KEY"] = args.api_key
        
        # Start network monitoring if requested
        network_start_time = time.time()
        
        # Run the benchmark
        cmd = ["bazel", "build", "//..."] + bazel_args
        result = run_command(cmd, cwd=args.project, env=env)
        
        # Stop network monitoring
        network_end_time = time.time()
        
        # Parse metrics
        metrics = parse_bazel_output(result["stdout"] + result["stderr"])
        metrics["total_time"] = result["execution_time"]
        
        # Add network stats if requested
        if args.network_stats:
            metrics["network"] = collect_network_stats(network_start_time, network_end_time)
        
        results.append(metrics)
    
    # Calculate average metrics
    avg_metrics = {}
    for key in results[0].keys():
        if key == "network":
            # Average network stats separately
            avg_network = {}
            for net_key in results[0]["network"].keys():
                values = [r["network"][net_key] for r in results]
                avg_network[net_key] = sum(values) / len(values)
            avg_metrics["network"] = avg_network
        else:
            # Average other metrics
            values = [r[key] for r in results]
            avg_metrics[key] = sum(values) / len(values)
            
            # Add standard deviation for time metrics
            if "time" in key:
                avg_metrics[f"{key}_stddev"] = statistics.stdev(values) if len(values) > 1 else 0
    
    return avg_metrics

def compare_results(current, previous):
    """Compare current and previous benchmark results."""
    comparison = {}
    
    for key in current.keys():
        if key in previous:
            if isinstance(current[key], dict):
                # Handle nested dictionaries (like network stats)
                comparison[key] = compare_results(current[key], previous[key])
            else:
                # Calculate percentage change
                change = ((current[key] - previous[key]) / previous[key]) * 100
                comparison[key] = {
                    "current": current[key],
                    "previous": previous[key],
                    "change_pct": change
                }
    
    return comparison

def detect_regressions(comparison, threshold):
    """Detect performance regressions based on comparison results."""
    regressions = []
    
    for key, value in comparison.items():
        if isinstance(value, dict):
            if "change_pct" in value:
                # For metrics where higher is worse (like build time)
                if "time" in key and value["change_pct"] > threshold:
                    regressions.append({
                        "metric": key,
                        "current": value["current"],
                        "previous": value["previous"],
                        "change_pct": value["change_pct"]
                    })
                # For metrics where lower is worse (like cache hit rate)
                elif "rate" in key and value["change_pct"] < -threshold:
                    regressions.append({
                        "metric": key,
                        "current": value["current"],
                        "previous": value["previous"],
                        "change_pct": value["change_pct"]
                    })
            else:
                # Recursively check nested dictionaries
                nested_regressions = detect_regressions(value, threshold)
                if nested_regressions:
                    regressions.extend([{
                        "metric": f"{key}.{r['metric']}",
                        "current": r["current"],
                        "previous": r["previous"],
                        "change_pct": r["change_pct"]
                    } for r in nested_regressions])
    
    return regressions

def generate_summary(project_name, benchmark_results, comparison=None, regressions=None):
    """Generate a markdown summary of benchmark results."""
    summary = f"# NativeLink Benchmark Results for {project_name}\n\n"
    summary += f"## Commit: {args.commit}\n\n"
    
    # Add benchmark results
    summary += "## Benchmark Results\n\n"
    summary += "| Metric | Value |\n"
    summary += "| ------ | ----- |\n"
    
    for benchmark_type, metrics in benchmark_results.items():
        summary += f"\n### {benchmark_type.capitalize()} Build\n\n"
        summary += "| Metric | Value |\n"
        summary += "| ------ | ----- |\n"
        
        for key, value in metrics.items():
            if isinstance(value, dict):
                # Handle nested dictionaries (like network stats)
                summary += f"| **{key}** | |\n"
                for sub_key, sub_value in value.items():
                    summary += f"| &nbsp;&nbsp;{sub_key} | {sub_value:.2f} |\n"
            else:
                # Format value based on type
                if "time" in key:
                    formatted_value = f"{value:.2f}s"
                elif "rate" in key:
                    formatted_value = f"{value:.2f}%"
                else:
                    formatted_value = f"{value}"
                
                summary += f"| {key} | {formatted_value} |\n"
    
    # Add comparison if available
    if comparison:
        summary += "\n## Comparison with Previous Commit\n\n"
        summary += f"Comparing current commit ({args.commit}) with previous commit ({args.compare_to}).\n\n"
        
        for benchmark_type, metrics_comparison in comparison.items():
            summary += f"\n### {benchmark_type.capitalize()} Build\n\n"
            summary += "| Metric | Current | Previous | Change |\n"
            summary += "| ------ | ------- | -------- | ------ |\n"
            
            for key, value in metrics_comparison.items():
                if isinstance(value, dict) and "change_pct" in value:
                    # Format values based on type
                    if "time" in key:
                        current = f"{value['current']:.2f}s"
                        previous = f"{value['previous']:.2f}s"
                    elif "rate" in key:
                        current = f"{value['current']:.2f}%"
                        previous = f"{value['previous']:.2f}%"
                    else:
                        current = f"{value['current']}"
                        previous = f"{value['previous']}"
                    
                    # Format change with color based on direction
                    change_pct = value["change_pct"]
                    if ("time" in key and change_pct > 0) or ("rate" in key and change_pct < 0):
                        change = f"🔴 +{change_pct:.2f}%"
                    else:
                        change = f"🟢 {change_pct:.2f}%"
                    
                    summary += f"| {key} | {current} | {previous} | {change} |\n"
                elif isinstance(value, dict):
                    # Handle nested dictionaries (like network stats)
                    summary += f"| **{key}** | | | |\n"
                    for sub_key, sub_value in value.items():
                        if isinstance(sub_value, dict) and "change_pct" in sub_value:
                            current = f"{sub_value['current']:.2f}"
                            previous = f"{sub_value['previous']:.2f}"
                            
                            change_pct = sub_value["change_pct"]
                            if change_pct > 0:
                                change = f"🔴 +{change_pct:.2f}%"
                            else:
                                change = f"🟢 {change_pct:.2f}%"
                            
                            summary += f"| &nbsp;&nbsp;{sub_key} | {current} | {previous} | {change} |\n"
    
    # Add regressions if available
    if regressions:
        summary += "\n## Performance Regressions\n\n"
        
        if not any(regressions.values()):
            summary += "No significant performance regressions detected.\n"
        else:
            for benchmark_type, benchmark_regressions in regressions.items():
                if benchmark_regressions:
                    summary += f"\n### {benchmark_type.capitalize()} Build\n\n"
                    summary += "| Metric | Current | Previous | Change |\n"
                    summary += "| ------ | ------- | -------- | ------ |\n"
                    
                    for regression in benchmark_regressions:
                        # Format values based on type
                        if "time" in regression["metric"]:
                            current = f"{regression['current']:.2f}s"
                            previous = f"{regression['previous']:.2f}s"
                        elif "rate" in regression["metric"]:
                            current = f"{regression['current']:.2f}%"
                            previous = f"{regression['previous']:.2f}%"
                        else:
                            current = f"{regression['current']}"
                            previous = f"{regression['previous']}"
                        
                        change = f"🔴 {regression['change_pct']:.2f}%"
                        summary += f"| {regression['metric']} | {current} | {previous} | {change} |\n"
    
    return summary

def main(args):
    # Create output directory
    os.makedirs(args.output_dir, exist_ok=True)
    
    # Extract project name from path
    project_name = os.path.basename(os.path.abspath(args.project))
    
    # Prepare benchmark configurations
    benchmark_configs = {
        "remote_cache_only": [
            "--remote_cache=" + args.nativelink_url,
            "--remote_header=x-nativelink-api-key=" + args.api_key
        ],
        "remote_cache_and_execution": [
            "--remote_cache=" + args.nativelink_url,
            "--remote_executor=" + args.nativelink_url,
            "--remote_header=x-nativelink-api-key=" + args.api_key
        ],
        "remote_execution_only": [
            "--remote_executor=" + args.nativelink_url,
            "--remote_header=x-nativelink-api-key=" + args.api_key,
            "--noremote_accept_cached"
        ]
    }
    
    # Run benchmarks
    benchmark_results = {}
    for benchmark_type, bazel_args in benchmark_configs.items():
        print(f"Running {benchmark_type} benchmark...")
        benchmark_results[benchmark_type] = run_benchmark(args, benchmark_type, bazel_args)
    
    # Run incremental build benchmark if requested
    if args.incremental:
        print("Running incremental build benchmark...")
        
        # First, do a clean build
        clean_cmd = ["bazel", "clean", "--expunge"]
        run_command(clean_cmd, cwd=args.project)
        
        # Run initial build
        initial_build_args = benchmark_configs["remote_cache_and_execution"]
        run_command(["bazel", "build", "//..."] + initial_build_args, cwd=args.project)
        
        # Make a small change to a source file
        # This is a placeholder. In a real implementation, you would modify a source file.
        # For now, we'll just touch a file to simulate a change.
        source_files = list(Path(args.project).glob("**/*.rs")) + list(Path(args.project).glob("**/*.cc"))
        if source_files:
            source_file = source_files[0]
            print(f"Modifying source file: {source_file}")
            with open(source_file, "a") as f:
                f.write("\n// Modified for incremental build benchmark\n")
        
        # Run incremental build benchmark
        benchmark_results["incremental"] = run_benchmark(args, "incremental", initial_build_args)
    
    # Save benchmark results
    timestamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    result_file = os.path.join(args.output_dir, f"{project_name}_{args.commit}_{timestamp}.json")
    
    with open(result_file, "w") as f:
        json.dump({
            "project": project_name,
            "commit": args.commit,
            "timestamp": timestamp,
            "metrics": benchmark_results
        }, f, indent=2)
    
    print(f"Benchmark results saved to {result_file}")
    
    # Compare with previous results if requested
    comparison = None
    regressions = None
    
    if args.compare_to:
        print(f"Comparing with previous commit: {args.compare_to}")
        
        # Find previous benchmark results
        previous_files = list(Path(args.output_dir).glob(f"{project_name}_{args.compare_to}_*.json"))
        
        if previous_files:
            # Use the most recent previous result
            previous_file = sorted(previous_files)[-1]
            print(f"Using previous benchmark results from {previous_file}")
            
            with open(previous_file, "r") as f:
                previous_data = json.load(f)
            
            # Compare results
            comparison = {}
            regressions = {}
            
            for benchmark_type in benchmark_results.keys():
                if benchmark_type in previous_data["metrics"]:
                    comparison[benchmark_type] = compare_results(
                        benchmark_results[benchmark_type],
                        previous_data["metrics"][benchmark_type]
                    )
                    
                    # Detect regressions
                    regressions[benchmark_type] = detect_regressions(
                        comparison[benchmark_type],
                        args.regression_threshold
                    )
        else:
            print(f"No previous benchmark results found for commit {args.compare_to}")
    
    # Generate summary
    summary = generate_summary(project_name, benchmark_results, comparison, regressions)
    
    # Save summary
    summary_file = os.path.join(args.output_dir, "summary.md")
    with open(summary_file, "w") as f:
        f.write(summary)
    
    print(f"Benchmark summary saved to {summary_file}")
    
    # Print summary to console
    print("\n" + summary)
    
    # Exit with error code if regressions were detected
    if regressions and any(regressions.values()):
        print("Performance regressions detected!")
        return 1
    
    return 0

if __name__ == "__main__":
    args = parse_args()
    sys.exit(main(args))