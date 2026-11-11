"""Exercise the Bazel compatibility lane's lockfile selection."""

import json
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


class Bazel8LockfileTest(unittest.TestCase):
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

    def test_compatibility_lane_is_nightly_only_without_remote_execution(self):
        workflow = Path('.github/workflows/native-bazel.yaml').read_text()
        lane = workflow.split('  unit-tests:\n', 1)[1].split('\n  redis-store-tester:', 1)[0]
        condition = next(line for line in lane.splitlines() if line.startswith('    if:'))
        self.assertIn('schedule', condition)
        self.assertIn('workflow_dispatch', condition)
        self.assertNotIn('push', condition)
        self.assertNotIn('pull_request', condition)
        self.assertIn('exec: "off"', lane)
        self.assertIn('mode: read', lane)
        self.assertNotIn('API_KEY_RW', lane)

    def test_lockfile_regeneration_watches_the_module_and_crate_graph(self):
        workflow = Path('.github/workflows/bazel8-lockfile.yaml').read_text()
        for source in ('"MODULE.bazel"', '"MODULE.bazel.lock"', '"Cargo.lock"',
                       '".bazelversion"'):
            self.assertIn(f'      - {source}\n', workflow)
        self.assertIn('USE_BAZEL_VERSION: 8.8.2', workflow)
        self.assertIn('bazel mod deps --lockfile_mode=update', workflow)
        # The regenerated file must be copied out and the default restored,
        # in that order, or the PR would commit a Bazel 8 default lockfile.
        regenerate = workflow.split('cp MODULE.bazel8.lock MODULE.bazel.lock', 1)[1]
        self.assertLess(regenerate.index('cp MODULE.bazel.lock MODULE.bazel8.lock'),
                        regenerate.index('git restore MODULE.bazel.lock'))
        self.assertIn('git add MODULE.bazel8.lock', workflow)

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
