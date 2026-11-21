"""Prevent green local-only PR checks from standing in for trusted builds."""

import copy
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
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
        self.entry = ci.ci_trust.contributors()[0]
        self.actor = {'id': self.entry['user_id']}
        self.pr['user'] = self.actor.copy()
        self.pr['head']['repo'] = {'id': self.entry['repository_id'],
                                  'full_name': self.entry['repository'], 'owner': self.actor.copy()}
        self.refs = []
        self.dispatches = []
        self.api = patch.object(ci, 'api', side_effect=self.respond)
        self.api.start()
        self.addCleanup(self.api.stop)

    def respond(self, path, payload=None):
        self.requests.append(path)
        if path.startswith('statuses/'):
            self.statuses.append((path, payload))
            return {}
        if path.startswith('git/matching-refs/'):
            return self.refs
        if path == 'git/refs':
            self.refs.append({'ref': payload['ref'], 'object': {'sha': payload['sha']}})
            return self.refs[-1]
        if path.endswith('/dispatches'):
            self.dispatches.append((path, payload))
            workflow = path.split('/')[2]
            self.runs[workflow].append(run(workflow, status='queued', conclusion=None))
            return None
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
            path.write_text(json.dumps({'repository': {'full_name': ci.REPOSITORY},
                                        'sender': self.actor, 'action': 'opened', **data}))
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

    def test_trusted_fork_dispatches_each_missing_workflow_once_without_nightly_jobs(self):
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        ci.report(self.pr, self.actor)
        self.assertEqual(len(self.refs), 1)
        self.assertEqual(self.refs[0]['object']['sha'], SHA)
        self.assertEqual(len(self.dispatches), 2)
        for path, payload in self.dispatches:
            self.assertEqual(payload['ref'], f'ci-approved/pr-3210-{SHA}')
            self.assertEqual(payload['inputs'], {'scope': 'branch'} if '/nix.yaml/' in path
                             else {'repetitions': '1'})
        self.assertEqual(self.statuses[-1][1]['state'], 'pending')
        ci.report(self.pr, self.actor)
        self.assertEqual(len(self.refs), 1)
        self.assertEqual(len(self.dispatches), 2)

    def test_existing_success_failure_and_in_progress_runs_are_not_dispatched_again(self):
        for changes in ({}, {'conclusion': 'failure'}, {'status': 'in_progress'}):
            self.runs = {workflow: [run(workflow, **changes)] for workflow in ci.WORKFLOWS}
            ci.report(self.pr, self.actor)
        self.assertEqual(self.refs, [])
        self.assertEqual(self.dispatches, [])

    def test_partial_dispatch_only_starts_the_missing_workflow(self):
        self.runs['nix.yaml'] = []
        ci.report(self.pr, self.actor)
        self.assertEqual(len(self.dispatches), 1)
        self.assertIn('/nix.yaml/', self.dispatches[0][0])

    def test_unlisted_actor_or_fork_cannot_promote(self):
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        ci.report(self.pr, {'id': 999})
        self.pr['head']['repo']['id'] = 999
        ci.report(self.pr, self.actor)
        self.assertEqual(self.refs, [])
        self.assertEqual(self.dispatches, [])

    def test_same_repository_pr_reuses_push_builds_without_promotion(self):
        self.pr['head']['repo'] = {'full_name': ci.REPOSITORY, 'id': 999,
                                  'owner': {'id': 999}}
        ci.report(self.pr, self.actor)
        self.assertEqual(self.statuses[-1][1]['state'], 'success')
        self.assertEqual(self.refs, [])

    def test_internal_pr_with_path_filtered_push_gets_missing_head_builds(self):
        # A web-only last commit can skip push CI even when the PR also changes
        # Rust. Removing the hosted duplicate must not leave that PR stuck.
        self.pr['head']['repo'] = {'full_name': ci.REPOSITORY}
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        ci.report(self.pr, {'id': 999})
        self.assertEqual(len(self.dispatches), 2)

    def test_stale_revision_cannot_be_promoted(self):
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        stale = copy.deepcopy(self.pr)
        stale['head']['sha'] = 'b' * 40
        with patch.object(ci, 'branch_runs', return_value=[]):
            ci.dispatch_missing(stale, self.actor)
        self.assertEqual(self.refs, [])
        self.assertEqual(self.dispatches, [])

    def test_existing_promotion_branch_is_never_force_updated(self):
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        self.refs = [{'ref': f'refs/heads/ci-approved/pr-3210-{SHA}',
                      'object': {'sha': 'b' * 40}}]
        with self.assertRaisesRegex(ValueError, 'exact PR commit'):
            ci.report(self.pr, self.actor)
        self.assertEqual(self.dispatches, [])

    def test_rejected_branch_creation_stays_pending_without_dispatch(self):
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        original = self.respond

        def rejected_api(path, payload=None):
            if path == 'git/refs':
                raise RuntimeError('GitHub denied branch creation')
            return original(path, payload)

        with patch.object(ci, 'api', side_effect=rejected_api), self.assertRaises(RuntimeError):
            ci.report(self.pr, self.actor)
        self.assertEqual(self.statuses[-1][1]['state'], 'pending')
        self.assertEqual(self.dispatches, [])

    def test_web_only_trusted_fork_does_not_dispatch_builds(self):
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        self.files = [{'filename': 'web/page.mdx'}]
        ci.report(self.pr, self.actor)
        self.assertEqual(self.statuses[-1][1]['state'], 'success')
        self.assertEqual(self.dispatches, [])

    def test_workflow_completion_and_body_edits_never_promote(self):
        self.runs = {workflow: [] for workflow in ci.WORKFLOWS}
        self.event('workflow_run', workflow_run=run('nix.yaml'))
        self.event('pull_request_target', number=3210, pull_request=PR, action='edited')
        self.assertEqual(self.dispatches, [])


class ApiTest(unittest.TestCase):
    def test_dispatch_accepts_githubs_empty_204_response(self):
        with patch.object(ci.subprocess, 'run', return_value=SimpleNamespace(stdout='')) as call:
            self.assertIsNone(ci.api('actions/workflows/nix.yaml/dispatches',
                                     {'ref': 'branch', 'inputs': {'scope': 'branch'}}))
            self.assertEqual(call.call_args.args[0][-2:], ['--input', '-'])

    def test_nix_automatic_scope_excludes_nightly_jobs(self):
        workflow = Path('.github/workflows/nix.yaml').read_text()
        self.assertIn('options: [all, branch]', workflow)
        for name in ('nix-cargo', 'integration'):
            job = workflow.split('  ' + name + ':\n', 1)[1]
            condition = job.splitlines()[0]
            # Prevent automatic dispatch from silently adding the four nightly
            # jobs while keeping scheduled and full manual coverage available.
            self.assertIn("github.event_name == 'schedule'", condition)
            self.assertIn("github.event_name == 'workflow_dispatch' && inputs.scope != 'branch'",
                          condition)


if __name__ == '__main__':
    unittest.main()
