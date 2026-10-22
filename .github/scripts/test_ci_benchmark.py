"""Regression coverage for cache reconciliation and credential boundaries."""

import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import benchmark_credentials
import ci_benchmark
import cluster_ci
import cluster_credentials


class BenchmarkTest(unittest.TestCase):
    def test_dispatch_payload_excludes_other_job_credentials(self):
        env = {name: 'value' for name in ('NL_API_KEY', 'NL_CLUSTER_ENDPOINT', 'NL_CLUSTER_BEP_ENDPOINT',
               'NL_INSTANCE', 'NL_RESULTS_URL', 'NL_WORKER_IMAGE', 'BENCHMARK_STARTED')}
        env.update(GITHUB_SHA='a' * 40, GITHUB_RUN_ID='123', GITHUB_RUN_ATTEMPT='1',
                   GITHUB_TOKEN='never-forward', ACTIONS_ID_TOKEN_REQUEST_TOKEN='never-forward')
        result = cluster_ci.payload(env, 3)
        self.assertNotIn('never-forward', json.dumps(result))
        self.assertEqual(result['NL_SUMMARY_DIRECTORY'], '/workspace/ci-results/123-1')
        self.assertEqual(result['NL_PERSISTENT_CACHE'], '/workspace')

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


if __name__ == "__main__":
    unittest.main()
