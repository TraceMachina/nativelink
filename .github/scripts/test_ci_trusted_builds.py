"""Prevent green local-only PR checks from standing in for trusted builds."""

import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import ci_trusted_builds as ci


SHA = 'a' * 40
PR = {'number': 3210, 'state': 'open', 'head': {'sha': SHA}, 'changed_files': 1,
      'base': {'ref': 'main', 'repo': {'full_name': ci.REPOSITORY}}}


def run(workflow, **changes):
    result = {'id': 10, 'head_sha': SHA, 'event': 'push', 'status': 'completed',
              'conclusion': 'success', 'head_repository': {'full_name': ci.REPOSITORY},
              'path': f'.github/workflows/{workflow}'}
    result.update(changes)
    return result


class TrustedBuildsTest(unittest.TestCase):
    def setUp(self):
        self.runs = {workflow: [run(workflow)] for workflow in ci.WORKFLOWS}
        self.jobs = [{'name': name, 'conclusion': 'success'} for name in ci.WORKFLOWS.values()]
        self.files = [{'filename': 'nativelink-worker/src/local_worker.rs'}]
        self.statuses = []
        self.requests = []
        self.pr = copy.deepcopy(PR)
        self.api = patch.object(ci, 'api', side_effect=self.respond)
        self.api.start()
        self.addCleanup(self.api.stop)

    def respond(self, path, payload=None):
        self.requests.append(path)
        if path.startswith('statuses/'):
            self.statuses.append((path, payload))
            return {}
        if path == 'pulls/3210':
            return self.pr
        if path.startswith('pulls?'):
            return [self.pr]
        if path.startswith('pulls/3210/files?'):
            return self.files
        if path.startswith('actions/workflows/'):
            workflow = path.split('/')[2]
            self.assertIn(f'head_sha={SHA}', path)
            return {'workflow_runs': self.runs[workflow]}
        if path.startswith('actions/runs/'):
            self.assertIn('filter=latest', path)
            return {'jobs': self.jobs}
        self.fail(f'Unexpected API request: {path}')

    def assert_state(self, expected):
        ci.report(self.pr)
        path, payload = self.statuses[-1]
        self.assertEqual(path, f'statuses/{SHA}')
        self.assertEqual(payload['context'], 'ci/trusted-builds')
        self.assertEqual(payload['state'], expected)
        self.assertLessEqual(len(payload['description']), 140)

    def test_local_only_pr_success_does_not_satisfy_gate(self):
        workflow = 'parallel-cluster-ci.yaml'
        self.runs[workflow] = [run(workflow, event='pull_request')]
        self.runs['nix.yaml'] = []
        self.assert_state('pending')

    def test_both_trusted_workflows_and_jobs_must_pass(self):
        self.assert_state('success')
        for workflow in ci.WORKFLOWS:
            with self.subTest(workflow=workflow):
                self.runs[workflow][0]['conclusion'] = 'failure'
                self.assert_state('failure')
                self.runs[workflow][0]['conclusion'] = 'success'

    def test_new_head_cannot_reuse_old_commit_or_fork_results(self):
        workflow = 'parallel-cluster-ci.yaml'
        for changes in ({'head_sha': 'b' * 40},
                        {'head_repository': {'full_name': 'contributor/nativelink'}},
                        {'path': '.github/workflows/another.yaml'},
                        {'event': 'schedule'}):
            with self.subTest(changes=changes):
                self.runs[workflow] = [run(workflow, **changes)]
                self.assert_state('pending')

    def test_latest_failure_or_rerun_overrides_previous_success(self):
        workflow = 'parallel-cluster-ci.yaml'
        for changes, expected in (({'conclusion': 'failure'}, 'failure'),
                                  ({'status': 'in_progress', 'conclusion': None}, 'pending'),
                                  ({'conclusion': 'cancelled'}, 'failure')):
            with self.subTest(changes=changes):
                self.runs[workflow] = [run(workflow, id=20, **changes), run(workflow)]
                self.assert_state(expected)

    def test_manual_branch_builds_count_but_skipped_jobs_do_not(self):
        for workflow in ci.WORKFLOWS:
            self.runs[workflow][0]['event'] = 'workflow_dispatch'
        self.assert_state('success')
        for job in self.jobs:
            job['conclusion'] = 'skipped'
            self.assert_state('failure')
            job['conclusion'] = 'success'
        self.jobs = []
        self.assert_state('failure')

    def test_web_only_changes_follow_existing_path_exemptions(self):
        self.files = [{'filename': 'web/apps/docs/page.mdx'},
                      {'filename': '.github/styles/style.yml'}]
        self.pr['changed_files'] = 2
        self.assert_state('success')
        self.assertFalse(any(path.startswith('actions/') for path in self.requests))

    def test_renames_and_incomplete_file_lists_cannot_skip_builds(self):
        self.runs['nix.yaml'] = []
        for files in ([{'filename': 'web/file.rs', 'previous_filename': 'src/file.rs'}],
                      [], [{'filename': 'web/page.mdx'}, {'filename': 'web/other.mdx'}]):
            with self.subTest(files=files):
                self.files = files
                self.assert_state('pending')

    def test_api_error_clears_previous_success(self):
        self.assert_state('success')
        original = self.respond

        def failed_api(path, payload=None):
            if path.startswith('actions/'):
                raise RuntimeError('API unavailable')
            return original(path, payload)

        with patch.object(ci, 'api', side_effect=failed_api), self.assertRaises(RuntimeError):
            ci.report(self.pr)
        self.assertEqual(self.statuses[-1][1]['state'], 'pending')

    def event(self, name, **data):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'event.json'
            path.write_text(json.dumps({'repository': {'full_name': ci.REPOSITORY}, **data}))
            with patch.dict(os.environ, GITHUB_EVENT_PATH=str(path), GITHUB_EVENT_NAME=name):
                ci.main()

    def test_branch_completion_finds_fork_pr_without_run_pr_metadata(self):
        self.event('workflow_run', workflow_run=run('nix.yaml', pull_requests=[]))
        self.assertEqual(self.statuses[-1][1]['state'], 'success')

    def test_fork_workflow_events_cannot_publish_status(self):
        for changes in ({'event': 'pull_request'},
                        {'head_repository': {'full_name': 'contributor/nativelink'}}):
            self.event('workflow_run', workflow_run=run('nix.yaml', **changes))
        self.assertEqual(self.statuses, [])

    def test_stale_pr_event_does_not_write_another_heads_status(self):
        self.event('pull_request_target', number=3210,
                   pull_request={'head': {'sha': 'b' * 40}})
        self.assertEqual(self.statuses, [])
        self.event('pull_request_target', number=3210, pull_request=PR)
        self.assertEqual(self.statuses[-1][1]['state'], 'success')

    def test_closed_or_other_base_prs_do_not_publish(self):
        self.pr['state'] = 'closed'
        ci.report(self.pr)
        self.pr['state'] = 'open'
        self.pr['base']['ref'] = 'release'
        ci.report(self.pr)
        self.assertEqual(self.statuses, [])

    def test_pagination_does_not_drop_later_results(self):
        with patch.object(ci, 'api', side_effect=[{'jobs': [{}] * 100}, {'jobs': [{'id': 101}]}]):
            self.assertEqual(len(list(ci.pages('actions/runs/10/jobs?filter=latest', 'jobs'))), 101)

    def test_invalid_sha_is_rejected_before_any_status_write(self):
        self.pr['head']['sha'] = 'main'
        with self.assertRaises(ValueError):
            ci.report(self.pr)
        self.assertEqual(self.statuses, [])


if __name__ == '__main__':
    unittest.main()
