"""Regress duplicate package builds and fail closed on unusable shared outputs."""
import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import cluster_ci
import nix_ci


class NixCITest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.directory = self.root / 'artifact'
        self.revision = 'a' * 40
        self.paths = {name: '/nix/store/' + 'b' * 32 + '-' + attribute
                      for name, attribute in nix_ci.TARGETS.items()}
        tools = self.root / 'bin'
        tools.mkdir()
        (tools / 'nix').write_text('''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
root=Path(os.environ['FAKE_ROOT'])
with (root/'calls').open('a') as trace:trace.write('nix '+sys.argv[1]+'\\n')
if sys.argv[1]=='eval':print(os.environ['FAKE_OUTPUTS'])
elif sys.argv[1]=='build':sys.exit(int(os.environ.get('FAKE_BUILD_FAILURE','0')))
elif sys.argv[1]=='path-info':sys.exit(0 if (root/'imported').exists() else 21)
else:sys.exit(22)
''')
        (tools / 'nix-store').write_text('''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
root=Path(os.environ['FAKE_ROOT'])
with (root/'calls').open('a') as trace:trace.write('store '+sys.argv[1]+'\\n')
if sys.argv[1]=='--query':print('\\n'.join(json.loads(os.environ['FAKE_OUTPUTS']).values()))
elif sys.argv[1]=='--export':
 sys.stdout.buffer.write(b'test Nix closure')
 sys.exit(int(os.environ.get('FAKE_EXPORT_FAILURE','0')))
elif sys.argv[1]=='--import':
 assert sys.stdin.buffer.read()==b'test Nix closure'
 if os.environ.get('FAKE_IMPORT_FAILURE'):sys.exit(19)
 (root/'imported').touch()
else:sys.exit(23)
''')
        for path in tools.iterdir():
            path.chmod(0o755)
        self.environment = patch.dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                                      FAKE_ROOT=str(self.root), FAKE_OUTPUTS=json.dumps(self.paths))
        self.environment.start()
        self.addCleanup(self.environment.stop)
        real_check_output = subprocess.check_output
        def commands(command, **kwargs):
            if command[0].endswith('/bin/rbe-toolchain-list'):
                return '["python", "java"]'
            return real_check_output(command, **kwargs)
        helper = patch.object(nix_ci.subprocess, 'check_output', side_effect=commands)
        helper.start()
        self.addCleanup(helper.stop)

    def calls(self):
        return (self.root / 'calls').read_text().splitlines() if (self.root / 'calls').exists() else []

    def test_one_build_serves_multiple_consumers_without_rebuilding(self):
        nix_ci.build(self.directory, self.revision)
        for _ in range(3):
            report = nix_ci.import_closure(self.directory, self.revision)
            self.assertEqual(report['outputs'], self.paths)
        self.assertEqual(self.calls().count('nix build'), 1)
        self.assertEqual(self.calls().count('store --import'), 3)
        self.assertEqual(self.calls().count('nix path-info'), 3)

    def test_bad_revision_checksum_and_package_definition_stop_before_import(self):
        report = nix_ci.build(self.directory, self.revision)
        for problem in ('revision', 'checksum', 'definitions'):
            with self.subTest(problem=problem):
                invalid = copy.deepcopy(report)
                if problem == 'revision':
                    invalid['revision'] = 'c' * 40
                if problem == 'checksum':
                    invalid['sha256'] = '0' * 64
                if problem == 'definitions':
                    invalid['outputs']['nativelink'] = '/nix/store/' + 'd' * 32 + '-different-package'
                (self.directory / 'nix.json').write_text(json.dumps(invalid))
                with self.assertRaises(ValueError):
                    nix_ci.import_closure(self.directory, self.revision)
                self.assertNotIn('store --import', self.calls())
        self.assertEqual(self.calls().count('nix build'), 1)

    def test_failed_import_never_falls_back_to_a_runner_build(self):
        nix_ci.build(self.directory, self.revision)
        with patch.dict(os.environ, FAKE_IMPORT_FAILURE='1'), self.assertRaises(RuntimeError):
            nix_ci.import_closure(self.directory, self.revision)
        self.assertEqual(self.calls().count('nix build'), 1)
        self.assertNotIn('nix path-info', self.calls())

    def test_failed_build_and_export_cannot_publish_a_stale_success(self):
        for variable in ('FAKE_BUILD_FAILURE', 'FAKE_EXPORT_FAILURE'):
            with self.subTest(variable=variable):
                nix_ci.build(self.directory, self.revision)
                with patch.dict(os.environ, **{variable: '17'}), self.assertRaises(
                        (subprocess.CalledProcessError, RuntimeError)):
                    nix_ci.build(self.directory, self.revision)
                self.assertFalse((self.directory / 'nix.json').exists())

    def test_missing_artifact_fails_without_starting_nix(self):
        with self.assertRaises(FileNotFoundError):
            nix_ci.import_closure(self.directory, self.revision)
        self.assertEqual(self.calls(), [])

    def test_nix_dispatch_does_not_forward_build_or_github_credentials(self):
        environment = {'GITHUB_REPOSITORY': 'TraceMachina/nativelink', 'GITHUB_EVENT_NAME': 'push',
                       'GITHUB_REF': 'refs/heads/bugfix/example', 'GITHUB_SHA': self.revision,
                       'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '1', 'BENCHMARK_STARTED': '0',
                       'NL_API_KEY': 'never-forward', 'GITHUB_TOKEN': 'never-forward',
                       'NATIVELINK_ATTIC_TOKEN': 'never-forward', 'GOOGLE_APPLICATION_CREDENTIALS': 'never-forward'}
        result = cluster_ci.payload(environment, 1, 'nix')
        self.assertNotIn('never-forward', json.dumps(result))
        self.assertNotIn('NL_API_KEY', result)
        self.assertEqual(result['NL_TASK'], 'nix')
        with self.assertRaises(ValueError):
            cluster_ci.payload(environment, 1, 'arbitrary-command')

    def test_workflow_consumers_wait_for_and_import_the_shared_package(self):
        workflow = Path('.github/workflows/nix.yaml').read_text()
        self.assertEqual(workflow.count('cluster_ci.py --task nix'), 1)
        self.assertNotIn('  pull_request:', workflow)
        self.assertNotIn('  generate-rbe-commands:', workflow)
        installation = workflow.split('  installation:', 1)[1].split('  integration:', 1)[0]
        toolchains = workflow.split('  rbe-toolchain:', 1)[1]
        for consumer in (installation, toolchains):
            self.assertIn('needs: [nix-build]', consumer)
            self.assertIn('actions/download-artifact@', consumer)
            self.assertIn('nix_ci.py import', consumer)
            self.assertIn("read_only: 'true'", consumer)
            self.assertNotIn('secrets.', consumer)
            self.assertLess(consumer.index('nix_ci.py import'), consumer.index('nix run -L'))
        self.assertIn('needs.nix-build.outputs.matrix', toolchains)


if __name__ == '__main__':
    unittest.main()
