"""Sanitizing and replaying a fork PR's staged build events."""

import concurrent.futures
import datetime
import json
import os
from pathlib import Path
import tempfile
import unittest

import ci_bep_replay

ROOT = Path(__file__).resolve().parents[2]
FORK_SHA = 'a' * 40
# CI installs the dependencies and sets this, so the proto tests cannot skip there.
REQUIRED = os.environ.get('BEP_REPLAY_TESTS_REQUIRED') == '1'


def event(**run):
    payload = {
        'id': 7, 'run_attempt': 2, 'run_number': 9, 'name': 'CI / Bazel',
        'event': 'pull_request', 'head_sha': FORK_SHA, 'head_branch': 'main',
        'head_repository': {'full_name': 'someone/nativelink', 'owner': {'login': 'someone'}},
        'actor': {'login': 'someone'},
        'run_started_at': '2026-11-07T12:00:00Z', 'updated_at': '2026-11-07T12:30:00Z',
    }
    payload.update(run)
    return {'repository': {'full_name': 'TraceMachina/nativelink'}, 'workflow_run': payload}


def delimited(*messages):
    out = b''
    for message in messages:
        size = len(message)
        while True:
            byte = size & 0x7f
            size >>= 7
            out += bytes([byte | (0x80 if size else 0)])
            if not size:
                break
        out += message
    return out


class SplitTest(unittest.TestCase):
    def test_round_trip(self):
        messages = [b'', b'x', b'y' * 300]
        self.assertEqual(ci_bep_replay.split_events(delimited(*messages)), messages)

    def test_truncation_and_bad_prefixes_are_rejected(self):
        for data in (delimited(b'abc')[:-1], b'\x80', b'\xff\xff\xff\xff\xff\x01'):
            with self.subTest(data=data), self.assertRaises(ValueError):
                ci_bep_replay.split_events(data)


class OriginTest(unittest.TestCase):
    def test_only_fork_pull_requests_are_replayed(self):
        for payload in (event(event='push'),
                        event(head_repository={'full_name': 'TraceMachina/nativelink',
                                               'owner': {'login': 'TraceMachina'}}),
                        event(head_repository=None), event(head_sha='HEAD')):
            with self.subTest(payload=payload['workflow_run']), self.assertRaises(ValueError):
                ci_bep_replay.origin_from_event(payload, 'bep-unit-tests-0')

    def test_artifact_name_must_be_one_the_lanes_produce(self):
        for name in ('bep-unit-tests', 'bep-../x-0', 'events-unit-tests-0', 'bep--0'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                ci_bep_replay.origin_from_event(event(), name)

    def test_ids_are_stable_per_run_attempt_and_job(self):
        origin = ci_bep_replay.origin_from_event(event(), 'bep-unit-tests-0')
        again = ci_bep_replay.origin_from_event(event(), 'bep-unit-tests-0')
        self.assertEqual(ci_bep_replay.replay_ids(origin), ci_bep_replay.replay_ids(again))
        others = {ci_bep_replay.replay_ids(ci_bep_replay.origin_from_event(payload, name))
                  for payload, name in ((event(run_attempt=3), 'bep-unit-tests-0'),
                                        (event(), 'bep-unit-tests-1'),
                                        (event(), 'bep-pull-request-0'))}
        self.assertNotIn(ci_bep_replay.replay_ids(origin), others)
        self.assertEqual(len(others), 3)

    def test_metadata_marks_the_fork_and_qualifies_its_branch(self):
        origin = ci_bep_replay.origin_from_event(event(), 'bep-unit-tests-0', 12)
        metadata = ci_bep_replay.trusted_metadata(origin)
        self.assertEqual(metadata['ROLE'], 'FORK_PR')
        self.assertEqual(metadata['branch'], 'someone:main')
        self.assertEqual(metadata['GITHUB_REF'], 'refs/pull/12/merge')
        self.assertEqual(metadata['GITHUB_SHA'], FORK_SHA)
        self.assertNotIn('USER', metadata)

    def test_control_characters_never_reach_metadata(self):
        origin = ci_bep_replay.origin_from_event(event(head_branch='x\ny'), 'bep-unit-tests-0')
        self.assertNotIn('branch', ci_bep_replay.trusted_metadata(origin))


class GitHubTest(unittest.TestCase):
    def test_pull_request_is_matched_on_head_commit_and_repository(self):
        pulls = [{'number': 1, 'head': {'sha': 'b' * 40, 'repo': {'full_name': 'someone/nativelink'}}},
                 {'number': 2, 'head': {'sha': FORK_SHA, 'repo': {'full_name': 'other/nativelink'}}},
                 {'number': 3, 'head': {'sha': FORK_SHA, 'repo': {'full_name': 'someone/nativelink'}}}]
        paths = []
        found = ci_bep_replay.find_pull_request(
            event(head_branch='a&b'), 'token', lambda path, token: paths.append(path) or pulls)
        self.assertEqual(found, 3)
        self.assertIn('head=someone%3Aa%26b', paths[0])

    def test_markdown_from_the_fork_cannot_escape_its_code_span(self):
        self.assertEqual(ci_bep_replay.code('a ``b`` c'), '``` a ``b`` c ```')
        self.assertTrue(ci_bep_replay.fenced('x\n```\ny').startswith('````text\n'))

    def test_check_run_links_the_build_and_quotes_failing_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / 'bep-unit-tests-0'
            log = artifact / 'testlogs/pkg/a_test/test.log'
            log.parent.mkdir(parents=True)
            log.write_text('early\n' * 2000 + 'panicked here\n')
            origin = ci_bep_replay.origin_from_event(event(), artifact.name)
            failed = [{'label': '//pkg:a_test', 'status': 'FAILED'},
                      {'label': '//pkg:odd|name', 'status': 'TIMEOUT'},
                      {'label': '//../..:escape', 'status': 'FAILED'}]
            check = ci_bep_replay.check_run(origin, 'inv', 'https://ui.example/builds/inv',
                                            'TESTS_FAILED', failed, artifact)
        self.assertEqual(check['head_sha'], FORK_SHA)
        self.assertEqual(check['conclusion'], 'neutral')
        self.assertEqual(check['output']['title'], '3 failing tests')
        self.assertIn('(https://ui.example/builds/inv)', check['output']['summary'])
        self.assertIn(r'odd\|name', check['output']['summary'])
        text = check['output']['text']
        self.assertTrue(text.rstrip().endswith('panicked here\n```'))
        self.assertLess(len(text), ci_bep_replay.MAX_TAIL_BYTES + 500)
        self.assertNotIn('escape', text)

    def test_public_cache_host_comes_from_the_setup_action(self):
        host = ci_bep_replay.public_cache_host()
        action = (ROOT / '.github/actions/setup-nativelink-cloud/action.yaml').read_text()
        self.assertTrue(host.startswith('cas-'))
        self.assertIn(f'default: "{host.removeprefix("cas-")}"', action)


def proto_dependencies():
    try:
        import grpc  # noqa: F401 pylint: disable=import-outside-toplevel,unused-import
        import grpc_tools  # noqa: F401 pylint: disable=import-outside-toplevel,unused-import
    except ImportError:
        return False
    return True


@unittest.skipUnless(REQUIRED or proto_dependencies(),
                     'grpcio-tools is not installed (see requirements-bep-replay.txt)')
class ProtoTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.generated = tempfile.TemporaryDirectory()
        cls.protos = ci_bep_replay.load_protos(Path(cls.generated.name))
        cls.origin = ci_bep_replay.origin_from_event(event(), 'bep-unit-tests-0', 12)

    @classmethod
    def tearDownClass(cls):
        cls.generated.cleanup()

    def build_event(self, kind, **fields):
        bes = self.protos.bes
        message = bes.BuildEvent()
        getattr(message.id, {'started': 'started', 'build_metadata': 'build_metadata',
                             'workspace_status': 'workspace_status', 'action': 'action_completed',
                             'finished': 'build_finished', 'progress': 'progress'}[kind]).SetInParent()
        getattr(message, kind).SetInParent()
        for name, value in fields.items():
            setattr(message, name, value)
        return message

    def stream(self):
        started = self.build_event('started')
        started.started.uuid = 'chosen-by-the-fork'
        started.started.command = 'test'
        started.started.start_time.FromDatetime(datetime.datetime(2099, 1, 1))
        metadata = self.build_event('build_metadata')
        metadata.build_metadata.metadata.update({'ROLE': 'CI', 'branch': 'main', 'USER': 'admin'})
        status = self.build_event('workspace_status')
        for key, value in (('BUILD_USER', 'admin'), ('BUILD_HOST', 'runner'),
                           ('STABLE_ROLE', 'CI'), ('GITHUB_REF', 'refs/heads/main')):
            status.workspace_status.item.add(key=key, value=value)
        action = self.build_event('action')
        action.action.stdout.uri = 'https://attacker.example/log'
        action.action.stderr.uri = 'bytestream://elsewhere.example/blobs/abc/1'
        action.action.primary_output.uri = 'bytestream://cas.example/blobs/abc/1'
        action.action.action_metadata_logs.add(name='log', uri='file:///home/runner/x')
        finished = self.build_event('finished', last_message=True)
        trailing = self.build_event('progress')
        return [started, metadata, status, action, finished, trailing]

    def sanitize(self, events):
        return ci_bep_replay.sanitize(self.protos, [e.SerializeToString() for e in events],
                                      self.origin, 'inv', frozenset({'cas.example'}))

    def test_identity_tags_links_and_times_are_rewritten(self):
        events, report = self.sanitize(self.stream())
        started, metadata, status, action, finished = events
        self.assertEqual(started.started.uuid, 'inv')
        self.assertEqual(started.started.start_time.ToDatetime(tzinfo=datetime.timezone.utc),
                         self.origin.updated + ci_bep_replay.CLOCK_SKEW)
        self.assertEqual(dict(metadata.build_metadata.metadata),
                         ci_bep_replay.trusted_metadata(self.origin))
        self.assertEqual([item.key for item in status.workspace_status.item], ['BUILD_HOST'])
        self.assertEqual(action.action.stdout.WhichOneof('file'), None)
        self.assertEqual(action.action.stderr.WhichOneof('file'), None)
        self.assertEqual(action.action.primary_output.uri, 'bytestream://cas.example/blobs/abc/1')
        self.assertEqual(action.action.action_metadata_logs[0].uri, 'file:///home/runner/x')
        self.assertTrue(finished.last_message)
        self.assertEqual(report, {'clamped times': 1, 'dropped status entries': 3,
                                  'cleared links': 2, 'dropped after last message': 1})

    def test_missing_metadata_is_injected_after_started(self):
        events, report = self.sanitize([self.stream()[0], self.stream()[4]])
        self.assertEqual(events[1].WhichOneof('payload'), 'build_metadata')
        self.assertEqual(events[1].build_metadata.metadata['ROLE'], 'FORK_PR')
        self.assertEqual(report['injected metadata'], 1)

    def test_only_progress_and_fetch_may_precede_the_only_started_event(self):
        stream = self.stream()
        fetch = self.protos.bes.BuildEvent()
        fetch.id.fetch.url = 'https://github.com/example/rules.tar.gz'
        fetch.fetch.success = True
        progress = self.build_event('progress')
        events, report = self.sanitize([progress, fetch, stream[0], stream[4]])
        self.assertEqual([e.WhichOneof('payload') for e in events],
                         ['progress', 'fetch', 'started', 'build_metadata', 'finished'])
        self.assertEqual(events[2].started.uuid, 'inv')
        self.assertEqual(report['injected metadata'], 1)
        for events in ([stream[1], stream[0]], [stream[0], stream[0]], [progress, stream[4]],
                       [stream[3], stream[0]]):
            with self.subTest(events=[e.WhichOneof('payload') for e in events]):
                with self.assertRaises(ValueError):
                    self.sanitize(events)

    def test_requests_follow_what_bazel_sends(self):
        events, _ = self.sanitize(self.stream())
        now = datetime.datetime(2026, 11, 7, tzinfo=datetime.timezone.utc)
        stream = ci_bep_replay.requests(self.protos, events, 'build', 'inv', now)
        self.assertEqual([r.ordered_build_event.sequence_number for r in stream],
                         list(range(1, len(events) + 2)))
        first = stream[0]
        self.assertEqual(list(first.notification_keywords), ['command_name=test', 'protocol_name=BEP'])
        self.assertFalse(stream[1].notification_keywords)
        ordered = first.ordered_build_event
        self.assertEqual((ordered.stream_id.build_id, ordered.stream_id.invocation_id),
                         ('build', 'inv'))
        self.assertEqual(ordered.stream_id.component, self.protos.events.StreamId.TOOL)
        self.assertEqual(ordered.event.bazel_event.type_url,
                         'type.googleapis.com/build_event_stream.BuildEvent')
        self.assertEqual(stream[-1].ordered_build_event.event.WhichOneof('event'),
                         'component_stream_finished')

    def test_outcome_reports_exit_and_failing_tests(self):
        bes = self.protos.bes
        summary = bes.BuildEvent()
        summary.id.test_summary.label = '//pkg:a_test'
        summary.test_summary.overall_status = bes.TestStatus.FAILED
        flaky = bes.BuildEvent()
        flaky.id.test_summary.label = '//pkg:b_test'
        flaky.test_summary.overall_status = bes.TestStatus.FLAKY
        finished = self.build_event('finished')
        finished.finished.exit_code.name = 'TESTS_FAILED'
        self.assertEqual(ci_bep_replay.outcome([summary, flaky, finished]),
                         ('TESTS_FAILED', [{'label': '//pkg:a_test', 'status': 'FAILED'}]))

    def serve(self, handler):
        import grpc  # pylint: disable=import-outside-toplevel
        server = grpc.server(concurrent.futures.ThreadPoolExecutor(max_workers=2))
        publish = self.protos.publish
        server.add_generic_rpc_handlers([grpc.method_handlers_generic_handler(
            'google.devtools.build.v1.PublishBuildEvent',
            {'PublishBuildToolEventStream': grpc.stream_stream_rpc_method_handler(
                handler, request_deserializer=publish.PublishBuildToolEventStreamRequest.FromString,
                response_serializer=publish.PublishBuildToolEventStreamResponse.SerializeToString)})])
        port = server.add_insecure_port('localhost:0')
        server.start()
        self.addCleanup(server.stop, None)
        return f'grpc://localhost:{port}'

    def test_publish_sends_the_key_and_requires_every_acknowledgement(self):
        received, headers = [], []

        def handler(requests, context):
            headers.extend(context.invocation_metadata())
            for request in requests:
                received.append(request)
                yield self.protos.publish.PublishBuildToolEventStreamResponse(
                    stream_id=request.ordered_build_event.stream_id,
                    sequence_number=request.ordered_build_event.sequence_number)

        endpoint = self.serve(handler)
        events, _ = self.sanitize(self.stream())
        stream = ci_bep_replay.requests(self.protos, events, 'build', 'inv',
                                        datetime.datetime.now(datetime.timezone.utc))
        acked = ci_bep_replay.publish(self.protos, stream, endpoint, 'fork-key', 'proj')
        self.assertEqual(acked, len(stream))
        self.assertEqual(len(received), len(stream))
        sent = dict(headers)
        self.assertEqual(sent['x-nativelink-api-key'], 'fork-key')
        self.assertEqual(sent['x-nativelink-project'], 'proj')

    def test_publish_retries_then_fails_without_echoing_the_key(self):
        import grpc  # pylint: disable=import-outside-toplevel
        calls = []

        def handler(requests, context):
            calls.append(1)
            next(requests)
            context.abort(grpc.StatusCode.PERMISSION_DENIED, 'no BES publish for this key')
            yield  # pragma: no cover

        endpoint = self.serve(handler)
        events, _ = self.sanitize(self.stream())
        stream = ci_bep_replay.requests(self.protos, events, 'build', 'inv',
                                        datetime.datetime.now(datetime.timezone.utc))
        with self.assertRaises(RuntimeError) as raised:
            ci_bep_replay.publish(self.protos, stream, endpoint, 'fork-key', 'proj', attempts=2)
        self.assertEqual(len(calls), 2)
        self.assertIn('PERMISSION_DENIED', str(raised.exception))
        self.assertNotIn('fork-key', str(raised.exception))

    def test_plaintext_is_refused_off_loopback(self):
        for endpoint in ('grpc://bes.example:443', 'https://bes.example',
                         'grpcs://user:pw@bes.example', 'grpcs://bes.example/path'):
            with self.subTest(endpoint=endpoint), self.assertRaises(ValueError):
                ci_bep_replay.channel_for(endpoint)

    def test_cli_dry_run_reports_without_publishing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'event.json').write_text(json.dumps(event()))
            artifact = root / 'bep/bep-unit-tests-0'
            artifact.mkdir(parents=True)
            (artifact / 'events.pb').write_bytes(
                delimited(*(e.SerializeToString() for e in self.stream())))
            (root / 'bep/stray').mkdir()
            ci_bep_replay.main(['--artifacts', str(root / 'bep'), '--event-path',
                                str(root / 'event.json'), '--summary', str(root / 'out.json')])
            summaries = json.loads((root / 'out.json').read_text())
        self.assertEqual([s['artifact'] for s in summaries], ['bep-unit-tests-0', 'stray'])
        self.assertFalse(summaries[0]['published'])
        self.assertEqual(summaries[1]['skipped'], 'unexpected name')


class WorkflowTest(unittest.TestCase):
    """The publisher runs trusted code only, and only for fork PR runs."""

    def setUp(self):
        self.text = (ROOT / '.github/workflows/fork-build-results.yaml').read_text()

    def test_triggers_on_the_lanes_that_stage_events(self):
        names = [line.split(':', 1)[1].strip() for workflow in ('native-bazel.yaml',
                                                                 'parallel-cluster-ci.yaml')
                 for line in (ROOT / '.github/workflows' / workflow).read_text().splitlines()
                 if line.startswith('name:')]
        self.assertIn(f'workflows: {json.dumps(names)}', self.text)

    def test_runs_only_for_fork_pull_requests_with_least_privilege(self):
        self.assertIn("github.event.workflow_run.event == 'pull_request'", self.text)
        self.assertIn('github.event.workflow_run.head_repository.full_name != github.repository',
                      self.text)
        self.assertIn('\npermissions: {}\n', self.text)
        for line in ('pull_request_target', 'head_sha', 'head_branch', 'ref:'):
            self.assertNotIn(line, self.text)
        self.assertIn('--require-hashes', self.text)
        self.assertEqual(self.text.count('secrets.'), 1)

    def test_pre_commit_runs_these_tests_with_their_dependencies(self):
        text = (ROOT / '.github/workflows/pre-commit.yaml').read_text()
        self.assertIn('requirements-bep-replay.txt', text)
        self.assertIn('BEP_REPLAY_TESTS_REQUIRED: "1"', text)


if __name__ == '__main__':
    unittest.main()
