"""Exercise the Bazel compatibility lane's lockfile selection."""

import json
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


class Bazel8LockfileTest(unittest.TestCase):
    def test_compatibility_lane_runs_only_on_schedule_or_manual_dispatch(self):
        workflow = Path('.github/workflows/native-bazel.yaml').read_text()
        job = workflow.split('  unit-tests:\n', 1)[1].split('  redis-store-tester:', 1)[0]
        condition = next(line.strip().removeprefix('if: ') for line in job.splitlines()
                         if line.startswith('    if: '))
        # This allowlist excludes pushes as well as PRs, while preserving
        # the existing nightly schedule and an explicit manual run.
        self.assertEqual(condition,
                         "${{ github.event_name == 'schedule' || github.event_name == 'workflow_dispatch' }}")
        self.assertIn('cron: "0 7 * * *"', workflow)
        self.assertIn('  workflow_dispatch:', workflow)
        self.assertIn('bazel_version: 8.8.2', job)
        redis_job = workflow.split('  redis-store-tester:\n', 1)[1]
        self.assertNotIn('\n    if:', redis_job)

    def run_compatibility_step(self, root):
        workflow = Path('.github/workflows/native-bazel.yaml').read_text()
        step = workflow.split('      - name: Run Bazel compatibility tests\n', 1)[1]
        script = textwrap.dedent(step.split('        run: |\n', 1)[1].split('\n  redis-store-tester:', 1)[0])
        return subprocess.run(['bash', '-e', '-c', script], cwd=root,
                              capture_output=True, text=True, check=False)

    def prepare_workspace(self, root):
        (root / 'MODULE.bazel.lock').write_text('{"source": "default"}\n')
        (root / 'tools').mkdir()
        bazel = root / 'tools/bazel-retry.sh'
        bazel.write_text('#!/bin/bash\ncp MODULE.bazel.lock selected.lock\nprintf "%s\\n" "$@" > arguments.txt\n')
        bazel.chmod(0o755)

    def test_compatibility_lane_selects_bazel8_lockfile_and_enforces_it(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.prepare_workspace(root)
            lockfile = '{"source": "bazel8"}\n'
            (root / 'MODULE.bazel8.lock').write_text(lockfile)
            result = self.run_compatibility_step(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'selected.lock').read_text(), lockfile)
            self.assertEqual((root / 'MODULE.bazel8.lock').read_text(), lockfile)
            arguments = (root / 'arguments.txt').read_text().splitlines()
            self.assertEqual(arguments[:2], ['test', '//...'])
            self.assertIn('--lockfile_mode=error', arguments)
            self.assertNotIn('--lockfile_mode=off', arguments)

    def test_missing_bazel8_lockfile_stops_before_running_bazel(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.prepare_workspace(root)
            result = self.run_compatibility_step(root)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((root / 'selected.lock').exists())
            self.assertEqual(json.loads((root / 'MODULE.bazel.lock').read_text()),
                             {'source': 'default'})


if __name__ == '__main__':
    unittest.main()
