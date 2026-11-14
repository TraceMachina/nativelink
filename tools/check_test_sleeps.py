#!/usr/bin/env python3
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

"""A ratchet on wall-clock waits in tests (RFC-0064 invariant 2).

A test that orders events with a real sleep flakes under CI load, and a
boundary case one tick either side of a timeout cannot be written with one.
New tests use the mock clock, tokio's paused time or gates. The sleeps that
exist today are recorded per file in tools/test-sleeps-baseline.json; a file
may go down, never up, and a file not in the baseline may have none.

Usage: tools/check_test_sleeps.py [--update]
  --update  rewrite the baseline to today's counts (only ever lowers it in
            review: a raise shows in the diff)
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "tools" / "test-sleeps-baseline.json"
# The directories the ratchet covers; widen as each area adopts it.
SCOPES = ["nativelink-scheduler/tests", "nativelink-sim/tests"]
SLEEP = re.compile(r"\b(?:std::thread::sleep|thread::sleep|tokio::time::sleep|time::sleep)\s*\(")


def counts():
    found = {}
    for scope in SCOPES:
        for path in sorted((ROOT / scope).rglob("*.rs")):
            n = sum(1 for line in path.read_text().splitlines()
                    if SLEEP.search(line) and not line.lstrip().startswith("//"))
            if n:
                found[str(path.relative_to(ROOT))] = n
    return found


def main(argv):
    now = counts()
    if "--update" in argv:
        BASELINE.write_text(json.dumps(now, indent=2, sort_keys=True) + "\n")
        print(f"baseline written: {sum(now.values())} sleeps in {len(now)} files")
        return 0
    base = json.loads(BASELINE.read_text()) if BASELINE.exists() else {}
    worse = [(f, n, base.get(f, 0)) for f, n in now.items() if n > base.get(f, 0)]
    better = [(f, n, base[f]) for f, n in now.items() if f in base and n < base[f]]
    gone = [f for f in base if f not in now]
    for f, n, b in worse:
        print(f"{f}: {n} wall-clock sleeps, baseline {b}. Use the mock clock, "
              "tokio paused time or a gate instead.", file=sys.stderr)
    if better or gone:
        print("Fewer sleeps than the baseline; lower it with tools/check_test_sleeps.py --update.")
    return 1 if worse else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
