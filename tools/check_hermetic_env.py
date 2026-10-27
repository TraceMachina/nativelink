"""Forbid compile-time environment paths that survive into compiled output.

The hermetic build rejects binaries that embed the absolute working
directory, and the usual way one gets in is `env!("CARGO_MANIFEST_DIR")` or
`env!("OUT_DIR")` retained in compiled code. That rejection happens late,
on the cluster lane, after a push. This check fails the same code at
commit time.

A compile-time include such as `include_str!`, `include_bytes!` or
`include!(concat!(env!("OUT_DIR"), ...))` embeds the file's contents, not
its path, so occurrences inside those macros are allowed. Code that needs
the manifest directory at run time reads the `CARGO_MANIFEST_DIR`
environment variable instead, which cargo also sets when tests run.
"""

import os
from pathlib import Path
import re
import sys

FORBIDDEN = re.compile(r'(?:option_)?env!\s*\(\s*"(?:CARGO_MANIFEST_DIR|OUT_DIR)"\s*\)')
# A compile-time include opening shortly before the match makes it legal.
COMPILE_TIME_INCLUDE = re.compile(
    r"(?:include_str!|include_bytes!|include!)\s*\(\s*(?:concat!\s*\(\s*)?$"
)

SELF_TEST = (
    ('let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));', True),
    ('include!(concat!(env!("OUT_DIR"), "/generated.rs"));', False),
    ('include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/data.json"))', False),
    ('include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/img.bin"))', False),
    ('let home = option_env!("OUT_DIR");', True),
    ('let other = env!("SOME_OTHER_VAR");', False),
)


def violations(contents):
    found = []
    for match in FORBIDDEN.finditer(contents):
        preceding = contents[max(0, match.start() - 80) : match.start()]
        if not COMPILE_TIME_INCLUDE.search(preceding):
            line = contents.count("\n", 0, match.start()) + 1
            found.append((line, match.group(0)))
    return found


def self_test():
    for sample, expect_violation in SELF_TEST:
        if bool(violations(sample)) != expect_violation:
            print(f"check_hermetic_env self-test failed on: {sample}")
            return False
    return True


def main(root):
    if not self_test():
        return 1
    failed = False
    for dirstr, _dirnames, filenames in os.walk(root):
        dirpath = Path(dirstr)
        if "target" in dirpath.parts or ".git" in dirpath.parts:
            continue
        for filename in sorted(filenames):
            if not filename.endswith(".rs"):
                continue
            fullpath = dirpath.joinpath(filename)
            for line, text in violations(fullpath.read_text(errors="replace")):
                print(
                    f"{fullpath.relative_to(root)}:{line}: {text} survives into "
                    "compiled output and embeds the absolute working directory, "
                    "which the hermetic build rejects. Read the variable from "
                    "the runtime environment, or use it only inside "
                    "include_str!/include_bytes!/include! for compile-time "
                    "file access."
                )
                failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
