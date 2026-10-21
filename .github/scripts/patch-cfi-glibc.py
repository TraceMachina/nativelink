#!/usr/bin/env python3
"""Work around Rust nightly CFI rejecting std's dlsym call into glibc.

Only the Rust standard library sources installed by rustup are changed, and only
for this CI run. The call to glibc's private __pthread_get_minstack symbol has
no LLVM CFI metadata; all other Rust code remains instrumented.
"""

import argparse
from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    content = path.read_text()
    if content.count(old) != 1:
        raise RuntimeError(f"expected exactly one unmodified anchor in {path}")
    path.write_text(content.replace(old, new, 1))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sysroot", type=Path)
    args = parser.parse_args()
    std = args.sysroot / "lib/rustlib/src/rust/library/std"

    replace_once(
        std / "src/lib.rs",
        "#![feature(cfg_sanitizer_cfi)]\n",
        "#![feature(cfg_sanitizer_cfi)]\n#![feature(sanitize)]\n",
    )
    replace_once(
        std / "src/sys/thread/unix.rs",
        '#[cfg(all(target_os = "linux", target_env = "gnu"))]\n'
        'unsafe fn min_stack_size(attr: *const libc::pthread_attr_t) -> usize {',
        '#[cfg(all(target_os = "linux", target_env = "gnu"))]\n'
        '// The private glibc symbol loaded with dlsym has no CFI type metadata.\n'
        '// Keep checking every other function, including the std callers.\n'
        '#[sanitize(cfi = "off")]\n'
        '#[inline(never)]\n'
        'unsafe fn min_stack_size(attr: *const libc::pthread_attr_t) -> usize {',
    )
    print("Scoped CFI exemption installed for std's glibc __pthread_get_minstack call")


if __name__ == "__main__":
    main()
