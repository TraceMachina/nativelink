#!/usr/bin/env python3
"""Dispatch trusted CI to one retained client using narrowly scoped pod exec."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
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
flock 9
cd /workspace/nativelink
git -c safe.directory=/workspace/nativelink diff --quiet HEAD -- >&2
git -c safe.directory=/workspace/nativelink fetch --depth=1 https://github.com/TraceMachina/nativelink.git "$GITHUB_SHA" >&2
git -c safe.directory=/workspace/nativelink checkout --detach "$GITHUB_SHA" >&2
mkdir -p "$NL_SUMMARY_DIRECTORY"
case "$NL_TASK" in
  bazel) exec python3 .github/scripts/ci_benchmark.py run --lane cluster --repetitions "$NL_REPETITIONS" --output "$NL_SUMMARY_DIRECTORY" ;;
  nix)
    # Keep the lock through the transfer: the next build prunes old archives.
    # stdout carries only the result bundle; build logs remain on stderr.
    python3 .github/scripts/nix_ci.py build --directory "$NL_SUMMARY_DIRECTORY" >&2
    exec tar -C "$NL_SUMMARY_DIRECTORY" -cf - nix.json closure.nar.gz
    ;;
  *) exit 2 ;;
esac
`;
const env = {...process.env, ...payload};
// The deadline includes waiting for the retained checkout's lock.
const result = spawnSync('timeout', ['--signal=TERM', '--kill-after=60', '9000', 'bash', '-euc', script],
                         {env, stdio: ['ignore','inherit','inherit']});
process.exit(result.status ?? 1);
"""


def credential_plugin(environ):
    """The command kubectl runs for a short-lived token. None is stored.

    The job already holds role credentials from GitHub's OIDC token, and the
    AWS CLI signs a token for this one cluster with them.
    """
    cluster = environ.get("NL_EKS_CLUSTER", "")
    region = environ.get("NL_AWS_REGION", "")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,99}", cluster) or not re.fullmatch(r"[a-z]{2}(-[a-z]+)+-\d", region):
        raise ValueError("Invalid cluster name or region")
    return {"apiVersion": "client.authentication.k8s.io/v1beta1", "interactiveMode": "Never",
            "command": "aws",
            "args": ["eks", "get-token", "--cluster-name", cluster, "--region", region, "--output", "json"]}


def kubeconfig(environ):
    server = environ["NL_GKE_SERVER"]
    if not server.startswith("https://"):
        raise ValueError("The cluster API must use TLS")
    return {"apiVersion": "v1", "kind": "Config", "current-context": "ci",
            "clusters": [{"name": "ci", "cluster": {"server": server,
                           "certificate-authority-data": environ["NL_GKE_CA_DATA"]}}],
            "contexts": [{"name": "ci", "context": {"cluster": "ci", "user": "ci",
                          "namespace": environ["NL_GKE_NAMESPACE"]}}],
            "users": [{"name": "ci", "user": {"exec": credential_plugin(environ)}}]}


def payload(environ, repetitions, task='bazel'):
    if (environ.get('GITHUB_REPOSITORY') != 'TraceMachina/nativelink'
            or environ.get('GITHUB_EVENT_NAME') not in ('push', 'workflow_dispatch')
            or not environ.get('GITHUB_REF', '').startswith('refs/heads/')):
        raise ValueError('Only trusted repository branch runs may use the retained client')
    if not re.fullmatch(r"[0-9a-f]{40}", environ["GITHUB_SHA"]):
        raise ValueError("Expected an exact source revision")
    run = environ["GITHUB_RUN_ID"] + "-" + environ["GITHUB_RUN_ATTEMPT"]
    if task not in ('bazel', 'nix'):
        raise ValueError('Unsupported retained client task')
    if not re.fullmatch(r"\d+-\d+", run) or (task == 'bazel' and not environ.get("NL_API_KEY")):
        raise ValueError("Expected a run identity and CI key")
    names = ("NL_API_KEY", "NL_CLUSTER_ENDPOINT", "NL_CLUSTER_BEP_ENDPOINT", "NL_INSTANCE",
             "NL_RESULTS_URL", "NL_WORKER_IMAGE", "GITHUB_SHA", "BENCHMARK_STARTED")
    if task == 'nix':
        names = ('GITHUB_SHA', 'BENCHMARK_STARTED')
    return {**{name: environ[name] for name in names}, "NL_PERSISTENT_CACHE": "/workspace",
            "CI_BAZEL_METADATA": environ.get("CI_BAZEL_METADATA", "{}"),
            "NL_REPETITIONS": str(repetitions), 'NL_TASK': task,
            "NL_SUMMARY_DIRECTORY": "/workspace/ci-results/" + run}


def receive_nix_result(command, build, environ, output):
    """Stream the manifest and closure in the same locked exec as the build."""
    import nix_ci

    output.mkdir(parents=True, exist_ok=True)
    # A failed retry must not leave a previous success available to consumers.
    (output / 'nix.json').unlink(missing_ok=True)
    (output / 'closure.nar.gz').unlink(missing_ok=True)
    with tempfile.TemporaryDirectory(prefix='.nix-transfer-', dir=output) as temporary:
        staging = Path(temporary)
        expected = {'nix.json': 64 * 1024, 'closure.nar.gz': 4 * 1024**3}
        received = set()
        with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              env=environ) as process:
            try:
                process.stdin.write(json.dumps(build).encode())
                process.stdin.close()
                with tarfile.open(fileobj=process.stdout, mode='r|') as bundle:
                    for member in bundle:
                        if (member.name not in expected or member.name in received
                                or not member.isfile() or member.size < 0
                                or member.size > expected[member.name]):
                            raise ValueError('Unexpected entry in the Nix result bundle')
                        with bundle.extractfile(member) as source, (staging / member.name).open('wb') as target:
                            shutil.copyfileobj(source, target)
                        received.add(member.name)
                # Drain tar padding and wait for kubectl's final exit status.
                while process.stdout.read(1024 * 1024):
                    pass
                if process.wait():
                    raise subprocess.CalledProcessError(process.returncode, command)
            except BaseException:
                process.kill()
                raise
        if received != set(expected):
            raise ValueError('Incomplete Nix result bundle')
        report = nix_ci.validate(json.loads((staging / 'nix.json').read_text()), environ['GITHUB_SHA'])
        archive = staging / 'closure.nar.gz'
        if archive.stat().st_size != report['bytes'] or nix_ci.digest(archive) != report['sha256']:
            raise ValueError('The transferred Nix closure failed checksum verification')
        archive.replace(output / archive.name)
        (output / 'nix.json').write_text(json.dumps(report, indent=2) + '\n')
    return report['exit_code']


def main(args):
    environ = os.environ.copy()
    build = payload(environ, args.repetitions, args.task)
    with tempfile.TemporaryDirectory(prefix="cluster-ci-") as temporary:
        config = Path(temporary) / "kubeconfig.json"
        # Public API address, CA and exec plugin path only. Tokens are obtained
        # in memory by the plugin for each command and are never written here.
        config.write_text(json.dumps(kubeconfig(environ)))
        environ["KUBECONFIG"] = str(config)
        base = ["kubectl", "--request-timeout=0", "-n", environ["NL_GKE_NAMESPACE"],
                "exec", environ["NL_GKE_POD"]]
        command = [*base, "-i", "--", "node", "--input-type=module", "-e", BOOTSTRAP]
        if args.task == 'nix':
            return receive_nix_result(command, build, environ, args.output)
        result = subprocess.run(command,
                                input=json.dumps(build), text=True, env=environ)
        filename = 'cluster.json'
        summary = subprocess.run([*base, "--", "cat", build["NL_SUMMARY_DIRECTORY"] + '/' + filename],
                                 text=True, capture_output=True, env=environ)
        if summary.returncode:
            sys.exit("The retained client did not produce benchmark evidence")
        report = json.loads(summary.stdout)
        if report["revision"] != environ["GITHUB_SHA"] or report["lane"] != "cluster":
            sys.exit("The retained client returned evidence for another source revision")
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / filename).write_text(json.dumps(report, indent=2) + "\n")
        return result.returncode or report["exit_code"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--task', choices=('bazel', 'nix'), default='bazel')
    parser.add_argument("--repetitions", type=int, choices=range(1, 21), default=3)
    parser.add_argument("--output", type=Path, required=True)
    raise SystemExit(main(parser.parse_args()))
