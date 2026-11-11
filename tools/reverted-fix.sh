#!/usr/bin/env bash
# Copyright 2026 Trace Machina, Inc. All rights reserved.
#
# Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#    See LICENSE file for details
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Shows that a regression test catches the bug it guards: reverse a fix's
# non-test changes in a throwaway worktree, run the named tests there, and
# expect them to fail. A test that still passes with its fix reverted does
# not guard that fix (RFC-0064 §3.1).
#
# Usage: tools/reverted-fix.sh <fix-commit> <cargo-package> <test-target> [test-filter...]
#   fix-commit    the commit (or range a..b) whose fix the tests guard
#   cargo-package e.g. nativelink-scheduler
#   test-target   the integration test file without .rs, or "lib" for unit tests
#   test-filter   optional names passed to the test binary
#
# The tests themselves are taken from the current checkout, so a test added
# after the fix still runs against the reverted code. Prints a summary to
# paste into the PR body. Exit 0 when the tests fail with the fix reverted
# (what we want), 1 when they pass, 2 on a setup error.
#
# It runs on HEAD: commit the test first. Set CARGO_TARGET_DIR to share a
# build cache; the worktree is removed on exit.

set -euo pipefail

if [[ $# -lt 3 ]]; then
    sed -n '/^# Usage/,/^# build cache/p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
fi

fix="$1"
package="$2"
target="$3"
shift 3
filters=("$@")

repo="$(git rev-parse --show-toplevel)"
head="$(git -C "$repo" rev-parse HEAD)"
work="$(mktemp -d "${TMPDIR:-/tmp}/reverted-fix.XXXXXX")"
cleanup() {
    git -C "$repo" worktree remove --force "$work" >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT

if [[ "$fix" == *..* ]]; then
    range="$fix"
else
    range="${fix}^..${fix}"
fi

git -C "$repo" worktree add --detach --quiet "$work" "$head"

# Reverse the fix outside test code only: the tests at HEAD are the ones
# being judged.
patch="$work/.reverted-fix.patch"
git -C "$repo" diff "$range" -- . \
    ':(exclude)*/tests/*' ':(exclude)tests/*' ':(exclude)*_test.rs' >"$patch"
if [[ ! -s "$patch" ]]; then
    echo "error: $range changes nothing outside test code" >&2
    exit 2
fi
if ! git -C "$work" apply --reverse --3way "$patch" >/dev/null 2>&1; then
    echo "error: $range does not reverse cleanly on $head; later commits changed the same lines" >&2
    exit 2
fi

test_args=(--test "$target")
if [[ "$target" == "lib" ]]; then
    test_args=(--lib)
fi

echo "Reverted ${range} (non-test files) on ${head:0:12}; running ${package} ${target} ${filters[*]:-}" >&2
set +e
(cd "$work" && cargo test --quiet -p "$package" "${test_args[@]}" -- "${filters[@]}") >"$work/.test.log" 2>&1
status=$?
set -e

summary="$(grep -E '^test result:|^test .* FAILED$|panicked at' "$work/.test.log" | head -20)"
if [[ $status -ne 0 ]]; then
    printf 'Reverted-fix run: with %s reverted, `cargo test -p %s %s %s` fails as it should:\n%s\n' \
        "$range" "$package" "${test_args[*]}" "${filters[*]:-}" "$summary"
    exit 0
fi
printf 'Reverted-fix run: with %s reverted, the tests still PASS; they do not guard this fix.\n%s\n' \
    "$range" "$summary"
exit 1
