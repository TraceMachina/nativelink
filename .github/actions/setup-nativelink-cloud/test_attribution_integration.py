"""Regressions for every NativeLink Cloud CI entry point and the Bazelisk hook."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[3]


class AttributionIntegrationTest(unittest.TestCase):
    def test_every_cloud_job_grants_oidc(self):
        checked = 0
        for path in (ROOT / '.github/workflows').glob('*.yaml'):
            text = path.read_text()
            # Job keys have two-space indentation. Commented-out jobs/steps do
            # not participate, and workflow-level read-all cannot grant OIDC.
            for job in re.split(r'^  [\w-]+:\s*$', text, flags=re.M)[1:]:
                active = '\n'.join(line for line in job.splitlines() if not line.lstrip().startswith('#'))
                if 'uses: ./.github/actions/setup-nativelink-cloud' not in active:
                    continue
                checked += 1
                self.assertIn('attribution-url: ${{ secrets.NATIVELINK_GITHUB_APP_URL }}', active, path.name)
                self.assertRegex(active, r'(?m)^    permissions:\n(?:      [^\n]+\n)*      id-token: write$', path.name)
        self.assertGreaterEqual(checked, 7)

    def test_cloud_setup_delegates_to_one_pinned_implementation(self):
        action = (ROOT / '.github/actions/setup-nativelink-cloud/action.yaml').read_text()
        self.assertIsNotNone(re.search(r'uses: TraceMachina/nativelink-action/attribution@[0-9a-f]{40}', action), 'The shared attribution Action must use a commit pin')
        self.assertIn("if: steps.configure.outputs.bes-enabled == 'true'", action)
        native = (ROOT / '.github/workflows/native-bazel.yaml').read_text()
        self.assertNotIn('run_bazel.py', native)
        self.assertFalse((ROOT / '.github/actions/setup-nativelink-cloud/run_bazel.py').exists())

    def test_docker_uses_secret_mounts_for_attribution(self):
        dockerfile = (ROOT / 'deployment-examples/docker-compose/Dockerfile').read_text()
        workflow = (ROOT / '.github/workflows/main.yaml').read_text()
        for name in ('config', 'wrapper'):
            self.assertIn(f'--mount=type=secret,id=nativelink-attribution-{name}', dockerfile)
            self.assertIn(f"nativelink-attribution-{name}=${{{{ steps.nl.outputs.attribution-{name} || '/dev/null' }}}}", workflow)
        self.assertNotRegex(dockerfile, r'(?m)^(ARG|ENV|COPY) .*OIDC|^(ARG|ENV|COPY) .*attribution')

    def test_only_authenticated_bes_setup_enables_attribution(self):
        action = (ROOT / '.github/actions/setup-nativelink-cloud/action.yaml').read_text()
        script = textwrap.dedent(action.split('      run: |\n', 1)[1].split('\n    - name:', 1)[0])
        for kind in ('private', 'public', 'disabled'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / 'output'
                env = dict(os.environ, GITHUB_OUTPUT=str(output), NL_CI_MODE='cache', NL_MODE='read',
                           NL_EXEC='off', RUNNER_OS='Linux', NL_API_KEY='', NL_CLAIM_BASE='',
                           NL_PUBLIC_KEY='', NL_PUBLIC_CLAIM_BASE='', NL_BES_RESULTS_URL='',
                           NL_CONTAINER_IMAGE='test-image')
                if kind == 'private':
                    env.update(NL_API_KEY='test-private-key', NL_CLAIM_BASE='private.invalid')
                elif kind == 'public':
                    env.update(NL_PUBLIC_KEY='test-public-key', NL_PUBLIC_CLAIM_BASE='public.invalid')
                else:
                    env['NL_CI_MODE'] = 'off'
                result = subprocess.run(['bash', '-euo', 'pipefail', '-c', script], cwd=directory,
                                        env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0)
                self.assertEqual('bes-enabled=true' in output.read_text(), kind == 'private')
                self.assertNotIn('test-private-key', result.stdout + result.stderr)
                rc = (Path(directory) / 'user.bazelrc').read_text()
                self.assertEqual('--bes_backend=' in rc, kind == 'private')
                self.assertNotIn('nl_ticket:', rc)

    def test_bazelisk_hook_preserves_arguments_and_delegates_in_nix_or_container(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            real = root / 'real-bazel'
            real.write_text(f'#!{sys.executable}\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\nsys.exit(23)\n')
            real.chmod(0o700)
            wrapper = root / 'wrapper.py'
            wrapper.write_text('import os,sys\nos.execv(os.environ["NATIVELINK_BAZEL_REAL"], ["bazel", "attributed", *sys.argv[1:]])\n')
            config = root / 'config.json'
            config.write_text('{}')
            args = ['--output_base', 'directory with spaces', 'run', '//:target', '--', 'a b']
            for enabled in (False, True):
                env = dict(os.environ, BAZEL_REAL=str(real), NATIVELINK_BAZEL_WRAPPER='', NATIVELINK_ATTRIBUTION_CONFIG='')
                if enabled:
                    env.update(NATIVELINK_BAZEL_WRAPPER=str(wrapper), NATIVELINK_ATTRIBUTION_CONFIG=str(config))
                result = subprocess.run(['bash', str(ROOT / 'tools/bazel'), *args], env=env,
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 23, result.stderr)
                self.assertEqual(json.loads(result.stdout), (['attributed'] if enabled else []) + args)


if __name__ == '__main__':
    unittest.main()
