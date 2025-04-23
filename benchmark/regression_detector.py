#!/usr/bin/env python3

import argparse
import json
import os
import sys
from pathlib import Path

def detect_regressions(results_dir, threshold_percent):
    """Detect performance regressions in benchmark results"""
    results_dir = Path(results_dir)
    results = []
    
    # Load all benchmark results
    for result_file in results_dir.glob("*.json"):
        with open(result_file, 'r') as f:
            data = json.load(f)
            results.append(data)
    
    if len(results) < 2:
        print("Not enough results to detect regressions")
        return True
    
    # Sort results by timestamp
    results.sort(key=lambda x: x["metadata"]["timestamp"])
    
    # Compare latest result with previous
    latest = results[-1]
    previous = results[-2]
    
    regressions = []
    
    # Check each benchmark configuration
    for config_name, config_data in latest["results"].items():
        if config_name in previous["results"]:
            latest_time = config_data["build_time_seconds"]
            previous_time = previous["results"][config_name]["build_time_seconds"]
            
            # Calculate regression percentage
            if previous_time > 0:
                change_percent = ((latest_time - previous_time) / previous_time) * 100
                
                if change_percent > threshold_percent:
                    regressions.append({
                        "config": config_name,
                        "previous_time": previous_time,
                        "latest_time": latest_time,
                        "change_percent": change_percent
                    })
    
    # Report regressions
    if regressions:
        print("⚠️ Performance regressions detected:")
        for reg in regressions:
            print(f"  - {reg['config']}: {reg['previous_time']:.2f}s → {reg['latest_time']:.2f}s " +
                  f"(+{reg['change_percent']:.2f}%)")
        return False
    
    print("✅ No performance regressions detected")
    return True

def main():
    parser = argparse.ArgumentParser(description='Detect NativeLink performance regressions')
    parser.add_argument('--results-dir', required=True, help='Directory with benchmark results')
    parser.add_argument('--threshold', type=float, default=5.0, 
                        help='Regression threshold percentage (default: 5.0)')
    
    args = parser.parse_args()
    
    if not detect_regressions(args.results_dir, args.threshold):
        sys.exit(1)

if __name__ == "__main__":
    main()