#!/usr/bin/env python3
"""Report LLVM bitcode lacking split-LTO metadata after a failed CFI link.

Checks generated artifacts rather than assuming RUSTFLAGS were applied to every
input (including dependencies and native libraries).
"""

import argparse
from collections import Counter
from pathlib import Path
import re
import subprocess
import sys

BITCODE = (b"BC\xc0\xde", b"\xde\xc0\x17\x0b")
SPLIT_FLAG = re.compile(rb'!"EnableSplitLTOUnit"\s*,\s*i32\s+(\d+)')


def archive_members(path):
    """Iterate GNU/BSD ar members without extracting temporary files."""
    data = path.read_bytes()
    if not data.startswith(b"!<arch>\n"):
        return
    offset = 8
    names = b""
    while offset + 60 <= len(data):
        header = data[offset : offset + 60]
        if header[58:60] != b"`\n":
            break
        try:
            size = int(header[48:58].strip())
        except ValueError:
            break
        start = offset + 60
        end = start + size
        if end > len(data):
            break
        name = header[:16].decode("utf-8", errors="replace").strip()
        member = memoryview(data)[start:end]
        if name == "//":
            names = bytes(member)
        elif name.startswith("#1/"):
            length = int(name[3:])
            name = bytes(member[:length]).decode("utf-8", errors="replace")
            member = member[length:]
        elif name.startswith("/") and name[1:].isdigit():
            at = int(name[1:])
            name = names[at:].split(b"/\n", 1)[0].decode("utf-8", errors="replace")
        else:
            name = name.rstrip("/")
        if member[:4] in BITCODE:
            yield name, bytes(member)
        offset = end + (size & 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("llvm_dis", type=Path)
    parser.add_argument("target_dir", type=Path)
    args = parser.parse_args()
    if not args.llvm_dis.is_file():
        parser.error(f"Missing nightly llvm-dis: {args.llvm_dis}")

    counts = Counter()
    offenders = Counter()
    examples = []
    errors = []
    archives = sorted(p for p in args.target_dir.rglob("*") if p.suffix in (".rlib", ".a"))
    objects = sorted(args.target_dir.rglob("*.o"))
    print(f"Inspecting {len(archives)} archives and {len(objects)} standalone objects", flush=True)

    def inspect(label, bitcode):
        process = subprocess.run(
            [str(args.llvm_dis), "-o", "-", "-"], input=bitcode,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
        )
        if process.returncode:
            counts["decode_error"] += 1
            if len(errors) < 10:
                errors.append((label, process.stderr.decode("utf-8", errors="replace")[:300]))
            return
        match = SPLIT_FLAG.search(process.stdout)
        split = bool(match and int(match.group(1)) == 1)
        counts["split=1" if split else "split=0 or missing"] += 1
        if not split:
            offenders[label.rsplit(" :: ", 1)[0]] += 1
            if len(examples) < 45:
                examples.append(label)

    for archive in archives:
        try:
            for member, bitcode in archive_members(archive):
                inspect(f"{archive} :: {member}", bitcode)
        except (OSError, ValueError) as exc:
            counts["archive_error"] += 1
            if len(errors) < 10:
                errors.append((str(archive), str(exc)))

    for obj in objects:
        try:
            with obj.open("rb") as file:
                if file.read(4) not in BITCODE:
                    continue
                file.seek(0)
                inspect(str(obj), file.read())
        except OSError as exc:
            counts["object_error"] += 1
            if len(errors) < 10:
                errors.append((str(obj), str(exc)))

    print("LLVM bitcode split-LTO metadata counts:")
    for name, count in sorted(counts.items()):
        print(f"  {name}: {count}")
    print("Archives containing modules without EnableSplitLTOUnit=1:")
    for name, count in offenders.most_common(40):
        print(f"  {count:4}  {name}")
    print("Examples (may include inputs not used by the failed link):")
    for name in examples:
        print(f"  {name}")
    for name, error in errors:
        print(f"Inspection warning: {name}: {error}", file=sys.stderr)
    if not offenders:
        print("No unsplit bitcode identified; inspect link inputs outside target/.")


if __name__ == "__main__":
    main()
