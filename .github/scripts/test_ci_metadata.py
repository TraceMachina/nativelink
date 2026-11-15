"""Job identity, timestamp accuracy and rc injection regression coverage."""

import contextlib
import io
import json
import os
from pathlib import Path
import shlex
import tempfile
import unittest
from unittest.mock import patch

import ci_metadata


class MetadataTest(unittest.TestCase):
    def setUp(self):
        self.env = {'GITHUB_WORKFLOW': 'Build and test', 'GITHUB_JOB': 'test',
                    'GITHUB_REPOSITORY': 'example/project', 'GITHUB_RUN_ID': '123',
                    'GITHUB_RUN_ATTEMPT': '2', 'GITHUB_HEAD_REF': 'feature/example',
                    'GITHUB_REF_NAME': '17/merge', 'RUNNER_NAME': 'private-machine',
                    'RUNNER_ENVIRONMENT': 'github-hosted', 'RUNNER_OS': 'Linux',
                    'RUNNER_ARCH': 'X64', 'GITHUB_TOKEN': 'never-publish-token',
                    'UNRELATED_SECRET': 'never-publish-secret'}
        self.job = {'name': 'Tests / Linux', 'status': 'in_progress',
                    'runner_name': 'private-machine', 'labels': ['private-network'],
                    'created_at': '2026-10-27T12:00:00Z',
                    'started_at': '2026-10-27T12:00:05Z'}

    def test_github_job_times_and_public_identity(self):
        def request(url, token):
            self.assertIn('/runs/123/attempts/2/jobs?', url)
            self.assertEqual(token, self.env['GITHUB_TOKEN'])
            return {'jobs': [self.job], 'total_count': 1}
        values = ci_metadata.collect(self.env, 'a' * 40, request)
        self.assertEqual(values, {'pipeline': 'Build and test', 'job': 'Tests / Linux',
                                 'runner': 'github-hosted/Linux/X64',
                                 'commit': 'a' * 40, 'branch': 'feature/example',
                                 'job_created': '2026-10-27T12:00:00Z',
                                 'job_started': '2026-10-27T12:00:05Z'})
        for private in ('private-machine', 'private-network', 'never-publish'):
            self.assertNotIn(private, json.dumps(values))

    def test_matrix_job_lookup_uses_current_runner_and_attempt(self):
        other = {**self.job, 'runner_name': 'another-runner'}
        pages = [{'jobs': [other] * 100, 'total_count': 101},
                 {'jobs': [self.job], 'total_count': 101}]
        urls = []
        def request(url, _token):
            urls.append(url)
            return pages.pop(0)
        self.assertEqual(ci_metadata.current_job(self.env, request), self.job)
        self.assertTrue(urls[-1].endswith('page=2'))

    def test_missing_or_ambiguous_job_never_invents_timing(self):
        for jobs in ([], [self.job, self.job], [{**self.job, 'status': 'completed'}]):
            with self.subTest(jobs=len(jobs)), contextlib.redirect_stdout(io.StringIO()):
                values = ci_metadata.collect(self.env, 'a' * 40,
                    lambda *_: {'jobs': jobs, 'total_count': len(jobs)})
                self.assertNotIn('job_created', values)
                self.assertNotIn('job_started', values)
                self.assertEqual(values['job'], 'test')

    def test_api_failure_is_optional_and_does_not_print_error_secrets(self):
        def request(*_):
            raise OSError('never-publish-token')
        with contextlib.redirect_stdout(io.StringIO()) as output:
            values = ci_metadata.collect(self.env, 'b' * 40, request)
        self.assertNotIn('never-publish', output.getvalue())
        self.assertNotIn('job_started', values)
        self.assertEqual(values['commit'], 'b' * 40)

    def test_timestamps_are_independent_utc_values_not_step_time(self):
        job = {**self.job, 'created_at': None, 'started_at': '2026-10-27T13:00:05+01:00'}
        values = ci_metadata.collect(self.env, 'a' * 40,
            lambda *_: {'jobs': [job], 'total_count': 1})
        self.assertNotIn('job_created', values)
        self.assertEqual(values['job_started'], '2026-10-27T12:00:05Z')
        for invalid in ('yesterday', '2026-10-27T12:00:00', None):
            self.assertIsNone(ci_metadata.timestamp(invalid))

    def test_push_branch_and_checked_out_commit(self):
        env = {**self.env, 'GITHUB_HEAD_REF': '', 'GITHUB_REF_NAME': 'main',
               'GITHUB_SHA': 'b' * 40}
        values = ci_metadata.collect(env, 'a' * 40,
            lambda *_: {'jobs': [self.job], 'total_count': 1})
        self.assertEqual(values['commit'], 'a' * 40)
        self.assertEqual(values['branch'], 'main')

    def test_rc_quoting_and_allowlist_reject_flag_injection(self):
        tricky = 'Build "quotes" and apostrophe\'s #hash \\path --remote_header=not-a-flag'
        with tempfile.TemporaryDirectory() as directory:
            ci_metadata.write_rc(directory, {'pipeline': tricky, 'job': 'unsafe\nstartup --bad',
                                            'branch': 'unsafe\r--bad', 'GITHUB_TOKEN': 'secret'})
            text = (Path(directory) / ci_metadata.FILENAME).read_text()
            self.assertEqual(shlex.split(text), ['build', '--build_metadata=pipeline=' + tricky])
            self.assertEqual(len(text.splitlines()), 1)
            self.assertNotIn('secret', text)

    def test_retained_checkout_replaces_and_clears_previous_job(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ci_metadata.FILENAME
            path.write_text('build --build_metadata=job=previous\n')
            ci_metadata.restore(directory, {'CI_BAZEL_METADATA': json.dumps({'job': 'current'})})
            self.assertEqual(shlex.split(path.read_text()), ['build', '--build_metadata=job=current'])
            for env in ({}, {'CI_BAZEL_METADATA': 'invalid'}, {'CI_BAZEL_METADATA': '[]'}):
                ci_metadata.restore(directory, env)
                self.assertEqual(path.read_text(), '')

    def test_entrypoint_exports_only_metadata_to_the_next_step(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / 'github-env'
            with patch.dict(os.environ, {**self.env, 'GITHUB_ENV': str(destination)}, clear=True), \
                    patch.object(ci_metadata.subprocess, 'check_output', return_value='a' * 40), \
                    patch.object(ci_metadata, 'collect', return_value={'pipeline': 'Build test'}), \
                    patch.object(ci_metadata.Path, 'cwd', return_value=Path(directory)):
                ci_metadata.main()
            name, payload = destination.read_text().strip().split('=', 1)
            self.assertEqual(name, 'CI_BAZEL_METADATA')
            self.assertEqual(json.loads(payload), {'pipeline': 'Build test'})


if __name__ == '__main__':
    unittest.main()
