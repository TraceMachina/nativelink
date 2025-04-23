#!/usr/bin/env python3

import argparse
import datetime
import json
import os
import subprocess
import time
from pathlib import Path

class NativeLinkBenchmark:
    def __init__(self, repo_path, output_dir, nativelink_url):
        self.repo_path = Path(repo_path)
        self.output_dir = Path(output_dir)
        self.nativelink_url = nativelink_url
        self.output_dir.mkdir(parents=True, exist_ok=True)
        
    def run_benchmark(self, commit_hash=None, project="rustlings"):
        """Run benchmarks for a specific commit"""
        if commit_hash:
            self._checkout_commit(commit_hash)
        
        # Record metadata
        metadata = {
            "timestamp": datetime.datetime.now().isoformat(),
            "commit": self._get_current_commit(),
            "project": project
        }
        
        # Run different benchmark configurations
        results = {}
        
        # 1. Build with remote cache only
        print(f"Running benchmark: remote cache only for {project}")
        cache_only_time = self._run_build_with_remote_cache(project)
        results["remote_cache_only"] = {
            "build_time_seconds": cache_only_time,
            "config": "remote cache only"
        }
        
        # 2. Build with remote cache and execution
        print(f"Running benchmark: remote cache and execution for {project}")
        cache_exec_time = self._run_build_with_remote_cache_and_execution(project)
        results["remote_cache_and_execution"] = {
            "build_time_seconds": cache_exec_time,
            "config": "remote cache and execution"
        }
        
        # Save results
        benchmark_data = {
            "metadata": metadata,
            "results": results
        }
        
        output_file = self.output_dir / f"{metadata['commit'][:8]}_{project}_{int(time.time())}.json"
        with open(output_file, 'w') as f:
            json.dump(benchmark_data, f, indent=2)
            
        print(f"Benchmark results saved to {output_file}")
        return benchmark_data
    
    def _checkout_commit(self, commit_hash):
        """Checkout a specific commit"""
        subprocess.run(["git", "checkout", commit_hash], cwd=self.repo_path, check=True)
        
    def _get_current_commit(self):
        """Get the current commit hash"""
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"], 
            cwd=self.repo_path, 
            check=True, 
            capture_output=True, 
            text=True
        )
        return result.stdout.strip()
    
    def _run_build_with_remote_cache(self, project):
        """Run build with remote cache only"""
        start_time = time.time()
        
        # Clean any previous builds
        subprocess.run(["bazel", "clean"], cwd=self.repo_path, check=True)
        
        # Configure remote cache
        bazel_command = [
            "bazel", 
            "build",
            f"//benchmark-projects/{project}",
            f"--remote_cache={self.nativelink_url}",
            "--remote_timeout=3600"
        ]
        
        subprocess.run(bazel_command, cwd=self.repo_path, check=True)
        
        end_time = time.time()
        return end_time - start_time
    
    def _run_build_with_remote_cache_and_execution(self, project):
        """Run build with remote cache and execution"""
        start_time = time.time()
        
        # Clean any previous builds
        subprocess.run(["bazel", "clean"], cwd=self.repo_path, check=True)
        
        # Configure remote cache and execution
        bazel_command = [
            "bazel", 
            "build",
            f"//benchmark-projects/{project}",
            f"--remote_cache={self.nativelink_url}",
            f"--remote_executor={self.nativelink_url}",
            "--remote_timeout=3600"
        ]
        
        subprocess.run(bazel_command, cwd=self.repo_path, check=True)
        
        end_time = time.time()
        return end_time - start_time

def main():
    parser = argparse.ArgumentParser(description='Run NativeLink benchmarks')
    parser.add_argument('--repo-path', default=os.getcwd(), help='Path to the NativeLink repository')
    parser.add_argument('--output-dir', default='benchmark_results', help='Directory to store benchmark results')
    parser.add_argument('--commit', help='Specific commit to benchmark')
    parser.add_argument('--project', default='rustlings', help='Project to benchmark')
    parser.add_argument('--nativelink-url', default='https://app.nativelink.com', help='NativeLink service URL')
    
    args = parser.parse_args()
    
    benchmark = NativeLinkBenchmark(args.repo_path, args.output_dir, args.nativelink_url)
    benchmark.run_benchmark(args.commit, args.project)

if __name__ == "__main__":
    main()