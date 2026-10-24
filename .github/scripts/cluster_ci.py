#!/usr/bin/env python3
"""Dispatch trusted CI to one retained client using narrowly scoped pod exec."""

import argparse
import asyncio
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time


# The supervisor is installed independently of the checkout. Credentials and
# heartbeats travel only over encrypted stdin; the pod bridge cannot renew a
# lease after the GitHub dispatcher disappears.
SUPERVISOR = '/opt/ci/ci_supervisor.py'


async def dispatch(command, build, environ, heartbeat=10, deadline=9600):
    process = await asyncio.create_subprocess_exec(
        *command, env=environ, stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL,
        limit=1024 * 1024)
    cancelled = asyncio.Event()
    loop = asyncio.get_running_loop()
    previous_signals = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, cancelled.set)
    result = {'exit_code': 74, 'reason': 'transport_lost'}

    async def send(message):
        process.stdin.write(json.dumps(message).encode() + b'\n')
        await asyncio.wait_for(process.stdin.drain(), 5)

    async def renew():
        while True:
            await asyncio.sleep(heartbeat)
            await send({'heartbeat': 1})

    async def receive():
        while line := await process.stdout.readline():
            message = json.loads(line)
            if 'log' in message:
                text = message['log']
                if build.get('NL_API_KEY'):
                    text = text.replace(build['NL_API_KEY'], '[REDACTED]')
                print(text, end='', flush=True)
            elif 'state' in message:
                if message['state'] in ('queued', 'running'):
                    print('Retained CI: ' + message['state'], flush=True)
            elif 'result' in message:
                value = message['result']
                reasons = {'completed', 'cancelled', 'lease_expired', 'deadline_exceeded',
                           'transport_lost', 'busy', 'duplicate_run', 'invalid_request',
                           'pod_stopping', 'recovery_failed'}
                if (value.get('reason') not in reasons
                        or type(value.get('exit_code')) is not int
                        or value.get('revision', build['GITHUB_SHA']) != build['GITHUB_SHA']
                        or value.get('task', build['NL_TASK']) != build['NL_TASK']):
                    raise ValueError('Invalid supervisor outcome')
                return {'exit_code': value['exit_code'], 'reason': value['reason']}
        return {'exit_code': 74, 'reason': 'transport_lost'}

    tasks = []
    try:
        await send({'protocol': 1, 'build': build})
        renewer = asyncio.create_task(renew())
        receiver = asyncio.create_task(receive())
        cancellation = asyncio.create_task(cancelled.wait())
        tasks = [renewer, receiver, cancellation]
        done, _ = await asyncio.wait(tasks, timeout=deadline,
                                     return_when=asyncio.FIRST_COMPLETED)
        if receiver in done:
            result = receiver.result()
        else:
            result = {'exit_code': 130 if cancellation in done else 124,
                      'reason': 'cancelled' if cancellation in done else 'transport_lost'}
            with contextlib.suppress(ConnectionError, asyncio.TimeoutError):
                await send({'cancel': 1})
    except (OSError, ValueError, asyncio.TimeoutError):
        result = {'exit_code': 74, 'reason': 'transport_lost'}
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        process.stdin.close()
        with contextlib.suppress(asyncio.TimeoutError):
            await asyncio.wait_for(process.wait(), 5)
        if process.returncode is None:
            process.kill()
            await process.wait()
        for sig in (signal.SIGINT, signal.SIGTERM):
            loop.remove_signal_handler(sig)
            signal.signal(sig, previous_signals[sig])
    return result


def failure_report(build, result, started):
    elapsed = time.monotonic() - started
    return {'schema': 1, 'lane': 'cluster', 'revision': build['GITHUB_SHA'],
            'workload': 'bazel test //...', 'setup_seconds': None,
            'build_test_seconds': None, 'total_seconds': elapsed,
            'runs': [], **result}


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
            "NL_REPETITIONS": str(repetitions), 'NL_TASK': task,
            "NL_SUMMARY_DIRECTORY": "/workspace/ci-results/" + run}


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
        started = time.monotonic()
        args.output.mkdir(parents=True, exist_ok=True)
        interrupted = {'exit_code': 74, 'reason': 'dispatch_interrupted'}
        if args.task == 'bazel':
            (args.output / 'cluster.json').write_text(json.dumps(
                failure_report(build, interrupted, started), indent=2) + '\n')
        (args.output / 'dispatch.json').write_text(json.dumps({
            'revision': build['GITHUB_SHA'], 'task': args.task, **interrupted}, indent=2) + '\n')
        result = asyncio.run(dispatch(
            [*base, '-i', '--', 'python3', SUPERVISOR, 'bridge'], build, environ))
        (args.output / 'dispatch.json').write_text(json.dumps({
            'revision': build['GITHUB_SHA'], 'task': args.task, **result}, indent=2) + '\n')
        print('Retained CI outcome: ' + result['reason'], flush=True)
        if args.task == 'nix' and result['exit_code']:
            return result['exit_code']
        filename = 'nix.json' if args.task == 'nix' else 'cluster.json'
        summary = None
        if result['reason'] == 'completed':
            with contextlib.suppress(subprocess.TimeoutExpired):
                summary = subprocess.run([
                    *base, "--", "cat", build["NL_SUMMARY_DIRECTORY"] + '/' + filename],
                    text=True, capture_output=True, env=environ, timeout=30)
        if summary is None or summary.returncode:
            # Cancellation, queue timeout and connection loss still produce
            # evidence. Never turn a missing JSON file into the primary error.
            result['exit_code'] = result['exit_code'] or 74
            if args.task == 'bazel':
                (args.output / filename).write_text(json.dumps(
                    failure_report(build, result, started), indent=2) + '\n')
            return result['exit_code']
        report = json.loads(summary.stdout)
        if args.task == 'nix':
            import nix_ci
            report = nix_ci.validate(report, environ['GITHUB_SHA'])
        elif report["revision"] != environ["GITHUB_SHA"] or report["lane"] != "cluster":
            sys.exit("The retained client returned evidence for another source revision")
        args.output.mkdir(parents=True, exist_ok=True)
        if args.task == 'nix':
            archive = args.output / 'closure.nar.gz'
            with archive.open('wb') as destination:
                subprocess.run([*base, '--', 'cat', build['NL_SUMMARY_DIRECTORY'] + '/closure.nar.gz'],
                               stdout=destination, env=environ, check=True)
            with archive.open('rb') as stream:
                checksum = hashlib.file_digest(stream, 'sha256').hexdigest()
            if archive.stat().st_size != report['bytes'] or checksum != report['sha256']:
                archive.unlink()
                raise ValueError('The transferred Nix closure failed checksum verification')
        (args.output / filename).write_text(json.dumps(report, indent=2) + "\n")
        return result["exit_code"] or report["exit_code"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--task', choices=('bazel', 'nix'), default='bazel')
    parser.add_argument("--repetitions", type=int, choices=range(1, 21), default=3)
    parser.add_argument("--output", type=Path, required=True)
    raise SystemExit(main(parser.parse_args()))
