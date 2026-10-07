#!/usr/bin/env python3
# Copyright 2026 The NativeLink Authors. All rights reserved.
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
"""Behavioral tests for tools/bazel-retry.sh.

Runs the real script against a fake `bazel` placed first on PATH, so the
retry/no-retry decision logic is exercised exactly as CI runs it. The fake
scripts its own per-invocation behavior from a plan file: one line per
attempt, `ok` or `fail:<stderr line>`, and appends to a call log so tests
can assert how many attempts the wrapper actually made.

Scenarios covered:
  - first-try success exits 0 after one attempt
  - a transient fetch error (HTTP 503) retries and succeeds
  - a transient remote-endpoint error (capabilities / UNAVAILABLE /
    connection refused) retries and succeeds
  - a non-transient failure (compile error) exits 1 with NO retry
  - three transient failures exhaust the retry budget and exit 1
  - arguments pass through to bazel unchanged

Run directly (no bazel needed): python3 tools/test-bazel-retry.py
"""

import os
import pathlib
import subprocess
import tempfile
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
RETRY_SH = REPO_ROOT / "tools" / "bazel-retry.sh"

FAKE_BAZEL = """#!/bin/bash
# Fake bazel: consumes one line of $PLAN per call, logs argv to $CALLS.
echo "$@" >> "$CALLS"
line=$(sed -n "$(wc -l < "$CALLS")p" "$PLAN")
case "$line" in
  ok) echo "INFO: Build completed successfully"; exit 0 ;;
  fail:*) echo "${line#fail:}"; exit 1 ;;
  *) echo "test error: plan exhausted" >&2; exit 97 ;;
esac
"""

TRANSIENT_FETCH = "fail:ERROR: GET returned 503 Service Unavailable"
TRANSIENT_GRPC = (
    "fail:ERROR: Failed to query remote execution capabilities: "
    "Connection refused: cas-example:443"
)
NON_TRANSIENT = "fail:ERROR: compilation of rust_library foo failed"


class BazelRetryTest(unittest.TestCase):
    def run_retry(self, plan, args=("test", "//..."), timeout=120):
        tmp = pathlib.Path(tempfile.mkdtemp(prefix="bazel-retry-test."))
        bindir = tmp / "bin"
        bindir.mkdir()
        fake = bindir / "bazel"
        fake.write_text(FAKE_BAZEL)
        fake.chmod(0o755)
        plan_file = tmp / "plan"
        plan_file.write_text("\n".join(plan) + "\n")
        calls = tmp / "calls"
        calls.touch()
        env = dict(
            os.environ,
            PATH=f"{bindir}:{os.environ['PATH']}",
            PLAN=str(plan_file),
            CALLS=str(calls),
        )
        proc = subprocess.run(
            ["bash", str(RETRY_SH), *args],
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        attempts = calls.read_text().splitlines()
        return proc, attempts

    def test_first_try_success(self):
        proc, attempts = self.run_retry(["ok"])
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(len(attempts), 1)

    def test_transient_fetch_error_retries_then_succeeds(self):
        proc, attempts = self.run_retry([TRANSIENT_FETCH, "ok"])
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(len(attempts), 2)
        self.assertIn("retrying", proc.stdout)

    def test_transient_remote_endpoint_error_retries_then_succeeds(self):
        proc, attempts = self.run_retry([TRANSIENT_GRPC, "ok"])
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(len(attempts), 2)

    def test_unavailable_status_retries(self):
        proc, attempts = self.run_retry(
            ["fail:ERROR: UNAVAILABLE: io exception", "ok"]
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertEqual(len(attempts), 2)

    def test_non_transient_error_does_not_retry(self):
        proc, attempts = self.run_retry([NON_TRANSIENT])
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(len(attempts), 1, "a compile error must not be retried")
        self.assertIn("non-transient", proc.stdout)

    def test_retry_budget_exhausts_after_three_attempts(self):
        proc, attempts = self.run_retry(
            [TRANSIENT_FETCH, TRANSIENT_FETCH, TRANSIENT_FETCH]
        )
        self.assertEqual(proc.returncode, 1)
        self.assertEqual(len(attempts), 3)
        self.assertIn("after 3 attempts", proc.stdout)

    def test_arguments_pass_through_unchanged(self):
        proc, attempts = self.run_retry(
            ["ok"], args=("run", "//:target", "--", "--flag=x y")
        )
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(attempts, ["run //:target -- --flag=x y"])


if __name__ == "__main__":
    unittest.main()
