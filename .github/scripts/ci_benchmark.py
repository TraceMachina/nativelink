#!/usr/bin/env python3
"""Run identical test workloads and retain only whitelisted aggregate evidence."""

import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time
from urllib.parse import urlsplit
import uuid


def cache_usage(summary):
    runners = summary.get("runnerCount", [])
    counts = {}
    for runner in runners:
        count = int(runner.get("count", 0))
        if count < 0:
            raise ValueError("Negative process count")
        name = runner["name"]
        counts[name] = counts.get(name, 0) + count
    stats = summary.get("actionCacheStatistics")
    local = int(stats.get("hits", 0)) if stats is not None else counts.get("action cache hit")
    captured = local is not None and "total" in counts
    outcomes = sum(value for name, value in counts.items()
                   if name not in ("total", "internal", "action cache hit"))
    disk = counts.get("disk cache hit", 0 if captured else None)
    remote = counts.get("remote cache hit", 0 if captured else None)
    hits = local + (disk or 0) + (remote or 0) if captured else None
    denominator = local + outcomes if captured else None
    return {"local_action_hits": local, "disk_hits": disk, "remote_hits": remote,
            "total_hits": hits, "reported_outcomes": denominator,
            "total_reuse_percent": 100 * hits / denominator if denominator else None}


def aggregates(path):
    result = {"cache": cache_usage({}), "tests": {"total": 0, "cached": 0, "failed": 0}}
    if not path.exists():
        return result
    with path.open() as events:
        for line in events:
            event = json.loads(line)
            if "buildMetrics" in event:
                result["cache"] = cache_usage(event["buildMetrics"].get("actionSummary", {}))
            if "testSummary" in event:
                summary = event["testSummary"]
                result["tests"]["total"] += 1
                result["tests"]["cached"] += int(summary.get("totalRunCount", 0)) > 0 and (
                    int(summary.get("totalNumCached", 0)) == int(summary.get("totalRunCount", 0)))
                result["tests"]["failed"] += summary.get("overallStatus") not in ("PASSED", "FLAKY")
    return result


def configuration(lane, environ):
    if not environ.get("NL_API_KEY"):
        raise ValueError("The CI API key is missing")
    if lane == "baseline":
        claim = environ["NL_BASELINE_CLAIM"]
        cache, bep = f"grpcs://cas-{claim}", f"grpcs://bes-{claim}"
    else:
        cache, bep = environ["NL_CLUSTER_ENDPOINT"], environ["NL_CLUSTER_BEP_ENDPOINT"]
    endpoints = (cache, bep)
    for endpoint in endpoints:
        parsed = urlsplit(endpoint)
        if parsed.scheme != "grpcs" or not parsed.hostname or parsed.username or parsed.password:
            raise ValueError("Expected TLS build endpoints without embedded credentials")
    helper = Path(__file__).with_name("benchmark_credentials.py").resolve()
    flags = ["--config=nl-cache", "--config=nl-toolchain", "--config=nl-quiet",
             f"--remote_cache={cache}", f"--bes_backend={bep}",
             "--remote_header=x-nativelink-project=nativelink-github-ci",
             "--bes_header=x-nativelink-project=nativelink-github-ci",
             "--build_metadata=ROLE=CI", f"--build_metadata=BENCHMARK_LANE={lane}",
             f"--build_metadata=GITHUB_SHA={environ['GITHUB_SHA']}",
             "--extra_toolchains=@@rules_python++python+pythons_hub//:all",
             "--lockfile_mode=error", "--verbose_failures"]
    for host in sorted({urlsplit(endpoint).hostname for endpoint in endpoints}):
        flags.append(f"--credential_helper={host}={helper}")
    if lane == "cluster":
        flags += ["--config=nl-rbe", f"--remote_executor={cache}",
                  f"--remote_instance_name={environ['NL_INSTANCE']}",
                  f"--bes_results_url={environ['NL_RESULTS_URL']}",
                  f"--remote_default_exec_properties=container-image={environ['NL_WORKER_IMAGE']}",
                  "--remote_default_exec_properties=cpu_count=1000",
                  "--remote_default_exec_properties=memory_kb=2000000",
                  "--remote_default_exec_properties=disk_kb=1000000",
                  "--remote_upload_local_results=true", "--jobs=48"]
        if environ.get("NL_PERSISTENT_CACHE"):
            root = environ["NL_PERSISTENT_CACHE"]
            flags += [f"--disk_cache={root}/cache/actions",
                      f"--repository_cache={root}/cache/repositories"]
    else:
        flags += ["--remote_upload_local_results=false"]
    return flags, " ".join(endpoints)


def execute(command, environ):
    # Stream useful compiler errors, redact exact credentials defensively, and
    # never persist console output or a subprocess exception containing argv.
    with subprocess.Popen(command, env=environ, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True) as process:
        for line in process.stdout:
            print(line.replace(environ["NL_API_KEY"], "[REDACTED]"), end="", flush=True)
        return process.wait()


def run(args):
    start = time.monotonic()
    environ = os.environ.copy()
    flags, environ["NL_ENDPOINTS"] = configuration(args.lane, environ)
    result = {"schema": 1, "lane": args.lane, "revision": environ["GITHUB_SHA"],
              "bazel_version": Path(".bazelversion").read_text().strip(),
              "setup_seconds": time.time() - float(environ["BENCHMARK_STARTED"]),
              "workload": "bazel test //...", "runs": []}
    status = 0
    with tempfile.TemporaryDirectory(prefix="ci-benchmark-") as temporary:
        for index in range(args.repetitions):
            bep = Path(temporary) / "events.json"
            invocation = str(uuid.uuid4())
            startup = []
            if environ.get("NL_PERSISTENT_CACHE"):
                startup = [f"--output_user_root={environ['NL_PERSISTENT_CACHE']}/bazel-ci",
                           "--max_idle_secs=0", "--host_jvm_args=-Xmx4g"]
            command = ["nix", "develop", "--impure", "--command", "bazel", *startup, "test", "//...",
                       *flags, f"--invocation_id={invocation}", f"--build_event_json_file={bep}"]
            before = time.monotonic()
            status = execute(command, environ)
            record = {"iteration": index + 1, "invocation_id": invocation,
                      "seconds": time.monotonic() - before, "exit_code": status,
                      **aggregates(bep)}
            result["runs"].append(record)
            bep.unlink(missing_ok=True)
            if status:
                break
    result["build_test_seconds"] = time.monotonic() - start
    result["total_seconds"] = result["setup_seconds"] + result["build_test_seconds"]
    result["exit_code"] = status
    args.output.mkdir(parents=True, exist_ok=True)
    # Only this constructed object is uploaded. No raw BEP, headers, rc files,
    # environment dumps, build logs or arbitrary server messages.
    (args.output / f"{args.lane}.json").write_text(json.dumps(result, indent=2) + "\n")
    return status


def compare(directory):
    paths = sorted(directory.glob("*.json"))
    if {path.stem for path in paths} != {"baseline", "cluster"}:
        raise ValueError("Comparison requires evidence from both lanes")
    print("## Parallel CI benchmark\n")
    print("| Lane | Setup (s) | First build/test (s) | Warm median (s) | Total (s) | Result |")
    print("| --- | ---: | ---: | ---: | ---: | --- |")
    revisions = set()
    results = []
    for path in paths:
        result = json.loads(path.read_text())
        results.append(result)
        revisions.add(result["revision"])
        runs = result["runs"]
        warm = [run["seconds"] for run in runs[1:] if not run["exit_code"]]
        warm_text = f"{statistics.median(warm):.1f}" if warm else "—"
        print(f"| {result['lane']} | {result['setup_seconds']:.1f} | "
              f"{runs[0]['seconds']:.1f} | {warm_text} | {result['total_seconds']:.1f} | "
              f"{'failed' if result['exit_code'] else 'passed'} |")
    if len(revisions) != 1:
        raise ValueError("Comparison requires the same revision in both lanes")
    print("\n| Lane / iteration | Total reuse | Local hits | Disk hits | Remote hits | Cached test targets |")
    print("| --- | ---: | ---: | ---: | ---: | ---: |")
    for result in results:
        for run in result["runs"]:
            cache = run["cache"]
            ratio = cache["total_reuse_percent"]
            percentage = f"{ratio:.1f}%" if ratio is not None else "Not captured"
            counters = [cache[name] if cache[name] is not None else "Unknown"
                        for name in ("local_action_hits", "disk_hits", "remote_hits")]
            print(f"| {result['lane']} / {run['iteration']} | {percentage} | "
                  f"{counters[0]} | {counters[1]} | {counters[2]} | "
                  f"{run['tests']['cached']} / {run['tests']['total']} |")
    print("\nFirst-run cache state is pre-existing/unknown, not cold. Warm repeats retain the same "
          "client process and test-result caching. Baseline uses the current staging cache in "
          "read-only mode; cluster uses its separate writable cache and warm execution workers. "
          "The cluster client retains its Bazel and Nix state between runs. Build and test time is combined; "
          "setup excludes runner queue time and cache-save teardown. Cache-layer counts are in the JSON "
          "artifacts; cached tests overlap action reuse and are not added to it.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    runner = sub.add_parser("run")
    runner.add_argument("--lane", choices=("baseline", "cluster"), required=True)
    runner.add_argument("--repetitions", type=int, choices=range(1, 21), default=3)
    runner.add_argument("--output", type=Path, required=True)
    reporter = sub.add_parser("compare")
    reporter.add_argument("directory", type=Path)
    arguments = parser.parse_args()
    if arguments.command == "run":
        raise SystemExit(run(arguments))
    compare(arguments.directory)
