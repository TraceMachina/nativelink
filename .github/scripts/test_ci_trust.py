"""Trust exact GitHub identities, never author strings or a fork's own policy."""

import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import ci_trust


def pull(entry):
    return {'user': {'id': entry['user_id']},
            'base': {'repo': {'full_name': ci_trust.REPOSITORY}},
            'head': {'repo': {'id': entry['repository_id'], 'full_name': entry['repository'],
                              'owner': {'id': entry['user_id']}}}}


class TrustTest(unittest.TestCase):
    def setUp(self):
        self.entries = ci_trust.contributors()
        self.entry = self.entries[0]
        self.pr = pull(self.entry)
        self.actor = {'id': self.entry['user_id']}

    def test_all_six_verified_forks_are_trusted(self):
        self.assertEqual({e['login'].lower() for e in self.entries},
                         {'amankrx', 'palfrey', 'corcillo', 'marcussorealheis',
                          'modernmedici', 'matdexir'})
        for entry in self.entries:
            with self.subTest(login=entry['login']):
                self.assertTrue(ci_trust.trusted_fork(pull(entry), {'id': entry['user_id']},
                                                     self.entries))

    def test_author_owner_repository_and_triggering_account_must_match_policy(self):
        changes = [('author', lambda p: p['user'].update(id=999)),
                   ('owner', lambda p: p['head']['repo']['owner'].update(id=999)),
                   ('recreated repo', lambda p: p['head']['repo'].update(id=999)),
                   ('other repo', lambda p: p['head']['repo'].update(full_name='amankrx/other')),
                   ('other base', lambda p: p['base']['repo'].update(full_name='other/nativelink')),
                   ('deleted repo', lambda p: p['head'].update(repo=None))]
        for name, mutate in changes:
            with self.subTest(change=name):
                pr = copy.deepcopy(self.pr)
                mutate(pr)
                self.assertFalse(ci_trust.trusted_fork(pr, self.actor, self.entries))
        self.assertFalse(ci_trust.trusted_fork(self.pr, {'id': 999}, self.entries))
        self.assertFalse(ci_trust.trusted_fork(self.pr, {}, self.entries))

    def test_other_listed_maintainer_can_update_a_trusted_fork(self):
        self.assertTrue(ci_trust.trusted_fork(self.pr, {'id': self.entries[1]['user_id']},
                                             self.entries))

    def test_login_casing_does_not_change_identity(self):
        self.pr['head']['repo']['full_name'] = self.entry['repository'].upper()
        self.assertTrue(ci_trust.trusted_fork(self.pr, self.actor, self.entries))

    def test_empty_base_policy_cannot_be_overridden_by_event_data(self):
        self.pr['trusted_contributors'] = self.entries
        self.assertFalse(ci_trust.trusted_fork(self.pr, self.actor, []))

    def test_routing_skips_only_the_trusted_forks_local_suite(self):
        for trusted in (True, False):
            with self.subTest(trusted=trusted), tempfile.TemporaryDirectory() as directory:
                event, output = Path(directory) / 'event.json', Path(directory) / 'output'
                actor = self.actor if trusted else {'id': 999}
                event.write_text(json.dumps({'pull_request': self.pr, 'sender': actor}))
                with patch.dict(os.environ, GITHUB_EVENT_NAME='pull_request',
                                GITHUB_EVENT_PATH=str(event), GITHUB_OUTPUT=str(output)):
                    ci_trust.main()
                self.assertEqual(output.read_text(), f'local={str(not trusted).lower()}\n')


if __name__ == '__main__':
    unittest.main()
