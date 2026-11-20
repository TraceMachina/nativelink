"""Regress duplicate package builds and fail closed on unusable shared outputs."""
import copy
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import cluster_ci
import nix_ci


class NixCITest(unittest.TestCase):
    def test_coverage_cache_failure_does_not_override_the_coverage_result(self):
        workflow = (Path(__file__).parents[1] / 'workflows/coverage.yaml').read_text()
        coverage = workflow.split('  coverage:', 1)[1].split('  deploy:', 1)[0]
        required, teardown = coverage.split('      - name: Teardown Worker', 1)
        # Generation and artifact publication must still fail the job normally.
        self.assertIn('nix build -L .#nativelinkCoverageForHost', required)
        self.assertIn('actions/upload-pages-artifact@', required)
        self.assertNotIn('continue-on-error:', required)
        self.assertEqual(coverage.count('continue-on-error:'), 1)
        self.assertIn('continue-on-error: true', teardown)
        self.assertIn('timeout-minutes: 10', teardown)
        self.assertIn('id: cache-upload', teardown)
        # Uploads are still attempted, and exhaustion remains visible.
        self.assertIn('uses: ./.github/actions/end-nix', teardown)
        self.assertIn('nix_name: .#nativelinkCoverageForHost', teardown)
        self.assertIn("steps.cache-upload.outcome == 'failure'", teardown)
        self.assertIn('::warning::Nix cache upload failed', teardown)
        self.assertIn('needs: coverage', workflow.split('  deploy:', 1)[1])

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
        (tools / 'nix-collect-garbage').write_text('''#!/usr/bin/env python3
import os,sys
from pathlib import Path
with (Path(os.environ['FAKE_ROOT'])/'calls').open('a') as trace:trace.write('gc\\n')
sys.exit(int(os.environ.get('FAKE_GC_FAILURE','0')))
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

    def test_build_prunes_older_runs_and_every_stale_archive_before_building(self):
        closure = self.root / 'closure'
        closure.mkdir()
        results = self.root / 'ci-results'
        runs = []
        for age, name in enumerate(['run-1', 'run-2', 'run-3', 'run-4']):
            run = results / name
            run.mkdir(parents=True)
            (run / 'result').symlink_to(closure)
            (run / 'closure.nar.gz').write_bytes(b'archived closure')
            (run / 'nix.json').write_text('{}')
            os.utime(run, (1_000 + age, 1_000 + age))
            runs.append(run)

        nix_ci.build(results / 'current', self.revision)

        removed = [run.name for run in runs if not run.exists()]
        self.assertEqual(removed, ['run-1', 'run-2'])
        for kept in (runs[2], runs[3]):
            self.assertTrue((kept / 'result').is_symlink(),
                            'retained result roots keep recent closures warm')
            self.assertFalse((kept / 'closure.nar.gz').exists(),
                             'archives are only read from the uploaded artifact')
        self.assertTrue((results / 'current' / 'closure.nar.gz').exists())
        self.assertLess(self.calls().index('gc'), self.calls().index('nix build'))

    def test_failed_store_collection_does_not_block_the_build(self):
        with patch.dict(os.environ, FAKE_GC_FAILURE='1'):
            report = nix_ci.build(self.directory, self.revision)
        self.assertEqual(report['outputs'], self.paths)
        self.assertEqual(self.calls().count('nix build'), 1)

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
        # Once for the package the jobs below import, once on the AWS deployment.
        self.assertEqual(workflow.count('cluster_ci.py --task nix'), 2)
        self.assertNotIn('  pull_request:', workflow)
        self.assertNotIn('  generate-rbe-commands:', workflow)
        installation = workflow.split('  installation:', 1)[1].split('  integration:', 1)[0]
        toolchains = workflow.split('  rbe-toolchain:', 1)[1]
        for consumer in (installation, toolchains):
            self.assertIn('needs: [nix-build]', consumer)
            self.assertIn('actions/download-artifact@', consumer)
            self.assertIn('nix_ci.py import', consumer)
            self.assertIn("read_only: \"true\"", consumer)
            self.assertNotIn('secrets.', consumer)
            self.assertLess(consumer.index('nix_ci.py import'), consumer.index('nix run -L'))
        self.assertIn('needs.nix-build.outputs.matrix', toolchains)

    def test_aws_nix_lane_is_optional_and_nothing_reads_what_it_built(self):
        workflow = Path('.github/workflows/nix.yaml').read_text()
        primary, rest = workflow.split('  nix-build:', 1)[1].split('  nix-build-aws:', 1)
        aws_job, after = rest.split('  nix-cargo:', 1)
        # The primary job is unchanged: it neither waits for nor depends on AWS.
        for setting in ('NL_EKS_CLUSTER', 'configure-aws-credentials', 'cluster-ci-aws', 'continue-on-error'):
            self.assertNotIn(setting, primary.split('\n  # The same build on the AWS deployment')[0])
        self.assertIn('environment: cluster-ci-aws', aws_job)
        self.assertIn('continue-on-error: true', aws_job)
        self.assertIn("vars.NL_AWS_LANE == 'true'", aws_job)
        # Reviewed code only: a pushed branch is built by the primary job.
        self.assertIn("github.ref == 'refs/heads/main'", aws_job)
        self.assertNotIn('startsWith(github.ref', aws_job)
        # The Nix client by its own name, and only short-lived credentials.
        self.assertIn('NL_GKE_POD: ${{ vars.NL_GKE_NIX_POD }}', aws_job)
        self.assertIn('id-token: write', aws_job)
        self.assertRegex(aws_job, r'aws-actions/configure-aws-credentials@[a-f0-9]{40}\n')
        for stored_key in ('aws-access-key-id', 'aws-secret-access-key', 'AWS_SECRET_ACCESS_KEY', 'NL_GCP_'):
            self.assertNotIn(stored_key, aws_job)
        # The log of this public repository shows a variable and masks a
        # secret, so what locates the deployment is read from secrets.
        for located in ('NL_AWS_DISPATCH_ROLE_ARN', 'NL_GKE_SERVER', 'NL_GKE_CA_DATA'):
            self.assertIn('${{ secrets.%s }}' % located, aws_job)
        self.assertIn('mask-aws-account-id: true', aws_job)
        # The result is read with a new token after the build, so the
        # credentials must outlast the job.
        session = int(re.search(r'role-duration-seconds: (\d+)', aws_job).group(1))
        limit = int(re.search(r'timeout-minutes: (\d+)', aws_job).group(1))
        self.assertGreater(session, limit * 60)
        # The cluster client asks the AWS CLI for each token, and the
        # free-disk action removes that CLI from the runner.
        self.assertNotIn('uses: ./.github/actions/free-disk', aws_job)
        # Nothing waits for it or imports its closure; only its report is kept.
        self.assertNotIn('nix-build-aws', after)
        self.assertNotIn('outputs:', aws_job)
        self.assertNotIn('name: nix-ci-package', aws_job)
        self.assertIn('name: nix-ci-aws', aws_job)
        self.assertIn('nix-ci/nix.json', aws_job)


if __name__ == '__main__':
    unittest.main()
