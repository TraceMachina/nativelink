#!/usr/bin/env python3
"""Stage a keyless PR build's event file and failed-test logs for ci_bep_replay.py.

Everything staged is untrusted; the consumer takes identity from workflow_run.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import xml.etree.ElementTree as ElementTree

SCHEMA = 1
EVENTS_NAME = 'events.pb'
MAX_EVENTS_BYTES = 64 << 20
MAX_LOG_BYTES = 2 << 20
MAX_TOTAL_LOG_BYTES = 32 << 20
MAX_FAILED_TARGETS = 50
LOG_NAMES = ('test.log', 'test.xml')
TRUNCATED_MARKER = b'[ci_bep_artifact: truncated, kept the last %d of %d bytes]\n'
# Bazel records the client environment as --client_env=NAME=value.
CLIENT_ENV = re.compile(rb'--client_env=([A-Za-z_][A-Za-z0-9_]*)=')
SENSITIVE_NAME = re.compile(r'TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|PRIVATE_KEY|API_KEY',
                            re.IGNORECASE)


def sensitive_env_names(events):
    """Credential-looking client environment names recorded in a BEP file."""
    with events.open('rb') as stream:
        names = {match.decode() for match in CLIENT_ENV.findall(stream.read())}
    return sorted(name for name in names if SENSITIVE_NAME.search(name))


def failed(xml_path):
    """True when a JUnit report records a failure or error, or cannot be read."""
    try:
        root = ElementTree.parse(xml_path).getroot()
    except (ElementTree.ParseError, OSError):
        # A crashed test can leave a truncated report; keep its log.
        return True
    return any(element.tag in ('failure', 'error') for element in root.iter())


def failed_test_directories(testlogs):
    """Directories under testlogs whose test.xml reports a failure, sorted."""
    if not testlogs.is_dir():
        return []
    found = []
    for xml_path in sorted(testlogs.rglob('test.xml')):
        if xml_path.is_symlink() or not xml_path.is_file():
            continue
        if failed(xml_path):
            found.append(xml_path.parent)
    return found


def copy_tail(source, destination, limit):
    """Copy at most limit bytes, keeping the tail where test failures print."""
    size = source.stat().st_size
    destination.parent.mkdir(parents=True, exist_ok=True)
    with source.open('rb') as reader, destination.open('wb') as writer:
        if size <= limit:
            shutil.copyfileobj(reader, writer)
            return size, False
        marker = TRUNCATED_MARKER % (limit, size)
        reader.seek(size - (limit - len(marker)))
        writer.write(marker)
        shutil.copyfileobj(reader, writer)
        return limit, True


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def record(path, root, original_size, truncated):
    return {'path': path.relative_to(root).as_posix(), 'bytes': path.stat().st_size,
            'original_bytes': original_size, 'truncated': truncated, 'sha256': digest(path)}


def identity(environ):
    names = ('GITHUB_REPOSITORY', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_JOB',
             'GITHUB_SHA', 'GITHUB_EVENT_NAME', 'GITHUB_HEAD_REF')
    return {name: environ[name] for name in names if environ.get(name)}


def collect(events, testlogs, output, environ):
    output.mkdir(parents=True, exist_ok=True)
    files, skipped = [], []

    if events.is_file() and not events.is_symlink():
        size = events.stat().st_size
        if size > MAX_EVENTS_BYTES:
            skipped.append({'path': EVENTS_NAME, 'reason': f'{size} bytes exceeds the cap'})
        elif names := sensitive_env_names(events):
            # Names only, never values.
            skipped.append({'path': EVENTS_NAME,
                            'reason': 'credential-like client environment: ' + ', '.join(names)})
        else:
            staged = output / EVENTS_NAME
            shutil.copyfile(events, staged)
            files.append(record(staged, output, size, False))
    else:
        skipped.append({'path': EVENTS_NAME, 'reason': 'not produced'})

    testlogs = testlogs.resolve() if testlogs.exists() else testlogs
    directories = failed_test_directories(testlogs)
    budget = MAX_TOTAL_LOG_BYTES
    for index, directory in enumerate(directories):
        relative = directory.relative_to(testlogs)
        if index >= MAX_FAILED_TARGETS:
            skipped.append({'path': relative.as_posix(), 'reason': 'failed-target cap'})
            continue
        for name in LOG_NAMES:
            source = directory / name
            if source.is_symlink() or not source.is_file():
                continue
            if budget <= len(TRUNCATED_MARKER) + 64:
                skipped.append({'path': (relative / name).as_posix(), 'reason': 'total log cap'})
                continue
            staged = output / 'testlogs' / relative / name
            original = source.stat().st_size
            _, truncated = copy_tail(source, staged, min(MAX_LOG_BYTES, budget))
            budget -= staged.stat().st_size
            files.append(record(staged, output, original, truncated))

    manifest = {'schema': SCHEMA, 'reported_identity': identity(environ),
                'failed_targets': len(directories), 'files': files, 'skipped': skipped}
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    return manifest


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--events', type=Path, required=True,
                        help='Binary BEP file written by --build_event_binary_file')
    parser.add_argument('--testlogs', type=Path, default=Path('bazel-testlogs'),
                        help='Bazel test log tree (the bazel-testlogs convenience symlink)')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(argv)
    if args.output.exists() and any(args.output.iterdir()):
        sys.exit(f'{args.output} must be empty')
    manifest = collect(args.events, args.testlogs, args.output, os.environ)
    staged = sum(entry['bytes'] for entry in manifest['files'])
    print(f"Staged {len(manifest['files'])} files ({staged} bytes); "
          f"{manifest['failed_targets']} failed test targets; "
          f"{len(manifest['skipped'])} skipped")
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
