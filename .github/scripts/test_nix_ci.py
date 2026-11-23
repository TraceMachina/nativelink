"""Regress duplicate package builds and fail closed on unusable shared outputs."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
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
import os,sys,time
from pathlib import Path
with (Path(os.environ['FAKE_ROOT'])/'calls').open('a') as trace:trace.write('gc\\n')
if os.environ.get('FAKE_GC_HANG'):time.sleep(30)
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

    def test_hung_store_collection_does_not_consume_the_build_budget(self):
        # A collector stuck behind another process's store lock is killed
        # after the bounded wait and the build still runs.
        with patch.object(nix_ci, 'GC_TIMEOUT_SECONDS', 0.2), \
                patch.dict(os.environ, FAKE_GC_HANG='1'):
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
        self.assertEqual(workflow.count('cluster_ci.py --task nix'), 1)
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

    def test_shared_nix_package_is_built_on_aws_for_every_branch(self):
        workflow = Path('.github/workflows/nix.yaml').read_text()
        job = workflow.split('  nix-build:', 1)[1].split('  nix-cargo:', 1)[0]
        self.assertNotIn('nix-build-aws', workflow)
        self.assertIn('environment: cluster-ci-aws', job)
        self.assertIn('name: Build shared Nix package\n', job)
        self.assertIn("startsWith(github.ref, 'refs/heads/')", job)
        for removed in ('continue-on-error', 'NL_AWS_LANE', 'cluster-ci-benchmark', 'NL_GCP_'):
            self.assertNotIn(removed, workflow)
        # The Nix client by its own name, and only short-lived credentials.
        self.assertIn('NL_GKE_POD: ${{ vars.NL_GKE_NIX_POD }}', job)
        self.assertIn('id-token: write', job)
        self.assertRegex(job, r'aws-actions/configure-aws-credentials@[a-f0-9]{40}\n')
        for stored_key in ('aws-access-key-id', 'aws-secret-access-key', 'AWS_SECRET_ACCESS_KEY'):
            self.assertNotIn(stored_key, job)
        # The log of this public repository shows a variable and masks a
        # secret, so what locates the deployment is read from secrets.
        for located in ('NL_AWS_DISPATCH_ROLE_ARN', 'NL_GKE_SERVER', 'NL_GKE_CA_DATA'):
            self.assertIn('${{ secrets.%s }}' % located, job)
        self.assertIn('mask-aws-account-id: true', job)
        # The result is read with a new token after the build, so the
        # credentials must outlast the job.
        session = int(re.search(r'role-duration-seconds: (\d+)', job).group(1))
        limit = int(re.search(r'timeout-minutes: (\d+)', job).group(1))
        self.assertGreater(session, limit * 60)
        # The cluster client asks the AWS CLI for each token, and the
        # free-disk action removes that CLI from the runner.
        self.assertNotIn('uses: ./.github/actions/free-disk', job)
        # The jobs below import its closure and read its command matrix.
        self.assertIn('matrix: ${{ steps.manifest.outputs.matrix }}', job)
        self.assertIn('name: nix-ci-package', job)


class NixTransferTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.output = self.root / 'download'
        self.revision = 'a' * 40
        self.closure = b'Nix closure bytes' * 4096
        self.report = {
            'kind': 'nix', 'revision': self.revision, 'system': nix_ci.SYSTEM, 'exit_code': 0,
            'outputs': {name: '/nix/store/' + 'b' * 32 + '-' + attribute
                        for name, attribute in nix_ci.TARGETS.items()},
            'matrix': {'command': ['python', 'java']}, 'build_seconds': 1,
            'bytes': len(self.closure), 'sha256': hashlib.sha256(self.closure).hexdigest(),
        }

    def bundle(self):
        return [('nix.json', json.dumps(self.report).encode()), ('closure.nar.gz', self.closure)]

    def transfer(self, entries=None, exit_code=0, truncate=False):
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w') as archive:
            for name, data in self.bundle() if entries is None else entries:
                member = tarfile.TarInfo(name)
                if data is None:
                    member.type = tarfile.SYMTYPE
                    member.linkname = '../outside'
                    archive.addfile(member)
                else:
                    member.size = len(data)
                    archive.addfile(member, io.BytesIO(data))
        bundle = self.root / 'bundle.tar'
        bundle.write_bytes(stream.getvalue()[:2048] if truncate else stream.getvalue())
        command = [sys.executable, '-c',
                   'import sys; from pathlib import Path; sys.stdin.buffer.read(); '
                   'sys.stdout.buffer.write(Path(sys.argv[1]).read_bytes()); sys.exit(int(sys.argv[2]))',
                   str(bundle), str(exit_code)]
        return cluster_ci.receive_nix_result(command, {},
                                              {**os.environ, 'GITHUB_SHA': self.revision}, self.output)

    def test_verified_stream_is_published_without_a_second_download(self):
        self.assertEqual(self.transfer(), 0)
        self.assertEqual((self.output / 'closure.nar.gz').read_bytes(), self.closure)
        self.assertEqual(json.loads((self.output / 'nix.json').read_text()), self.report)
        self.assertEqual({p.name for p in self.output.iterdir()}, {'nix.json', 'closure.nar.gz'})

    def test_wrong_revision_size_and_checksum_leave_no_success(self):
        for field, invalid in (('revision', 'c' * 40), ('bytes', len(self.closure) + 1),
                               ('sha256', '0' * 64)):
            with self.subTest(field=field), patch.dict(self.report, {field: invalid}):
                with self.assertRaises(ValueError):
                    self.transfer()
                self.assertEqual(list(self.output.iterdir()), [])

    def test_missing_duplicate_link_and_unexpected_members_are_rejected(self):
        for entries in (self.bundle()[:1], self.bundle() + self.bundle()[:1],
                        [('nix.json', None)], [('../outside', b'bad')],
                        [('nix.json', b'x' * (64 * 1024 + 1))]):
            with self.subTest(names=[name for name, _ in entries]), self.assertRaises(ValueError):
                self.transfer(entries)
            self.assertEqual(list(self.output.iterdir()), [])
            self.assertFalse((self.root / 'outside').exists())

    def test_disconnect_and_failed_remote_command_cannot_publish_partial_or_stale_success(self):
        for problem in ({'truncate': True}, {'exit_code': 17}):
            with self.subTest(problem=problem):
                self.transfer()
                with self.assertRaises((tarfile.TarError, subprocess.CalledProcessError)):
                    self.transfer(**problem)
                self.assertEqual(list(self.output.iterdir()), [])

    def test_dispatch_keeps_the_lock_until_the_closure_has_been_sent(self):
        # Execute the real bootstrap with small build fixtures. A queued build
        # prunes the archive as soon as that exec exits, just as on the client.
        workspace = self.root / 'workspace'
        (workspace / 'nativelink').mkdir(parents=True)
        fixtures = self.root / 'fixtures'
        fixtures.mkdir()
        (fixtures / 'nix.json').write_text(json.dumps(self.report))
        (fixtures / 'closure.nar.gz').write_bytes(self.closure)
        tools = self.root / 'bin'
        tools.mkdir()
        scripts = {
            'git': "print('checkout diagnostic')\n",
            'flock': "import fcntl, sys\nfcntl.flock(int(sys.argv[1]), fcntl.LOCK_EX)\n",
            'timeout': "import subprocess, sys\nsys.exit(subprocess.call(sys.argv[4:]))\n",
            'python3': '''import os, shutil, sys
from pathlib import Path
assert sys.argv[1:] == ['.github/scripts/nix_ci.py', 'build', '--directory', os.environ['NL_SUMMARY_DIRECTORY']]
destination = Path(os.environ['NL_SUMMARY_DIRECTORY'])
for name in ('nix.json', 'closure.nar.gz'):
    shutil.copyfile(Path(os.environ['FIXTURES']) / name, destination / name)
print('build diagnostic')
''',
            'tar': '''import fcntl, os, subprocess, sys
from pathlib import Path
with (Path(os.environ['WORKSPACE']) / 'ci.lock').open('a') as lock:
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        pass
    else:
        raise AssertionError('The build lock was released before transfer')
sys.exit(subprocess.call([os.environ['REAL_TAR'], *sys.argv[1:]]))
''',
            'kubectl': '''import json, os, subprocess, sys
from pathlib import Path
command = sys.argv[sys.argv.index('--') + 1:]
workspace = os.environ['WORKSPACE']
with (Path(workspace) / 'execs').open('a') as trace:
    trace.write(command[0] + '\\n')
if command[0] == 'node':
    payload = json.load(sys.stdin)
    payload['NL_SUMMARY_DIRECTORY'] = payload['NL_SUMMARY_DIRECTORY'].replace('/workspace', workspace)
    command[-1] = command[-1].replace('/workspace', workspace)
    result = subprocess.run(command, input=json.dumps(payload).encode())
    # Another run acquires ci.lock and prunes immediately after the first one.
    (Path(payload['NL_SUMMARY_DIRECTORY']) / 'closure.nar.gz').unlink(missing_ok=True)
    sys.exit(result.returncode)
command[-1] = command[-1].replace('/workspace', workspace)
sys.exit(subprocess.call(command))
''',
        }
        for name, script in scripts.items():
            path = tools / name
            path.write_text('#!' + sys.executable + '\n' + script)
            path.chmod(0o755)
        environment = {
            'PATH': str(tools) + os.pathsep + os.environ['PATH'],
            'FIXTURES': str(fixtures), 'WORKSPACE': str(workspace), 'REAL_TAR': shutil.which('tar'),
            # macOS tar otherwise adds AppleDouble files absent on the Linux client.
            'COPYFILE_DISABLE': '1',
            'GITHUB_REPOSITORY': 'TraceMachina/nativelink', 'GITHUB_EVENT_NAME': 'push',
            'GITHUB_REF': 'refs/heads/main', 'GITHUB_SHA': self.revision,
            'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '1', 'BENCHMARK_STARTED': '0',
            'NL_GKE_SERVER': 'https://cluster.example', 'NL_GKE_CA_DATA': 'unused',
            'NL_GKE_NAMESPACE': 'nativelink', 'NL_GKE_POD': 'nix-client',
            'NL_EKS_CLUSTER': 'ci', 'NL_AWS_REGION': 'us-east-1',
        }
        with patch.dict(os.environ, environment):
            self.assertEqual(cluster_ci.main(SimpleNamespace(
                task='nix', repetitions=1, output=self.output)), 0)
        self.assertEqual((workspace / 'execs').read_text(), 'node\n')
        self.assertFalse((workspace / 'ci-results/123-1/closure.nar.gz').exists())
        self.assertEqual((self.output / 'closure.nar.gz').read_bytes(), self.closure)
        self.assertEqual(json.loads((self.output / 'nix.json').read_text()), self.report)


if __name__ == '__main__':
    unittest.main()
