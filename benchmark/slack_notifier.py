#!/usr/bin/env python3

import argparse
import json
import os
import requests
from pathlib import Path

def send_slack_notification(webhook_url, message):
    """Send a notification to Slack"""
    payload = {
        "text": message,
        "mrkdwn": True
    }
    
    response = requests.post(webhook_url, json=payload)
    if response.status_code != 200:
        print(f"Failed to send Slack notification: {response.text}")
    else:
        print("Slack notification sent successfully")

def format_benchmark_message(commit, results_dir, repo, run_id):
    """Format a message with benchmark results"""
    results_dir = Path(results_dir)
    
    # Find the most recent result files for this commit
    result_files = list(results_dir.glob(f"*_{commit[:8]}_*.json"))
    
    if not result_files:
        return f"No benchmark results found for commit {commit[:8]}"
    
    # Start building the message
    message = f"*NativeLink Benchmark Results*\n"
    message += f"Repository: {repo}\n"
    message += f"Commit: {commit[:8]}\n"
    message += f"Run: <https://github.com/{repo}/actions/runs/{run_id}|View Run>\n\n"
    
    # Add results for each project
    for result_file in result_files:
        with open(result_file, 'r') as f:
            data = json.load(f)
        
        project = data.get("project", "unknown")
        message += f"*Project: {project}*\n"
        
        for config, metrics in data.get("metrics", {}).items():
            build_time = metrics.get("build_time", 0)
            message += f"• {config}: {build_time:.2f}s\n"
        
        message += "\n"
    
    message += "See the workflow artifacts for detailed reports."
    return message

def main():
    parser = argparse.ArgumentParser(description='Send benchmark results to Slack')
    parser.add_argument('--commit', required=True, help='Commit hash')
    parser.add_argument('--results-dir', required=True, help='Directory with benchmark results')
    parser.add_argument('--webhook-url', required=True, help='Slack webhook URL')
    parser.add_argument('--repo', required=True, help='GitHub repository (owner/repo)')
    parser.add_argument('--run-id', required=True, help='GitHub Actions run ID')
    
    args = parser.parse_args()
    
    message = format_benchmark_message(args.commit, args.results_dir, args.repo, args.run_id)
    send_slack_notification(args.webhook_url, message)

if __name__ == "__main__":
    main()