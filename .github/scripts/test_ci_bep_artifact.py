"""Bounds and selection for the staged PR build-event artifact."""

import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import ci_bep_artifact

PASSED = '<testsuites><testsuite tests="1"><testcase name="t" status="run"/></testsuite></testsuites>'
FAILED = ('<testsuites><testsuite tests="1" errors="1"><testcase name="t" status="run">'
          '<error message="exited with error code 101"/></testcase></testsuite></testsuites>')


class CollectTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.events = self.root / 'events.pb'
        self.testlogs = self.root / 'testlogs'
        self.output = self.root / 'out'

    def tearDown(self):
        self.directory.cleanup()

    def target(self, name, report, log='log line\n'):
        directory = self.testlogs / name
        directory.mkdir(parents=True)
        (directory / 'test.xml').write_text(report)
        (directory / 'test.log').write_text(log)
        return directory

    def collect(self, environ=None):
        return ci_bep_artifact.collect(self.events, self.testlogs, self.output, environ or {})

    def staged(self):
        return sorted(path.relative_to(self.output).as_posix()
                      for path in self.output.rglob('*') if path.is_file())

    def test_stages_events_and_only_failed_test_logs(self):
        self.events.write_bytes(b'\x01\x02')
        self.target('pkg/passing_test', PASSED)
        self.target('pkg/failing_test', FAILED)
        manifest = self.collect()
        self.assertEqual(self.staged(), ['events.pb', 'manifest.json',
                                         'testlogs/pkg/failing_test/test.log',
                                         'testlogs/pkg/failing_test/test.xml'])
        self.assertEqual(manifest['failed_targets'], 1)
        on_disk = json.loads((self.output / 'manifest.json').read_text())
        self.assertEqual(on_disk, manifest)

    def test_unreadable_report_counts_as_failed(self):
        self.target('pkg/crashed_test', '<testsuites><testsu')
        self.collect()
        self.assertIn('testlogs/pkg/crashed_test/test.log', self.staged())

    def test_missing_events_and_testlogs_still_write_a_manifest(self):
        manifest = self.collect()
        self.assertEqual(self.staged(), ['manifest.json'])
        self.assertEqual(manifest['skipped'], [{'path': 'events.pb', 'reason': 'not produced'}])

    def test_oversized_events_are_left_out_rather_than_truncated(self):
        self.events.write_bytes(b'x' * 11)
        with patch.object(ci_bep_artifact, 'MAX_EVENTS_BYTES', 10):
            manifest = self.collect()
        self.assertNotIn('events.pb', self.staged())
        self.assertIn('exceeds the cap', manifest['skipped'][0]['reason'])

    def test_events_with_a_credential_like_environment_are_withheld(self):
        self.events.write_bytes(b'\x0a--client_env=PATH=/usr/bin\x12'
                                b'--client_env=ACTIONS_RUNTIME_TOKEN=leaked\x12')
        manifest = self.collect()
        self.assertNotIn('events.pb', self.staged())
        reason = manifest['skipped'][0]['reason']
        self.assertIn('ACTIONS_RUNTIME_TOKEN', reason)
        self.assertNotIn('leaked', (self.output / 'manifest.json').read_text())

    def test_public_cache_header_alone_does_not_withhold_events(self):
        self.events.write_bytes(b'--remote_header=x-nativelink-api-key=public\x12'
                                b'--client_env=HOME=/home/runner\x12')
        self.collect()
        self.assertIn('events.pb', self.staged())

    def test_long_log_keeps_its_tail(self):
        self.target('pkg/noisy_test', FAILED, log='a' * 5000 + 'panicked at the end\n')
        with patch.object(ci_bep_artifact, 'MAX_LOG_BYTES', 200):
            manifest = self.collect()
        staged = (self.output / 'testlogs/pkg/noisy_test/test.log').read_bytes()
        self.assertEqual(len(staged), 200)
        self.assertTrue(staged.startswith(b'[ci_bep_artifact: truncated'))
        self.assertTrue(staged.endswith(b'panicked at the end\n'))
        entry = next(f for f in manifest['files'] if f['path'].endswith('test.log'))
        self.assertTrue(entry['truncated'])
        self.assertEqual(entry['original_bytes'], 5020)

    def test_total_log_budget_and_target_cap(self):
        for index in range(4):
            self.target(f'pkg/t{index}_test', FAILED, log='z' * 300)
        with patch.object(ci_bep_artifact, 'MAX_TOTAL_LOG_BYTES', 1000), \
                patch.object(ci_bep_artifact, 'MAX_FAILED_TARGETS', 3):
            manifest = self.collect()
        logs = sum(entry['bytes'] for entry in manifest['files'])
        self.assertLessEqual(logs, 1000)
        reasons = {entry['reason'] for entry in manifest['skipped']}
        self.assertIn('failed-target cap', reasons)
        self.assertEqual(manifest['failed_targets'], 4)

    def test_symlinks_are_not_followed_out_of_the_tree(self):
        secret = self.root / 'secret.txt'
        secret.write_text('not for upload')
        directory = self.target('pkg/link_test', FAILED)
        (directory / 'test.log').unlink()
        os.symlink(secret, directory / 'test.log')
        self.events.symlink_to(secret)
        self.collect()
        self.assertEqual(self.staged(), ['manifest.json', 'testlogs/pkg/link_test/test.xml'])

    def test_testlogs_symlink_root_is_resolved(self):
        self.target('pkg/failing_test', FAILED)
        link = self.root / 'bazel-testlogs'
        link.symlink_to(self.testlogs)
        ci_bep_artifact.collect(self.events, link, self.output, {})
        self.assertIn('testlogs/pkg/failing_test/test.log', self.staged())

    def test_manifest_identity_is_allowlisted(self):
        manifest = self.collect({'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '7',
                                 'GITHUB_TOKEN': 'never', 'NL_API_KEY': 'never'})
        self.assertEqual(manifest['reported_identity'], {'GITHUB_SHA': 'a' * 40,
                                                         'GITHUB_RUN_ID': '7'})
        self.assertNotIn('never', (self.output / 'manifest.json').read_text())

    def test_cli_refuses_a_non_empty_output(self):
        self.output.mkdir()
        (self.output / 'stale').write_text('x')
        with self.assertRaises(SystemExit):
            ci_bep_artifact.main(['--events', str(self.events), '--output', str(self.output)])


class WorkflowTest(unittest.TestCase):
    """The upload must never run where a private key could be in the events."""

    def test_pr_lane_captures_and_guards_on_the_credential(self):
        workflows = Path(__file__).resolve().parents[2] / '.github/workflows'
        text = (workflows / 'parallel-cluster-ci.yaml').read_text()
        self.assertIn('--build_event_binary_file="$RUNNER_TEMP/bep/events.pb"', text)
        guard = ("if: always() && contains(fromJSON('[\"public\", \"none\"]'), "
                 "steps.setup-nativelink-cloud.outputs.credential)")
        self.assertEqual(text.count(guard), 2)
        self.assertNotIn("credential != 'private'", text)
        for workflow in ('native-bazel.yaml', 'parallel-cluster-ci.yaml'):
            with self.subTest(workflow=workflow):
                self.assertIn('env -u BAZELISK_GITHUB_TOKEN ./tools/bazel-retry.sh test //...',
                              (workflows / workflow).read_text())


if __name__ == '__main__':
    unittest.main()
