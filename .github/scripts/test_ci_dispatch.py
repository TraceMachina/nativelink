"""A broken exec reply must not restart or lose the owned build."""
import contextlib
import io
import hashlib
import tempfile
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import cluster_ci


class DispatchTest(unittest.TestCase):
    def build(self):
        return {'GITHUB_SHA': 'a' * 40, 'NL_TASK': 'bazel', 'NL_API_KEY': 'test-secret'}

    def test_lost_submission_reply_reconnects_with_same_owner_and_cursor(self):
        requests = []
        responses = [ConnectionError(),
                     {'protocol': 2, 'cursor': 1, 'events': [{'log': 'test-secret\n'}]},
                     subprocess.TimeoutExpired('transport', 15),
                     {'protocol': 2, 'cursor': 2, 'events': [],
                      'result': {'reason': 'completed', 'exit_code': 0}}]

        def exchange(command, request, environ):
            requests.append(json.loads(json.dumps(request)))
            response = responses.pop(0)
            if isinstance(response, Exception):
                raise response
            return response

        with contextlib.redirect_stdout(io.StringIO()) as out:
            result = cluster_ci.dispatch([], self.build(), {}, poll_seconds=0, exchange_fn=exchange)
        self.assertEqual(result, {'reason': 'completed', 'exit_code': 0})
        self.assertEqual(len({r['token'] for r in requests}), 1)
        self.assertEqual([r['cursor'] for r in requests], [0, 0, 1, 1])
        self.assertNotIn('test-secret', out.getvalue())
        self.assertIn('[REDACTED]', out.getvalue())

    def test_prolonged_outage_returns_distinct_failure(self):
        with patch.object(cluster_ci, 'exchange', side_effect=ConnectionError()) as exchange:
            result = cluster_ci.dispatch([], self.build(), {}, poll_seconds=0, exchange_fn=exchange)
        self.assertEqual(result, {'reason': 'transport_lost', 'exit_code': 74})
        self.assertEqual(exchange.call_count, 3)

    def test_different_revision_cannot_report_success(self):
        def exchange(*args):
            return {'protocol': 2, 'cursor': 0, 'result': {
                'reason': 'completed', 'exit_code': 0, 'revision': 'b' * 40}}
        self.assertEqual(cluster_ci.dispatch([], self.build(), {}, exchange_fn=exchange)['exit_code'], 74)

    def test_bazel_and_nix_have_distinct_pods_and_admission_queues(self):
        for filename, job, task in [('parallel-cluster-ci.yaml', 'cluster', 'bazel'),
                                    ('nix.yaml', 'nix-build', 'nix')]:
            workflow = Path('.github/workflows', filename).read_text()
            producer = workflow.split('  ' + job + ':\n', 1)[1].split('    steps:', 1)[0]
            self.assertIn('group: retained-ci-' + task, producer)
            self.assertIn('queue: max', producer)
            self.assertIn('NL_GKE_' + task.upper() + '_POD', workflow)
            self.assertNotIn('vars.NL_GKE_POD', workflow)

    def test_transport_arguments_do_not_contain_credentials(self):
        request = {'token': 'owner-secret', 'build': self.build()}
        with patch.object(cluster_ci.subprocess, 'run') as run:
            run.return_value.returncode = 0
            run.return_value.stdout = '{"protocol":2}'
            cluster_ci.exchange(['kubectl', 'exec'], request, {})
        self.assertNotIn('secret', repr(run.call_args.args))
        self.assertIn('owner-secret', run.call_args.kwargs['input'])
        self.assertEqual(run.call_args.kwargs['timeout'], 15)


class ArchiveTest(unittest.TestCase):
    def test_partial_chunk_retries_same_offset_without_duplicate_bytes(self):
        data = b'abcdefghi'
        report = {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
        offsets = []
        def read(command, **kwargs):
            offset, count = map(int, command[-2:])
            offsets.append(offset)
            content = data[offset:offset + count]
            if len(offsets) == 2:
                content = content[:1]
            return subprocess.CompletedProcess(command, 0, stdout=content)
        with tempfile.TemporaryDirectory() as directory, patch.object(cluster_ci.subprocess, 'run', side_effect=read):
            archive = Path(directory) / 'archive'
            cluster_ci.download_archive([], '/result', archive, report, {}, chunk_bytes=3)
            self.assertEqual(archive.read_bytes(), data)
        self.assertEqual(offsets, [0, 3, 3, 6])

    def test_corrupt_complete_transfer_is_rejected(self):
        report = {'bytes': 3, 'sha256': hashlib.sha256(b'abc').hexdigest()}
        response = subprocess.CompletedProcess([], 0, stdout=b'xyz')
        with tempfile.TemporaryDirectory() as directory, patch.object(cluster_ci.subprocess, 'run', return_value=response):
            archive = Path(directory) / 'archive'
            with self.assertRaisesRegex(ValueError, 'checksum'):
                cluster_ci.download_archive([], '/result', archive, report, {})
            self.assertFalse(archive.exists())
