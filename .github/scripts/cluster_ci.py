#!/usr/bin/env python3
"""Dispatch trusted CI to one retained client using narrowly scoped pod exec."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile


# The input travels over the encrypted exec stream, never through argv or a
# credential file. Only the selected build environment is sent to the pod.
BOOTSTRAP = r"""
import {spawnSync} from 'node:child_process';
let input = ''; for await (const chunk of process.stdin) input += chunk;
let payload;
try { payload = JSON.parse(input); }
catch { process.stderr.write('Invalid dispatch payload\n'); process.exit(1); }
const script = `
set -euo pipefail
exec 9>/workspace/ci.lock
flock -n 9
cd /workspace/nativelink
git -c safe.directory=/workspace/nativelink diff --quiet HEAD --
git -c safe.directory=/workspace/nativelink fetch --depth=1 https://github.com/TraceMachina/nativelink.git "$GITHUB_SHA"
git -c safe.directory=/workspace/nativelink checkout --detach "$GITHUB_SHA"
mkdir -p "$NL_SUMMARY_DIRECTORY"
exec timeout --signal=TERM --kill-after=60 9000 python3 .github/scripts/ci_benchmark.py run --lane cluster --repetitions "$NL_REPETITIONS" --output "$NL_SUMMARY_DIRECTORY"
`;
const env = {...process.env, ...payload};
const result = spawnSync('bash', ['-euc', script], {env, stdio: ['ignore','inherit','inherit']});
process.exit(result.status ?? 1);
"""


def kubeconfig(environ):
    server = environ["NL_GKE_SERVER"]
    if not server.startswith("https://"):
        raise ValueError("The cluster API must use TLS")
    return {"apiVersion": "v1", "kind": "Config", "current-context": "ci",
            "clusters": [{"name": "ci", "cluster": {"server": server,
                           "certificate-authority-data": environ["NL_GKE_CA_DATA"]}}],
            "contexts": [{"name": "ci", "context": {"cluster": "ci", "user": "ci",
                          "namespace": environ["NL_GKE_NAMESPACE"]}}],
            "users": [{"name": "ci", "user": {"exec": {
                "apiVersion": "client.authentication.k8s.io/v1", "interactiveMode": "Never",
                "command": sys.executable,
                "args": [str(Path(__file__).with_name("cluster_credentials.py").resolve())]}}}]}


def payload(environ, repetitions):
    if not re.fullmatch(r"[0-9a-f]{40}", environ["GITHUB_SHA"]):
        raise ValueError("Expected an exact source revision")
    run = environ["GITHUB_RUN_ID"] + "-" + environ["GITHUB_RUN_ATTEMPT"]
    if not re.fullmatch(r"\d+-\d+", run) or not environ.get("NL_API_KEY"):
        raise ValueError("Expected a run identity and CI key")
    names = ("NL_API_KEY", "NL_CLUSTER_ENDPOINT", "NL_CLUSTER_BEP_ENDPOINT", "NL_INSTANCE",
             "NL_RESULTS_URL", "NL_WORKER_IMAGE", "GITHUB_SHA", "BENCHMARK_STARTED")
    return {**{name: environ[name] for name in names}, "NL_PERSISTENT_CACHE": "/workspace",
            "NL_REPETITIONS": str(repetitions), "NL_SUMMARY_DIRECTORY": "/workspace/ci-results/" + run}


def main(args):
    environ = os.environ.copy()
    build = payload(environ, args.repetitions)
    with tempfile.TemporaryDirectory(prefix="cluster-ci-") as temporary:
        config = Path(temporary) / "kubeconfig.json"
        # Public API address, CA and exec plugin path only. Tokens are obtained
        # in memory by the plugin for each command and are never written here.
        config.write_text(json.dumps(kubeconfig(environ)))
        environ["KUBECONFIG"] = str(config)
        base = ["kubectl", "--request-timeout=0", "-n", environ["NL_GKE_NAMESPACE"],
                "exec", environ["NL_GKE_POD"]]
        result = subprocess.run([*base, "-i", "--", "node", "--input-type=module", "-e", BOOTSTRAP],
                                input=json.dumps(build), text=True, env=environ)
        summary = subprocess.run([*base, "--", "cat", build["NL_SUMMARY_DIRECTORY"] + "/cluster.json"],
                                 text=True, capture_output=True, env=environ)
        if summary.returncode:
            sys.exit("The retained client did not produce benchmark evidence")
        report = json.loads(summary.stdout)
        if report["revision"] != environ["GITHUB_SHA"] or report["lane"] != "cluster":
            sys.exit("The retained client returned evidence for another source revision")
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / "cluster.json").write_text(json.dumps(report, indent=2) + "\n")
        return result.returncode or report["exit_code"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repetitions", type=int, choices=range(1, 21), default=3)
    parser.add_argument("--output", type=Path, required=True)
    raise SystemExit(main(parser.parse_args()))
