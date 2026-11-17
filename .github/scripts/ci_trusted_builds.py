#!/usr/bin/env python3
"""Report trusted branch builds on a PR head without executing PR code."""

import json
import os
from pathlib import Path
import re
import subprocess


REPOSITORY = 'TraceMachina/nativelink'
CONTEXT = 'ci/trusted-builds'
WORKFLOWS = {
    'parallel-cluster-ci.yaml': 'NativeLink / Linux',
    'nix.yaml': 'Build shared Nix package',
}
GUIDE = (f'https://github.com/{REPOSITORY}/blob/main/'
         '.github/scripts/README-ci-benchmark.md#before-merging-a-pull-request')


def api(path, payload=None):
    command = ['gh', 'api', '--method', 'GET' if payload is None else 'POST',
               f'repos/{REPOSITORY}/{path}']
    if payload is not None:
        command += ['--input', '-']
    result = subprocess.run(command, input=None if payload is None else json.dumps(payload),
                            text=True, stdout=subprocess.PIPE, check=True)
    return json.loads(result.stdout)


def pages(path, key=None):
    separator = '&' if '?' in path else '?'
    page = 1
    while True:
        result = api(f'{path}{separator}per_page=100&page={page}')
        items = result if key is None else result[key]
        yield from items
        if len(items) < 100:
            return
        page += 1


def source_sha(value):
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{40}', value):
        raise ValueError('Expected a full source commit SHA')
    return value


def needs_builds(pr, files):
    # GitHub caps the files endpoint at 3,000 entries. An incomplete listing
    # must never exempt a PR, nor may a rename hide a removed source file.
    if not files or len(files) != pr['changed_files']:
        return True
    return any(not path.startswith(('web/', '.github/styles/'))
               for file in files
               for path in (file['filename'], file.get('previous_filename', file['filename'])))


def build_result(workflow, job_name, sha):
    runs = [run for run in pages(f'actions/workflows/{workflow}/runs?head_sha={sha}',
                                'workflow_runs')
            if run['head_sha'] == sha
            and run['event'] in ('push', 'workflow_dispatch')
            and run.get('head_repository', {}).get('full_name') == REPOSITORY
            and run['path'] == f'.github/workflows/{workflow}']
    if not runs:
        return 'pending', GUIDE
    # A newer failure/pending run must override an older success. Re-runs
    # retain the run ID; the API exposes that run's latest attempt below.
    run = max(runs, key=lambda item: item['id'])
    url = f'https://github.com/{REPOSITORY}/actions/runs/{run["id"]}'
    if run['status'] != 'completed':
        return 'pending', url
    if run['conclusion'] != 'success':
        return 'failure', url
    jobs = list(pages(f'actions/runs/{run["id"]}/jobs?filter=latest', 'jobs'))
    required = [job for job in jobs if job['name'] == job_name]
    # A green workflow whose credentialed job was skipped is not evidence.
    if len(required) != 1 or required[0]['conclusion'] != 'success':
        return 'failure', url
    return 'success', url


def status(sha, state, description, url=GUIDE):
    api(f'statuses/{sha}', {'context': CONTEXT, 'state': state,
                          'description': description, 'target_url': url})


def report(pr):
    if (pr['state'] != 'open' or pr['base']['ref'] != 'main'
            or pr['base']['repo']['full_name'] != REPOSITORY):
        return
    sha = source_sha(pr['head']['sha'])
    # Fail closed if any subsequent API request fails: don't leave stale green.
    status(sha, 'pending', 'Checking trusted branch Bazel and Nix results')
    files = list(pages(f'pulls/{pr["number"]}/files'))
    if not needs_builds(pr, files):
        status(sha, 'success', 'Only web/style files changed; branch builds are not required')
        return
    results = [build_result(workflow, job, sha) for workflow, job in WORKFLOWS.items()]
    for state, url in results:
        if state == 'failure':
            status(sha, 'failure', 'Trusted branch build failed or its required job was skipped', url)
            return
    if any(state == 'pending' for state, _ in results):
        status(sha, 'pending', 'Awaiting trusted Bazel and Nix builds; see maintainer instructions')
    else:
        status(sha, 'success', 'Trusted branch Bazel and Nix builds passed for this commit')


def main():
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    if event['repository']['full_name'] != REPOSITORY:
        raise ValueError('Only the upstream repository may publish trusted build status')
    if os.environ['GITHUB_EVENT_NAME'] == 'pull_request_target':
        # Re-read instead of trusting a possibly superseded synchronize event.
        pr = api(f'pulls/{int(event["number"])}')
        if pr['head']['sha'] == event['pull_request']['head']['sha']:
            report(pr)
    elif os.environ['GITHUB_EVENT_NAME'] == 'workflow_run':
        run = event['workflow_run']
        if (run['event'] not in ('push', 'workflow_dispatch')
                or run.get('head_repository', {}).get('full_name') != REPOSITORY):
            return
        sha = source_sha(run['head_sha'])
        # workflow_run.pull_requests can be empty for promoted fork commits.
        # Match open PR heads through the API, never through build artifacts.
        for pr in pages('pulls?state=open&base=main'):
            if pr['head']['sha'] == sha:
                current = api(f'pulls/{pr["number"]}')
                if current['head']['sha'] == sha:
                    report(current)
    else:
        raise ValueError('Unsupported trusted status event')


if __name__ == '__main__':
    main()
