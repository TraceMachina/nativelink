#!/usr/bin/env python3
"""Choose fork CI using identities from GitHub and policy from the base branch."""

import json
import os
from pathlib import Path


REPOSITORY = 'TraceMachina/nativelink'


def contributors():
    return json.loads(Path(__file__).resolve().parents[1].joinpath(
        'trusted-contributors.json').read_text())


def trusted_fork(pr, actor, entries):
    repo = pr['head'].get('repo') or {}
    owner = repo.get('owner') or {}
    author = pr.get('user') or {}
    if (pr['base']['repo']['full_name'] != REPOSITORY
            or actor.get('id') not in {entry['user_id'] for entry in entries}):
        return False
    return any(repo.get('id') == entry['repository_id']
               and repo.get('full_name', '').lower() == entry['repository'].lower()
               and owner.get('id') == author.get('id') == entry['user_id']
               for entry in entries)


def trusted_source(pr, actor, entries):
    # Pushing into the upstream repository already authorizes branch CI.
    return ((pr['head'].get('repo') or {}).get('full_name') == REPOSITORY
            or trusted_fork(pr, actor, entries))


def main():
    # This script and the list must come from the trusted PR base, never its head.
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    if os.environ['GITHUB_EVENT_NAME'] != 'pull_request':
        raise ValueError('The hosted routing step only handles pull requests')
    trusted = trusted_fork(event['pull_request'], event['sender'], contributors())
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
        output.write(f'local={str(not trusted).lower()}\n')
    print('Using trusted branch CI' if trusted else 'Using local fork CI')


if __name__ == '__main__':
    main()
