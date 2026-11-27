#!/usr/bin/env python3
"""Write allowlisted GitHub Actions metadata for Bazel's workspace rc import."""

import datetime
import json
import os
from pathlib import Path
import subprocess
import urllib.error
import urllib.parse
import urllib.request

FILENAME = 'ci-metadata.bazelrc'
KEYS = ('pipeline', 'job', 'runner', 'job_created', 'job_started', 'commit', 'branch')


def safe_values(values):
    """Never accept arbitrary Bazel flags, multiline values or environment dumps."""
    return {key: value for key in KEYS
            if isinstance(value := values.get(key), str) and 0 < len(value) <= 1024
            and not any(ord(char) < 32 or ord(char) == 127 for char in value)}


def write_rc(directory, values):
    values = safe_values(values)
    # Quote the entire option: Bazel rc parsing removes these quotes but keeps
    # whitespace, apostrophes, hashes and backslashes inside a single argument.
    lines = []
    for key, value in values.items():
        option = f'--build_metadata={key}={value}'.replace('\\', '\\\\').replace('"', '\\"')
        lines.append(f'build "{option}"\n')
    (Path(directory) / FILENAME).write_text(''.join(lines), encoding='utf-8')
    return values


def restore(directory, environ):
    """Replace metadata from the previous job in a retained checkout."""
    try:
        values = json.loads(environ.get('CI_BAZEL_METADATA', '{}'))
    except (ValueError, TypeError):
        values = {}
    return write_rc(directory, values if isinstance(values, dict) else {})


def request_json(url, token):
    headers = {'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=5) as response:
        return json.load(response)


def current_job(environ, request=request_json):
    """Match the running job within this attempt, including matrix jobs."""
    runner = environ.get('RUNNER_NAME')
    if not runner:
        return None
    base = environ.get('GITHUB_API_URL', 'https://api.github.com').rstrip('/')
    if urllib.parse.urlsplit(base).scheme != 'https':
        return None
    repo = urllib.parse.quote(environ['GITHUB_REPOSITORY'], safe='/')
    run = int(environ['GITHUB_RUN_ID'])
    attempt = int(environ['GITHUB_RUN_ATTEMPT'])
    matches = []
    for page in range(1, 6):
        result = request(f'{base}/repos/{repo}/actions/runs/{run}/attempts/{attempt}/jobs?per_page=100&page={page}',
                         environ.get('GITHUB_TOKEN', ''))
        jobs = result['jobs']
        matches.extend(job for job in jobs if job.get('status') == 'in_progress'
                       and job.get('runner_name') == runner)
        if page * 100 >= result['total_count']:
            return matches[0] if len(matches) == 1 else None
    return None


def timestamp(value):
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.datetime.fromisoformat(value.replace('Z', '+00:00'))
    except ValueError:
        return None
    if parsed.tzinfo is None:
        return None
    return parsed.astimezone(datetime.timezone.utc).isoformat().replace('+00:00', 'Z')


def collect(environ, revision, request=request_json):
    values = {'pipeline': environ.get('GITHUB_WORKFLOW'), 'job': environ.get('GITHUB_JOB'),
              'runner': '/'.join(environ.get(k, '') for k in
                                 ('RUNNER_ENVIRONMENT', 'RUNNER_OS', 'RUNNER_ARCH')),
              'commit': revision,
              'branch': environ.get('GITHUB_HEAD_REF') or environ.get('GITHUB_REF_NAME')}
    try:
        job = current_job(environ, request)
    except (OSError, ValueError, KeyError, TypeError):
        job = None
    if job:
        values.update(job=job.get('name'), job_created=timestamp(job.get('created_at')),
                      job_started=timestamp(job.get('started_at')))
    else:
        # Do not substitute workflow creation or step execution for job timing.
        print('::notice::Job timestamps unavailable; continuing without them.')
    return safe_values(values)


def main():
    environ = os.environ
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    values = write_rc(Path.cwd(), collect(environ, revision))
    # Only this allowlist crosses into the retained client; never the API token.
    if environ.get('GITHUB_ENV'):
        with open(environ['GITHUB_ENV'], 'a', encoding='utf-8') as output:
            output.write('CI_BAZEL_METADATA=' + json.dumps(values, ensure_ascii=True) + '\n')


if __name__ == '__main__':
    main()
