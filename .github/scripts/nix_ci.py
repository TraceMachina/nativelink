#!/usr/bin/env python3
"""Build one Nix closure and import that exact output in downstream CI jobs."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

SYSTEM = 'x86_64-linux'
TARGETS = {'nativelink': 'nativelink', 'installation': 'nativelink-is-executable-test',
           'toolchain': 'rbe-toolchain-with-nativelink-test', 'commands': 'rbe-toolchain-list'}
STORE_PATH = re.compile(r'/nix/store/[0-9a-z]{32}-[A-Za-z0-9+._?=-]+')


def outputs():
    expression = 'packages: {' + ''.join(
        f'{name} = packages.{attribute}.outPath;' for name, attribute in TARGETS.items()) + '}'
    result = subprocess.run(['nix', 'eval', '--json', f'.#packages.{SYSTEM}', '--apply', expression],
                            check=True, stdout=subprocess.PIPE, text=True)
    return json.loads(result.stdout)


def validate(report, revision):
    if (not re.fullmatch(r'[0-9a-f]{40}', revision) or report.get('revision') != revision
            or report.get('kind') != 'nix' or report.get('system') != SYSTEM
            or report.get('exit_code') != 0):
        raise ValueError('Nix artifact does not match this source and platform')
    paths = report['outputs']
    if set(paths) != set(TARGETS) or not all(STORE_PATH.fullmatch(p) for p in paths.values()):
        raise ValueError('Invalid Nix output paths')
    commands = report['matrix']['command']
    if (not commands or not isinstance(commands, list) or len(commands) != len(set(commands))
            or not all(re.fullmatch(r'[a-z0-9][a-z0-9-]{0,40}', c) for c in commands)):
        raise ValueError('Invalid toolchain command matrix')
    if not re.fullmatch(r'[0-9a-f]{64}', report['sha256']) or not 0 < report['bytes'] <= 4 * 1024**3:
        raise ValueError('Invalid Nix closure checksum or size')
    # Only constructed fields are retained, never arbitrary build output.
    return {'kind': 'nix', 'revision': revision, 'system': SYSTEM, 'exit_code': 0,
            'outputs': paths, 'matrix': {'command': commands}, 'sha256': report['sha256'],
            'bytes': report['bytes'], 'build_seconds': float(report['build_seconds'])}


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def build(directory, revision):
    if not re.fullmatch(r'[0-9a-f]{40}', revision):
        raise ValueError('Expected an exact source revision')
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'nix.json').unlink(missing_ok=True)
    before = time.monotonic()
    # Nix deduplicates the common NativeLink dependency across these outputs.
    # The retained client's sandbox returns EPERM when this build chmods the
    # flake source it is copying into the store, so the copy runs unsandboxed.
    subprocess.run(['nix', 'build', '-L', '--option', 'sandbox', 'false',
                    '--no-write-lock-file', '--out-link',
                    str(directory.resolve() / 'result'),
                    *('.#' + attribute for attribute in TARGETS.values())], check=True)
    paths = outputs()
    if set(paths) != set(TARGETS) or not all(STORE_PATH.fullmatch(p) for p in paths.values()):
        raise ValueError('Unexpected Nix build outputs')
    commands = json.loads(subprocess.check_output(
        [paths['commands'] + '/bin/rbe-toolchain-list'], text=True))
    closure = subprocess.check_output(['nix-store', '--query', '--requisites', *paths.values()],
                                      text=True).splitlines()
    if not set(paths.values()).issubset(closure) or not all(STORE_PATH.fullmatch(p) for p in closure):
        raise ValueError('Incomplete Nix runtime closure')
    archive = directory / 'closure.nar.gz'
    with archive.open('wb') as destination, gzip.GzipFile(
            filename='', fileobj=destination, mode='wb', compresslevel=1, mtime=0) as compressed:
        with subprocess.Popen(['nix-store', '--export', *closure], stdout=subprocess.PIPE) as process:
            shutil.copyfileobj(process.stdout, compressed)
            if process.wait():
                raise RuntimeError('Nix closure export failed')
    report = validate({'kind': 'nix', 'revision': revision, 'system': SYSTEM, 'exit_code': 0,
                       'outputs': paths, 'matrix': {'command': commands}, 'sha256': digest(archive),
                       'bytes': archive.stat().st_size, 'build_seconds': time.monotonic() - before}, revision)
    (directory / 'nix.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def import_closure(directory, revision):
    report = validate(json.loads((directory / 'nix.json').read_text()), revision)
    archive = directory / 'closure.nar.gz'
    if archive.stat().st_size != report['bytes'] or digest(archive) != report['sha256']:
        raise ValueError('Nix closure checksum does not match its manifest')
    if outputs() != report['outputs']:
        raise ValueError('Nix artifact outputs do not match the checked-out package definitions')
    # The artifact comes from this workflow's trusted build job. Legacy import
    # transfers the complete store closure without rebuilding its derivations:
    # https://nix.dev/manual/nix/2.34/command-ref/nix-store/import
    with gzip.open(archive, 'rb') as stream, subprocess.Popen(
            ['nix-store', '--import'], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL) as process:
        try:
            with process.stdin:
                shutil.copyfileobj(stream, process.stdin)
        except BrokenPipeError:
            pass
        if process.wait():
            raise RuntimeError('Nix closure import failed; refusing to rebuild on the runner')
    subprocess.run(['nix', 'path-info', *report['outputs'].values()],
                   stdout=subprocess.DEVNULL, check=True)
    print('Imported the verified Nix package and test closure; no downstream package build needed')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=('build', 'import'))
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    action = build if args.operation == 'build' else import_closure
    action(args.directory, os.environ['GITHUB_SHA'])
