"""Regression coverage for cache reconciliation and credential boundaries."""

import contextlib
import asyncio
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

import benchmark_credentials
import ci_benchmark
import cluster_ci
import cluster_credentials


class BenchmarkTest(unittest.TestCase):
    def test_primary_ci_dispatches_builds_without_a_hosted_baseline(self):
        workflow = Path('.github/workflows/parallel-cluster-ci.yaml').read_text()
        for hosted_build in ('matrix:', 'baseline', 'prepare-nix', 'setup-bazel', 'bazel test'):
            self.assertNotIn(hosted_build, workflow)
        self.assertEqual(workflow.count('python3 .github/scripts/cluster_ci.py'), 1)
        self.assertIn("inputs.repetitions || '1'", workflow)
        self.assertNotIn('group: cluster-ci-${{ github.ref }}', workflow)
        self.assertEqual(cluster_ci.SUPERVISOR, '/opt/ci/ci_supervisor.py')
        self.assertNotIn('  nix-bazel:', Path('.github/workflows/nix.yaml').read_text())
        pinned_version = Path('.bazelversion').read_text().strip()
        self.assertNotIn(f'bazel_version: {pinned_version}',
                         Path('.github/workflows/native-bazel.yaml').read_text())

    def test_bazel_and_nix_queue_for_the_same_retained_checkout(self):
        # A per-branch/workflow key lets both producers start and spend the
        # pod admission budget fighting for the same checkout lock.
        for filename, job in (('parallel-cluster-ci.yaml', 'cluster'), ('nix.yaml', 'nix-build')):
            with self.subTest(workflow=filename):
                workflow = Path('.github/workflows', filename).read_text()
                producer = workflow.split(f'  {job}:\n', 1)[1].split('    steps:', 1)[0]
                self.assertIn('    concurrency:\n      group: retained-cluster-client\n'
                              '      cancel-in-progress: false\n      queue: max\n', producer)

    def test_cluster_summary_preserves_failures_and_unknown_cache_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'cluster.json'
            result = {'revision': 'a' * 40, 'total_seconds': 12, 'runs': [{
                'iteration': 1, 'seconds': 10, 'exit_code': 7,
                'cache': ci_benchmark.cache_usage({'runnerCount': [
                    {'name': 'total', 'count': 10}, {'name': 'remote cache hit', 'count': 2}]}),
                'tests': {'cached': 2, 'total': 3, 'failed': 1}}]}
            path.write_text(json.dumps(result))
            with contextlib.redirect_stdout(io.StringIO()) as output:
                ci_benchmark.summarize(path)
            self.assertIn('Not captured', output.getvalue())
            self.assertIn('Unknown', output.getvalue())
            self.assertIn('2/3 | failed', output.getvalue())

    def test_docs_lint_does_not_depend_on_a_floating_vale_release(self):
        workflow = Path('.github/workflows/vale.yaml').read_text()
        action = workflow.split('errata-ai/vale-action@', 1)[1]
        self.assertRegex(action, r'(?m)^\s+version: ["\x27]?\d+\.\d+\.\d+["\x27]?\s*$')

    def test_dispatch_payload_excludes_other_job_credentials(self):
        env = {name: 'value' for name in ('NL_API_KEY', 'NL_CLUSTER_ENDPOINT', 'NL_CLUSTER_BEP_ENDPOINT',
               'NL_INSTANCE', 'NL_RESULTS_URL', 'NL_WORKER_IMAGE', 'BENCHMARK_STARTED')}
        env.update(GITHUB_SHA='a' * 40, GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='1',
                   GITHUB_REPOSITORY='TraceMachina/nativelink', GITHUB_EVENT_NAME='push',
                   GITHUB_REF='refs/heads/feature/example',
                   GITHUB_TOKEN='never-forward', ACTIONS_ID_TOKEN_REQUEST_TOKEN='never-forward')
        result = cluster_ci.payload(env, 3)
        self.assertNotIn('never-forward', json.dumps(result))
        self.assertEqual(result['NL_SUMMARY_DIRECTORY'], '/workspace/ci-results/123-1')
        self.assertEqual(result['NL_PERSISTENT_CACHE'], '/workspace')
        for change in ({'GITHUB_REPOSITORY': 'outside/nativelink'},
                       {'GITHUB_EVENT_NAME': 'pull_request'},
                       {'GITHUB_EVENT_NAME': 'pull_request_target'},
                       {'GITHUB_REF': 'refs/pull/1/merge'}, {'GITHUB_REF': 'refs/tags/v1'}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                cluster_ci.payload({**env, **change}, 3)

    def test_federation_keeps_tokens_out_of_kubeconfig(self):
        env = {'NL_GCP_PROVIDER': 'projects/123/locations/global/workloadIdentityPools/ci/providers/github',
               'NL_GCP_SERVICE_ACCOUNT': 'ci-test@example.iam.gserviceaccount.com',
               'ACTIONS_ID_TOKEN_REQUEST_URL': 'https://example.actions.githubusercontent.com/token?x=1',
               'ACTIONS_ID_TOKEN_REQUEST_TOKEN': 'request-secret', 'NL_GKE_SERVER': 'https://cluster.example',
               'NL_GKE_CA_DATA': 'public-ca', 'NL_GKE_NAMESPACE': 'test'}
        responses = [{'value': 'identity-secret'}, {'access_token': 'sts-secret'},
                     {'accessToken': 'short-lived-secret', 'expireTime': '2026-10-22T23:00:00Z'}]
        with patch.object(cluster_credentials, 'request_json', side_effect=responses) as request:
            result = cluster_credentials.credential(env)
        self.assertEqual(result['status']['token'], 'short-lived-secret')
        self.assertIn('audience=', request.call_args_list[0].args[0])
        self.assertEqual(request.call_args_list[1].args[2]['subjectToken'], 'identity-secret')
        self.assertIn('https://www.googleapis.com/auth/userinfo.email',
                      request.call_args_list[2].args[2]['scope'])
        config = json.dumps(cluster_ci.kubeconfig(env))
        for secret in ('request-secret', 'identity-secret', 'sts-secret', 'short-lived-secret'):
            self.assertNotIn(secret, config)
        self.assertNotIn('token', cluster_ci.kubeconfig(env)['users'][0]['user'])

    def test_oidc_rejects_untrusted_token_destinations(self):
        env = {'NL_GCP_PROVIDER': 'projects/123/locations/global/workloadIdentityPools/ci/providers/github',
               'NL_GCP_SERVICE_ACCOUNT': 'ci-test@example.iam.gserviceaccount.com',
               'ACTIONS_ID_TOKEN_REQUEST_URL': 'https://evil.example/token'}
        with patch.object(cluster_credentials, 'request_json') as request, self.assertRaises(ValueError):
            cluster_credentials.credential(env)
        request.assert_not_called()

    def test_total_reuse_counts_local_hits_once_and_uses_spawn_outcomes(self):
        # One action may spawn multiple processes; total-internal is not a
        # valid denominator. Test-result hits must not be added again either.
        summary = {"actionCacheStatistics": {"hits": 95}, "runnerCount": [
            {"name": "total", "count": 4}, {"name": "internal", "count": 1},
            {"name": "action cache hit", "count": 95},
            {"name": "disk cache hit", "count": 2},
            {"name": "remote cache hit", "count": 1}, {"name": "remote", "count": 2}]}
        self.assertEqual(ci_benchmark.cache_usage(summary)["total_reuse_percent"], 98)

    def test_unknown_local_coverage_does_not_fabricate_total_reuse(self):
        usage = ci_benchmark.cache_usage({"runnerCount": [
            {"name": "total", "count": 10}, {"name": "remote cache hit", "count": 2}]})
        self.assertEqual(usage["remote_hits"], 2)
        self.assertIsNone(usage["total_reuse_percent"])

    def test_bep_artifact_contains_only_aggregate_allowlist(self):
        with tempfile.TemporaryDirectory() as directory:
            bep = Path(directory) / "events"
            bep.write_text(json.dumps({"optionsParsed": {"secret": "do-not-retain"}}) + "\n" +
                           json.dumps({"testSummary": {"overallStatus": "PASSED", "totalRunCount": 1,
                                                       "totalNumCached": 1, "label": "private"}}) + "\n")
            result = ci_benchmark.aggregates(bep)
            self.assertEqual(result["tests"], {"total": 1, "cached": 1, "failed": 0})
            self.assertNotIn("private", json.dumps(result))
            self.assertNotIn("do-not-retain", json.dumps(result))

    def test_helper_never_sends_key_to_other_hosts_ports_or_cleartext(self):
        endpoint = "grpcs://cache.example:443"
        for uri in ["https://evil.example", "https://cache.example.evil.example",
                    "https://cache.example:444", "http://cache.example"]:
            self.assertEqual(benchmark_credentials.credentials(uri, endpoint, "secret"), {})
        self.assertEqual(benchmark_credentials.credentials("https://cache.example", endpoint, "secret"),
                         {"headers": {"x-nativelink-api-key": ["secret"]}})

    def test_missing_key_fails_without_echoing_request(self):
        helper = Path(__file__).with_name("benchmark_credentials.py")
        result = subprocess.run([sys.executable, str(helper), "get"],
                                input='{"uri":"https://cache.example/private"}', text=True,
                                capture_output=True, env={**os.environ, "NL_ENDPOINTS": "grpcs://cache.example",
                                                         "NL_API_KEY": ""})
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertNotIn("private", result.stderr)

    def test_configuration_keeps_key_out_of_argv_and_caps_cluster_jobs(self):
        env = {"NL_API_KEY": "do-not-write-me", "GITHUB_SHA": "a" * 40,
               "NL_CLUSTER_ENDPOINT": "grpcs://cache.example:443",
               "NL_CLUSTER_BEP_ENDPOINT": "grpcs://cache.example:443",
               "NL_INSTANCE": "main", "NL_RESULTS_URL": "https://ui.example/build/",
               "NL_WORKER_IMAGE": "image@sha256:123"}
        flags, endpoints = ci_benchmark.configuration("cluster", env)
        self.assertNotIn(env["NL_API_KEY"], " ".join(flags) + endpoints)
        self.assertGreater(flags.index("--jobs=48"), flags.index("--config=nl-rbe"))
        with self.assertRaises(ValueError):
            ci_benchmark.configuration("cluster", {**env, "NL_API_KEY": ""})
        with self.assertRaises(ValueError):
            ci_benchmark.configuration("baseline", env)

    def test_failure_is_retained_and_stops_warm_repetitions(self):
        with tempfile.TemporaryDirectory() as directory:
            args = type("Args", (), {"lane": "cluster", "repetitions": 3, "output": Path(directory)})()
            with patch.dict(os.environ, {"GITHUB_SHA": "a" * 40, "BENCHMARK_STARTED": "0"}), \
                    patch.object(ci_benchmark, "configuration", return_value=([], "")), \
                    patch.object(ci_benchmark, "execute", return_value=7) as execute:
                self.assertEqual(ci_benchmark.run(args), 7)
                execute.assert_called_once()
            result = json.loads((Path(directory) / "cluster.json").read_text())
            self.assertEqual(result["exit_code"], 7)
            self.assertEqual(len(result["runs"]), 1)

    def test_host_bazel_does_not_inherit_the_nix_library_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = {
                'nix': '#!/bin/sh\nwhile [ "$1" != --command ]; do shift; done\nshift\n'
                       'export LD_LIBRARY_PATH=/incompatible/nix-runtime\nexec "$@"\n',
                'bazel': '#!/bin/sh\nif [ -n "${LD_LIBRARY_PATH+x}" ]; then exit 81; fi\n',
            }
            for name, script in scripts.items():
                path = root / name
                path.write_text(script)
                path.chmod(0o755)
            args = type('Args', (), {'lane': 'cluster', 'repetitions': 1, 'output': root})()
            with patch.dict(os.environ, {'PATH': str(root) + os.pathsep + os.environ['PATH'],
                                         'NL_API_KEY': 'test-only-key', 'GITHUB_SHA': 'a' * 40,
                                         'BENCHMARK_STARTED': '0'}), \
                    patch.object(ci_benchmark, 'configuration', return_value=([], '')):
                self.assertEqual(ci_benchmark.run(args), 0)

    def test_persistent_build_fails_fast_on_bazel_lock_and_keeps_stable_root(self):
        with tempfile.TemporaryDirectory() as directory:
            args = type('Args', (), {'lane': 'cluster', 'repetitions': 1, 'output': Path(directory)})()
            with patch.dict(os.environ, {'GITHUB_SHA': 'a' * 40, 'BENCHMARK_STARTED': '0',
                                         'NL_PERSISTENT_CACHE': '/workspace'}), \
                    patch.object(ci_benchmark, 'configuration', return_value=([], '')), \
                    patch.object(ci_benchmark, 'execute', return_value=9) as execute:
                self.assertEqual(ci_benchmark.run(args), 9)
            command = execute.call_args.args[0]
            self.assertIn('--output_user_root=/workspace/bazel-ci', command)
            self.assertLess(command.index('--noblock_for_lock'), command.index('test'))

    def test_busy_dispatch_produces_failure_evidence_without_fetching_stale_results(self):
        env = {name: 'value' for name in ('NL_API_KEY', 'NL_CLUSTER_ENDPOINT', 'NL_CLUSTER_BEP_ENDPOINT',
               'NL_INSTANCE', 'NL_RESULTS_URL', 'NL_WORKER_IMAGE', 'BENCHMARK_STARTED')}
        env.update(GITHUB_SHA='a' * 40, GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='1',
                   GITHUB_REPOSITORY='TraceMachina/nativelink', GITHUB_EVENT_NAME='push',
                   GITHUB_REF='refs/heads/example', NL_GKE_SERVER='https://cluster.example',
                   NL_GKE_CA_DATA='public', NL_GKE_NAMESPACE='example', NL_GKE_POD='client-0')
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, env), \
                patch.object(cluster_ci, 'dispatch', new_callable=AsyncMock) as dispatch, \
                patch.object(cluster_ci.subprocess, 'run') as fetch:
            dispatch.return_value = {'reason': 'busy', 'exit_code': 75}
            args = type('Args', (), {'task': 'bazel', 'repetitions': 1, 'output': Path(directory)})()
            self.assertEqual(cluster_ci.main(args), 75)
            fetch.assert_not_called()
            report = json.loads((args.output / 'cluster.json').read_text())
            self.assertEqual(report['exit_code'], 75)
            self.assertEqual(report['runs'], [])
            with contextlib.redirect_stdout(io.StringIO()) as output:
                ci_benchmark.summarize(args.output / 'cluster.json')
            self.assertIn('busy (exit 75)', output.getvalue())

    def test_comparison_requires_both_lanes_and_matching_revisions(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(ValueError):
                ci_benchmark.compare(Path(directory))
            base = {"revision": "a" * 40, "setup_seconds": 1, "total_seconds": 3,
                    "exit_code": 0, "runs": [{"seconds": 2, "exit_code": 0}]}
            for lane in ("baseline", "cluster"):
                (Path(directory) / f"{lane}.json").write_text(json.dumps({
                    **base, "lane": lane, "revision": lane * 10}))
            with self.assertRaises(ValueError):
                ci_benchmark.compare(Path(directory))


class DispatchTest(unittest.IsolatedAsyncioTestCase):
    async def test_old_pod_reports_missing_supervisor_without_dumping_diagnostics(self):
        with tempfile.TemporaryDirectory() as directory:
            python = Path(directory) / 'python3'
            python.write_text('#!/bin/sh\necho "private diagnostic" >&2\nexit 2\n')
            python.chmod(0o755)
            env = {**os.environ, 'PATH': directory + os.pathsep + os.environ['PATH']}
            with contextlib.redirect_stdout(io.StringIO()) as output:
                result = await cluster_ci.dispatch(
                    ['bash', '-c', cluster_ci.BRIDGE],
                    {'GITHUB_SHA': 'a' * 40, 'NL_TASK': 'bazel'}, env, deadline=2)
            self.assertEqual(result, {'reason': 'supervisor_unavailable', 'exit_code': 69})
            self.assertNotIn('private diagnostic', output.getvalue())

    async def test_heartbeats_originate_at_dispatcher_and_logs_hide_key(self):
        script = '''import json,sys
request=json.loads(sys.stdin.readline())
assert request['protocol']==1
for _ in range(2):
 assert json.loads(sys.stdin.readline())=={'heartbeat':1}
print(json.dumps({'log':request['build']['NL_API_KEY']}), flush=True)
print(json.dumps({'result':{'exit_code':0,'reason':'completed'}}), flush=True)
'''
        build = {'GITHUB_SHA': 'a' * 40, 'NL_TASK': 'bazel', 'NL_API_KEY': 'test-only-key'}
        with contextlib.redirect_stdout(io.StringIO()) as output:
            result = await cluster_ci.dispatch([sys.executable, '-c', script], build,
                                                os.environ.copy(), heartbeat=0.05, deadline=2)
        self.assertEqual(result, {'reason': 'completed', 'exit_code': 0})
        self.assertNotIn('test-only-key', output.getvalue())
        self.assertIn('[REDACTED]', output.getvalue())

    async def test_lost_exec_stream_is_a_distinct_failure(self):
        result = await cluster_ci.dispatch([sys.executable, '-c', 'pass'],
                                           {'GITHUB_SHA': 'a' * 40, 'NL_TASK': 'bazel'},
                                           os.environ.copy(), heartbeat=0.05, deadline=2)
        self.assertEqual(result, {'reason': 'transport_lost', 'exit_code': 74})


if __name__ == "__main__":
    unittest.main()
