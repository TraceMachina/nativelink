#!/usr/bin/env python3
"""Publish a fork PR's staged build events to the build-results UI.

Fork PRs get no secrets, so their Bazel lanes cannot stream BES. They stage
the binary BEP file as a bep-* artifact instead (ci_bep_artifact.py). A
trusted workflow_run job, running this file from the default branch, replays
that file as an ordered BES stream with a key that can publish build events
and nothing else.

Everything in the artifact is untrusted, so before it is sent:
- Identity comes only from the workflow_run event. The fork's build metadata
  is replaced, and workspace status entries that become tags or a reported
  user are dropped, so a fork build cannot pass as a trusted one.
- File links survive only as file:// or as bytestream:// on an allowlisted
  host, so the UI never links to a host the fork chose.
- The start and finish times are clamped to the triggering run.
- The invocation id is derived from the run and artifact, so a re-run of the
  replay overwrites the same build instead of adding a second one.

After publishing, each job's build gets a check run on the PR head that links
it and quotes the failing tests' log tails, which people without access to
the UI can still read. Without --publish this only reports what it would send.
"""

import argparse
import collections
import dataclasses
import datetime
import importlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import time
from urllib.parse import quote, urlencode, urlsplit
import urllib.error
import urllib.request
import uuid

PROTO_ROOT = Path(__file__).resolve().parents[2] / 'nativelink-proto'
BES_PROTO = 'src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto'
PROTO_FILES = (
    BES_PROTO,
    'src/main/java/com/google/devtools/build/lib/packages/metrics/package_load_metrics.proto',
    'src/main/protobuf/action_cache.proto',
    'src/main/protobuf/command_line.proto',
    'src/main/protobuf/failure_details.proto',
    'src/main/protobuf/invocation_policy.proto',
    'src/main/protobuf/option_filters.proto',
    'src/main/protobuf/strategy_policy.proto',
    'google/api/annotations.proto',
    'google/api/client.proto',
    'google/api/field_behavior.proto',
    'google/api/http.proto',
    'google/devtools/build/v1/build_events.proto',
    'google/devtools/build/v1/build_status.proto',
    'google/devtools/build/v1/publish_build_event.proto',
)
PUBLISH_METHOD = '/google.devtools.build.v1.PublishBuildEvent/PublishBuildToolEventStream'

# Same cap as the producer; anything larger was never staged.
MAX_EVENTS_BYTES = 64 << 20
MAX_EVENT_BYTES = 16 << 20
MAX_EVENTS = 1_000_000
# Names the replay ids so they cannot collide with ids Bazel generates.
ID_NAMESPACE = uuid.UUID('5a0c1f4e-8f0e-4d55-9a3b-6f1a3c2b7d10')
ARTIFACT_NAME = re.compile(r'bep-([A-Za-z0-9_-]{1,64})-([0-9]{1,3})')
SHA = re.compile(r'[0-9a-f]{40}')
COMMAND = re.compile(r'[a-z_-]{1,32}')
# Workspace status keys the ETL turns into tags or a reported user.
TAG_STATUS = re.compile(r'(STABLE_|GITHUB_|BUILDKITE_)')
REPORTED_STATUS = frozenset({'BUILD_USER'})
# Started may precede the run's recorded start by clock skew only.
CLOCK_SKEW = datetime.timedelta(minutes=5)
FINISHED_OK = frozenset({'PASSED', 'FLAKY'})
# Check run output; GitHub caps summary and text at 65535 characters each.
MAX_CHECK_TESTS = 10
MAX_TAIL_BYTES = 4096
MAX_CHECK_TEXT = 60000
CHECK_PREFIX = 'Build results / '
CHECK_JOB = re.compile(r'[A-Za-z0-9_ -]{1,80}')
CHECK_TITLE = re.compile(r'[A-Za-z0-9_ ]{1,80}')
# One comment per PR, found by this marker and rebuilt from the check runs.
COMMENT_MARKER = '<!-- fork-build-results -->'
COMMENT_AUTHOR = 'github-actions[bot]'
# Each lane's run rebuilds the comment, waits this long and rebuilds it
# again, so two lanes finishing together still end with both rows.
COMMENT_SETTLE_SECONDS = 15


def load_protos(output):
    """Generate Python modules for the vendored protos into output and import them."""
    from grpc_tools import protoc  # pylint: disable=import-outside-toplevel
    import importlib.resources  # pylint: disable=import-outside-toplevel
    well_known = importlib.resources.files('grpc_tools') / '_proto'
    code = protoc.main(['protoc', f'-I{PROTO_ROOT}', f'-I{well_known}',
                        f'--python_out={output}', *PROTO_FILES])
    if code:
        raise RuntimeError(f'protoc failed with {code}')
    sys.path.insert(0, str(output))
    importlib.invalidate_caches()
    module = BES_PROTO.removesuffix('.proto').replace('/', '.') + '_pb2'
    return Protos(bes=importlib.import_module(module),
                  events=importlib.import_module('google.devtools.build.v1.build_events_pb2'),
                  publish=importlib.import_module(
                      'google.devtools.build.v1.publish_build_event_pb2'))


@dataclasses.dataclass(frozen=True)
class Protos:
    bes: object
    events: object
    publish: object


def split_events(data):
    """The messages of a varint-length-delimited file, as Bazel writes BEP."""
    if len(data) > MAX_EVENTS_BYTES:
        raise ValueError(f'{len(data)} bytes exceeds the cap')
    position, out = 0, []
    while position < len(data):
        size, shift = 0, 0
        while True:
            if position >= len(data):
                raise ValueError('truncated length prefix')
            byte = data[position]
            position += 1
            size |= (byte & 0x7f) << shift
            if not byte & 0x80:
                break
            shift += 7
            if shift > 28:
                raise ValueError('length prefix too long')
        if size > MAX_EVENT_BYTES:
            raise ValueError(f'an event of {size} bytes exceeds the cap')
        if position + size > len(data):
            raise ValueError('truncated event')
        out.append(data[position:position + size])
        position += size
        if len(out) > MAX_EVENTS:
            raise ValueError('too many events')
    return out


def parse_time(value):
    parsed = datetime.datetime.fromisoformat(value.replace('Z', '+00:00'))
    if parsed.tzinfo is None:
        raise ValueError(f'{value} has no time zone')
    return parsed


@dataclasses.dataclass(frozen=True)
class Origin:
    """What the trusted workflow_run event says about the run being replayed."""

    repository: str
    run_id: int
    run_attempt: int
    run_number: int
    workflow: str
    head_sha: str
    head_repository: str
    head_branch: str
    actor: str
    started: datetime.datetime
    updated: datetime.datetime
    job: str
    job_index: int
    pull_request: int | None = None


def origin_from_event(event, artifact_name, pull_request=None):
    """Origin from a workflow_run event payload; refuses runs that are not fork PRs."""
    run = event['workflow_run']
    if run.get('event') != 'pull_request':
        raise ValueError('only pull_request runs are replayed')
    repository = event['repository']['full_name']
    head_repository = (run.get('head_repository') or {}).get('full_name') or ''
    if not head_repository or head_repository == repository:
        # Repository PRs hold the cluster key and stream BES themselves.
        raise ValueError('only fork PRs are replayed')
    if not SHA.fullmatch(run.get('head_sha', '')):
        raise ValueError('workflow_run has no head commit')
    match = ARTIFACT_NAME.fullmatch(artifact_name)
    if not match:
        raise ValueError(f'unexpected artifact name {artifact_name!r}')
    return Origin(repository=repository, run_id=int(run['id']),
                  run_attempt=int(run.get('run_attempt') or 1),
                  run_number=int(run['run_number']), workflow=run['name'],
                  head_sha=run['head_sha'], head_repository=head_repository,
                  head_branch=run.get('head_branch') or '',
                  actor=(run.get('actor') or {}).get('login') or '',
                  started=parse_time(run['run_started_at']), updated=parse_time(run['updated_at']),
                  job=match.group(1), job_index=int(match.group(2)),
                  pull_request=pull_request)


def replay_ids(origin):
    """(build_id, invocation_id), stable across re-runs of the replay job."""
    name = (f'{origin.repository}/{origin.run_id}/{origin.run_attempt}/'
            f'{origin.job}/{origin.job_index}')
    return (str(uuid.uuid5(ID_NAMESPACE, 'build:' + name)),
            str(uuid.uuid5(ID_NAMESPACE, 'invocation:' + name)))


def safe_value(value):
    return (isinstance(value, str) and 0 < len(value) <= 1024
            and not any(ord(char) < 32 or ord(char) == 127 for char in value))


def trusted_metadata(origin):
    """Build metadata for the replayed build, in the keys the trusted lanes use."""
    owner = origin.head_repository.split('/', 1)[0]
    values = {
        'ROLE': 'FORK_PR',
        'GITHUB_REPOSITORY': origin.repository,
        'GITHUB_SHA': origin.head_sha,
        'GITHUB_RUN_ID': str(origin.run_id),
        'GITHUB_RUN_ATTEMPT': str(origin.run_attempt),
        'GITHUB_RUN_NUMBER': str(origin.run_number),
        'GITHUB_EVENT_NAME': 'pull_request',
        'GITHUB_JOB': origin.job,
        'pipeline': origin.workflow,
        'commit': origin.head_sha,
        # Owner-qualified, so a fork branch named main reads as the fork's.
        'branch': f'{owner}:{origin.head_branch}',
        'head_repository': origin.head_repository,
        'actor': origin.actor,
    }
    if origin.pull_request:
        values['GITHUB_REF'] = f'refs/pull/{origin.pull_request}/merge'
        values['pull_request'] = str(origin.pull_request)
    return {key: value for key, value in values.items() if safe_value(value)}


def clamp(timestamp, low, high):
    """Clamp a Timestamp message in place; returns True when it moved."""
    if not timestamp.seconds and not timestamp.nanos:
        return False
    value = timestamp.ToDatetime(tzinfo=datetime.timezone.utc)
    bounded = min(max(value, low), high)
    if bounded != value:
        timestamp.FromDatetime(bounded)
        return True
    return False


def clamp_millis(millis, low, high):
    if not millis:
        return millis
    return int(min(max(millis, low.timestamp() * 1000), high.timestamp() * 1000))


def allowed_uri(uri, hosts):
    parsed = urlsplit(uri)
    if parsed.scheme == 'file':
        return True
    return parsed.scheme == 'bytestream' and parsed.hostname in hosts


def clear_untrusted_uris(message, hosts):
    """Clear every build_event_stream.File uri that is not allowed; returns the count."""
    cleared = 0
    for field, value in message.ListFields():
        if field.message_type is None:
            continue
        for item in value if field.is_repeated else (value,):
            if not hasattr(item, 'ListFields'):
                continue  # map keys
            if (item.DESCRIPTOR.full_name == 'build_event_stream.File'
                    and item.WhichOneof('file') == 'uri' and not allowed_uri(item.uri, hosts)):
                item.ClearField('uri')
                cleared += 1
            cleared += clear_untrusted_uris(item, hosts)
    return cleared


# On a cold repository cache Bazel downloads modules before the build starts
# and reports that as progress and fetch events ahead of `started`. Nothing
# else may come first.
BEFORE_STARTED = frozenset({'progress', 'fetch'})


def sanitize(protos, raw_events, origin, invocation_id, uri_hosts):
    """Decoded, rewritten events and a report of what changed."""
    bes = protos.bes
    metadata = trusted_metadata(origin)
    low, high = origin.started - CLOCK_SKEW, origin.updated + CLOCK_SKEW
    report = collections.Counter()
    events, saw_metadata, started_at = [], False, None
    for index, raw in enumerate(raw_events):
        event = bes.BuildEvent()
        event.ParseFromString(raw)
        kind = event.WhichOneof('payload')
        if kind == 'started':
            if started_at is not None:
                raise ValueError('the stream has more than one started event')
            started_at = len(events)
        elif started_at is None and kind not in BEFORE_STARTED:
            raise ValueError(f'a {kind} event comes before started')
        if kind == 'started':
            event.started.uuid = invocation_id
            report['clamped times'] += clamp(event.started.start_time, low, high)
            event.started.start_time_millis = clamp_millis(event.started.start_time_millis, low, high)
        elif kind == 'build_metadata':
            event.build_metadata.metadata.clear()
            event.build_metadata.metadata.update(metadata)
            saw_metadata = True
        elif kind == 'workspace_status':
            items = list(event.workspace_status.item)
            keep = [item for item in items
                    if item.key not in REPORTED_STATUS and not TAG_STATUS.match(item.key)]
            report['dropped status entries'] += len(items) - len(keep)
            del event.workspace_status.item[:]
            event.workspace_status.item.extend(keep)
        elif kind == 'finished':
            report['clamped times'] += clamp(event.finished.finish_time, low, high)
            event.finished.finish_time_millis = clamp_millis(
                event.finished.finish_time_millis, low, high)
        report['cleared links'] += clear_untrusted_uris(event, uri_hosts)
        events.append(event)
        if event.last_message:
            report['dropped after last message'] = len(raw_events) - index - 1
            break
    if started_at is None:
        raise ValueError('the stream has no started event')
    if not saw_metadata:
        injected = bes.BuildEvent()
        injected.id.build_metadata.SetInParent()
        injected.build_metadata.metadata.update(metadata)
        events.insert(started_at + 1, injected)
        report['injected metadata'] = 1
    return events, {key: value for key, value in report.items() if value}


def outcome(events):
    """Exit code name and failing test labels, for the check run and log."""
    exit_name, failed = None, []
    for event in events:
        kind = event.WhichOneof('payload')
        if kind == 'finished':
            exit_name = event.finished.exit_code.name or 'SUCCESS'
        elif kind == 'test_summary':
            statuses = event.test_summary.DESCRIPTOR.fields_by_name['overall_status'].enum_type
            status = statuses.values_by_number[event.test_summary.overall_status].name
            if status not in FINISHED_OK:
                failed.append({'label': event.id.test_summary.label, 'status': status})
    return exit_name, failed


def requests(protos, events, build_id, invocation_id, now):
    """The PublishBuildToolEventStream requests for events, in sequence order."""
    stream_id = protos.events.StreamId(build_id=build_id, invocation_id=invocation_id,
                                       component=protos.events.StreamId.TOOL)
    command = events[0].started.command
    out = []
    for sequence, event in enumerate(events, 1):
        request = protos.publish.PublishBuildToolEventStreamRequest()
        ordered = request.ordered_build_event
        ordered.stream_id.CopyFrom(stream_id)
        ordered.sequence_number = sequence
        ordered.event.event_time.FromDatetime(now)
        ordered.event.bazel_event.Pack(event)
        if sequence == 1:
            # What Bazel sends on its first event.
            name = command if COMMAND.fullmatch(command) else 'unknown'
            request.notification_keywords.extend([f'command_name={name}', 'protocol_name=BEP'])
        out.append(request)
    finished = protos.publish.PublishBuildToolEventStreamRequest()
    ordered = finished.ordered_build_event
    ordered.stream_id.CopyFrom(stream_id)
    ordered.sequence_number = len(events) + 1
    ordered.event.event_time.FromDatetime(now)
    ordered.event.component_stream_finished.type = (
        protos.events.BuildEvent.BuildComponentStreamFinished.FINISHED)
    out.append(finished)
    return out


def channel_for(endpoint):
    """A gRPC channel; plaintext only to loopback, for local testing."""
    import grpc  # pylint: disable=import-outside-toplevel
    parsed = urlsplit(endpoint)
    if not parsed.hostname or parsed.username or parsed.password or parsed.path not in ('', '/'):
        raise ValueError('Expected a bare build event endpoint without credentials')
    if parsed.scheme == 'grpcs':
        return grpc.secure_channel(f'{parsed.hostname}:{parsed.port or 443}',
                                   grpc.ssl_channel_credentials())
    if parsed.scheme == 'grpc' and parsed.hostname in ('localhost', '127.0.0.1', '::1'):
        return grpc.insecure_channel(f'{parsed.hostname}:{parsed.port or 80}')
    raise ValueError('Expected a grpcs:// build event endpoint')


def publish(protos, stream, endpoint, api_key, project, timeout=300, attempts=3):
    """Send one stream; returns the number of acknowledged events."""
    import grpc  # pylint: disable=import-outside-toplevel
    metadata = [('x-nativelink-project', project)] if project else []
    if api_key:
        if any(char in api_key for char in '\r\n'):
            raise ValueError('A valid API key is required')
        metadata.append(('x-nativelink-api-key', api_key))
    with channel_for(endpoint) as channel:
        call = channel.stream_stream(
            PUBLISH_METHOD,
            request_serializer=protos.publish.PublishBuildToolEventStreamRequest.SerializeToString,
            response_deserializer=protos.publish.PublishBuildToolEventStreamResponse.FromString)
        for attempt in range(1, attempts + 1):
            acked = 0
            try:
                for response in call(iter(stream), metadata=metadata, timeout=timeout):
                    acked += 1
                    if response.sequence_number != acked:
                        raise RuntimeError(f'acknowledgement {response.sequence_number} out of order')
                if acked == len(stream):
                    return acked
                error = f'{acked} of {len(stream)} events acknowledged'
            except grpc.RpcError as rpc_error:
                # The server's details, never the request metadata.
                error = f'{rpc_error.code().name}: {rpc_error.details()}'
            # The ids and sequence numbers are fixed, so a retry overwrites.
            print(f'::warning::Build event stream attempt {attempt} failed: {error}')
        raise RuntimeError(f'Build event stream failed after {attempts} attempts: {error}')


def results_link(base, invocation_id):
    parsed = urlsplit(base)
    if parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password:
        raise ValueError('Expected an https results URL without credentials')
    return base.rstrip('/') + '/' + invocation_id


def public_cache_host():
    """The read-only cache host fork PR lanes use, from setup-nativelink-cloud."""
    action = (PROTO_ROOT.parent / '.github/actions/setup-nativelink-cloud/action.yaml').read_text()
    match = re.search(r'\n  public-claim-base:\n(?:    .*\n|      .*\n)*?    default: "([a-z0-9.-]+)"',
                      action)
    if not match:
        raise ValueError('setup-nativelink-cloud has no public-claim-base default')
    return 'cas-' + match.group(1)


def github_request(path, token, body=None, method='GET'):
    base = os.environ.get('GITHUB_API_URL', 'https://api.github.com').rstrip('/')
    if urlsplit(base).scheme != 'https':
        raise ValueError('Expected an https GitHub API URL')
    headers = {'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28',
               'Authorization': 'Bearer ' + token}
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers['Content-Type'] = 'application/json'
    request = urllib.request.Request(base + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(request, timeout=30) as response:
        payload = response.read()
    return json.loads(payload) if payload else None


def find_pull_request(event, token, request=github_request):
    """The open PR whose head is this run's commit. workflow_run lists none for forks."""
    run = event['workflow_run']
    head = run['head_repository']
    query = urlencode({'state': 'open', 'per_page': 100,
                       'head': f"{head['owner']['login']}:{run['head_branch']}"})
    repository = quote(event['repository']['full_name'], safe='/')
    for pull in request(f'/repos/{repository}/pulls?{query}', token):
        if (pull['head']['sha'] == run['head_sha']
                and pull['head']['repo']['full_name'] == head['full_name']):
            return int(pull['number'])
    return None


def code(text):
    """Inline code that its own backticks cannot close."""
    text = ' '.join(str(text).split())
    fence = '`' * (max((len(run) for run in re.findall(r'`+', text)), default=0) + 1)
    return f'{fence} {text} {fence}'


def fenced(text):
    fence = '`' * max(3, max((len(run) for run in re.findall(r'`+', text)), default=0) + 1)
    return f'{fence}text\n{text.rstrip()}\n{fence}'


def log_tail(artifact, label):
    """The tail of a failed test's staged log, or None."""
    if not label.startswith('//') or ':' not in label:
        return None
    package, name = label[2:].split(':', 1)
    root = (artifact / 'testlogs').resolve()
    path = root / package / name / 'test.log'
    if path.is_symlink() or not path.is_file() or root not in path.resolve().parents:
        return None
    with path.open('rb') as stream:
        size = path.stat().st_size
        stream.seek(max(0, size - MAX_TAIL_BYTES))
        tail = stream.read().decode('utf-8', 'replace')
    return tail if size <= MAX_TAIL_BYTES else '…' + tail.split('\n', 1)[-1]


def result_title(exit_name, failed):
    """A short result for a check run title, the comment and the job summary."""
    if failed:
        return f'{len(failed)} failing test' + ('' if len(failed) == 1 else 's')
    if exit_name == 'SUCCESS':
        return 'Passed'
    return f'Bazel exited {exit_name}' if exit_name else 'No finished event'


def check_run(origin, invocation_id, link, exit_name, failed, artifact):
    """A completed check run that links the build and quotes its failing tests."""
    name = f'Build results / {origin.job}' + (f' {origin.job_index}' if origin.job_index else '')
    summary = [f'**[Open this build in the build-results UI]({link})**',
               '(Trace Machina sign-in required; failing test logs are below.)', '',
               f'{code(origin.workflow)} run {origin.run_id}, job {code(origin.job)}: '
               f'Bazel exited {code(exit_name or "without a finished event")}.']
    if failed:
        summary += ['', '| Failing test | Status |', '| --- | --- |']
        # GFM splits table cells on | even inside code spans.
        summary += ['| ' + code(test['label']).replace('|', r'\|') + f' | {test["status"]} |'
                    for test in failed[:MAX_CHECK_TESTS]]
        if len(failed) > MAX_CHECK_TESTS:
            summary.append(f'\n{len(failed) - MAX_CHECK_TESTS} more in the build results.')
    text = []
    for test in failed[:MAX_CHECK_TESTS]:
        tail = log_tail(artifact, test['label'])
        if tail is not None:
            text += [f'### {code(test["label"])}', fenced(tail), '']
    body = '\n'.join(text)
    if len(body) > MAX_CHECK_TEXT:
        body = body[:MAX_CHECK_TEXT] + '\n\n(truncated)'
    return {'name': name, 'head_sha': origin.head_sha, 'status': 'completed',
            'conclusion': 'neutral', 'details_url': link, 'external_id': invocation_id,
            'output': {'title': result_title(exit_name, failed),
                       'summary': '\n'.join(summary), 'text': body}}


def check_rows(repository, sha, token, results_url, request=github_request):
    """(job, result, check run URL, build link) for each build-results check on sha."""
    path = (f'/repos/{quote(repository, safe="/")}/commits/{sha}/check-runs'
            '?filter=latest&per_page=100')
    rows = []
    for run in request(path, token).get('check_runs', []):
        name = run.get('name') or ''
        if (not name.startswith(CHECK_PREFIX)
                or (run.get('app') or {}).get('slug') != 'github-actions'
                or not CHECK_JOB.fullmatch(name[len(CHECK_PREFIX):])
                or not str(run.get('html_url', '')).startswith('https://')):
            continue
        try:
            invocation_id = str(uuid.UUID(run.get('external_id') or ''))
        except ValueError:
            continue
        title = (run.get('output') or {}).get('title') or ''
        rows.append((name[len(CHECK_PREFIX):],
                     title if CHECK_TITLE.fullmatch(title) else 'See the check run',
                     run['html_url'], results_link(results_url, invocation_id)))
    return sorted(rows)


def comment_body(sha, rows):
    lines = [COMMENT_MARKER,
             f'**Build results** for this fork PR at `{sha[:12]}`:', '',
             '| Job | Result | Links |', '| --- | --- | --- |']
    lines += [f'| {job} | {result} | **[Open the build]({link})** · [check run]({check}) |'
              for job, result, check, link in rows]
    lines += ['', 'Fork PRs cannot stream build events, so a trusted workflow publishes '
              'them after CI finishes and updates this comment as each job is published. '
              'Opening a build needs a Trace Machina sign-in; the check run lists the '
              'failing tests and their logs.']
    return '\n'.join(lines)


def upsert_comment(repository, pull_request, body, token, request=github_request):
    """Update this workflow's comment on the PR, creating it or removing duplicates."""
    repository = quote(repository, safe='/')
    ours = []
    for page in range(1, 11):
        comments = request(f'/repos/{repository}/issues/{pull_request}/comments'
                           f'?per_page=100&page={page}', token)
        ours += [comment for comment in comments
                 if (comment.get('user') or {}).get('login') == COMMENT_AUTHOR
                 and (comment.get('body') or '').startswith(COMMENT_MARKER)]
        if len(comments) < 100:
            break
    if not ours:
        return request(f'/repos/{repository}/issues/{pull_request}/comments', token,
                       {'body': body}, method='POST')['id']
    ours.sort(key=lambda comment: comment['id'])
    for duplicate in ours[1:]:
        # Two lanes that finished together can both have created one.
        request(f'/repos/{repository}/issues/comments/{duplicate["id"]}', token, method='DELETE')
    if ours[0].get('body') != body:
        request(f'/repos/{repository}/issues/comments/{ours[0]["id"]}', token,
                {'body': body}, method='PATCH')
    return ours[0]['id']


def comment_on_pull_request(origin, token, results_url, request=github_request,
                            sleep=time.sleep):
    """Keep one PR comment listing every published build of the PR's head commit."""
    written = None
    for _ in range(2):
        rows = check_rows(origin.repository, origin.head_sha, token, results_url, request)
        body = comment_body(origin.head_sha, rows)
        if not rows or body == written:
            break
        upsert_comment(origin.repository, origin.pull_request, body, token, request)
        written = body
        sleep(COMMENT_SETTLE_SECONDS)
    return written


def job_summary(summaries):
    """Markdown for this run's job summary page."""
    lines = ['## Fork build results', '', '| Artifact | Result | Build |', '| --- | --- | --- |']
    for summary in summaries:
        name = code(summary['artifact']).replace('|', r'\|')
        if 'skipped' in summary:
            lines.append(f"| {name} | Skipped: {summary['skipped']} | |")
            continue
        link = summary.get('results_url')
        lines.append(f"| {name} | {result_title(summary['exit_code'], summary['failed_tests'])} | "
                     + (f'**[Open the build]({link})**' if link else 'Dry run, not published')
                     + ' |')
    if not summaries:
        lines.append('| | No bep-* artifacts in the triggering run | |')
    return '\n'.join(lines) + '\n'


def replay(protos, artifact, event, args, api_key, token, pull_request):
    """Replay one bep-* artifact directory; returns its summary."""
    origin = origin_from_event(event, artifact.name, pull_request)
    build_id, invocation_id = replay_ids(origin)
    events_path = artifact / 'events.pb'
    if events_path.is_symlink() or not events_path.is_file():
        return {'artifact': artifact.name, 'skipped': 'no events.pb'}
    events, report = sanitize(protos, split_events(events_path.read_bytes()), origin,
                              invocation_id, frozenset(args.uri_host or [public_cache_host()]))
    stream = requests(protos, events, build_id, invocation_id,
                      datetime.datetime.now(datetime.timezone.utc))
    exit_name, failed = outcome(events)
    summary = {'artifact': artifact.name, 'build_id': build_id, 'invocation_id': invocation_id,
               'events': len(events), 'exit_code': exit_name, 'failed_tests': failed,
               'sanitized': report, 'metadata': trusted_metadata(origin), 'published': False}
    if args.publish:
        summary['acknowledged'] = publish(protos, stream, args.endpoint, api_key, args.project)
        summary['published'] = True
        summary['results_url'] = link = results_link(args.results_url, invocation_id)
        if token:
            repository = quote(origin.repository, safe='/')
            github_request(f'/repos/{repository}/check-runs', token,
                           check_run(origin, invocation_id, link, exit_name, failed, artifact),
                           method='POST')
    return summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--artifacts', type=Path, required=True,
                        help='Directory holding the downloaded bep-* artifact directories')
    parser.add_argument('--event-path', type=Path, default=os.environ.get('GITHUB_EVENT_PATH'),
                        help='The workflow_run event payload (default: $GITHUB_EVENT_PATH)')
    parser.add_argument('--pull-request', type=int,
                        help='PR number; looked up with GITHUB_TOKEN when omitted')
    parser.add_argument('--uri-host', action='append',
                        help='bytestream:// host whose links are kept (default: the public cache)')
    parser.add_argument('--endpoint', help='grpcs:// build event endpoint')
    parser.add_argument('--project', default='nativelink-github-ci')
    parser.add_argument('--results-url', help='Base of the build results link')
    parser.add_argument('--summary', type=Path, help='Write a JSON summary here')
    parser.add_argument('--publish', action='store_true',
                        help='Send the streams and post check runs; the key is read from '
                             'NL_FORK_BEP_API_KEY and the GitHub token from GITHUB_TOKEN')
    args = parser.parse_args(argv)
    if not args.event_path:
        parser.error('--event-path or GITHUB_EVENT_PATH is required')
    api_key = os.environ.get('NL_FORK_BEP_API_KEY', '')
    token = os.environ.get('GITHUB_TOKEN', '')
    if args.publish and not (args.endpoint and args.results_url and api_key):
        parser.error('--publish needs --endpoint, --results-url and NL_FORK_BEP_API_KEY')

    event = json.loads(args.event_path.read_text())
    origin_from_event(event, 'bep-check-0')  # Refuse a run that is not a fork PR up front.
    pull_request = args.pull_request
    if pull_request is None and token:
        pull_request = find_pull_request(event, token)
    artifacts = sorted(path for path in args.artifacts.iterdir()
                       if path.is_dir() and not path.is_symlink()) if args.artifacts.is_dir() else []
    summaries = []
    with tempfile.TemporaryDirectory() as generated:
        protos = load_protos(Path(generated))
        for artifact in artifacts:
            if not ARTIFACT_NAME.fullmatch(artifact.name):
                summaries.append({'artifact': artifact.name, 'skipped': 'unexpected name'})
                continue
            summaries.append(replay(protos, artifact, event, args, api_key, token, pull_request))
    for summary in summaries:
        if 'skipped' in summary:
            print(f"Skipped {summary['artifact']}: {summary['skipped']}")
            continue
        verb = 'Published' if summary['published'] else 'Would publish'
        print(f"{verb} {summary['artifact']}: {summary['events']} events as invocation "
              f"{summary['invocation_id']} ({summary['exit_code']}, "
              f"{len(summary['failed_tests'])} failing "
              f"test{'' if len(summary['failed_tests']) == 1 else 's'}); "
              f"sanitized: {summary['sanitized'] or 'nothing'}")
        if summary.get('results_url'):
            print(f"Build results: {summary['results_url']}")
    if args.summary:
        args.summary.write_text(json.dumps(summaries, indent=2) + '\n')
    if step_summary := os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(step_summary, 'a', encoding='utf-8') as stream:
            stream.write(job_summary(summaries))
    if token and pull_request and any(summary.get('published') for summary in summaries):
        origin = origin_from_event(event, 'bep-comment-0', pull_request)
        try:
            comment_on_pull_request(origin, token, args.results_url)
        except (urllib.error.URLError, KeyError, TypeError) as error:
            # The builds are published and linked from their check runs.
            print(f'::warning::Could not update the PR comment: {error}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
