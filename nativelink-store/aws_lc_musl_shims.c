// Copyright 2026 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// The Nix clang used for Linux builds (LRE-CC under Bazel, `TARGET_CC` for
// cargo) compiles AWS-LC against glibc headers, which rename a few libc calls
// to their glibc-specific variants: `sscanf` and `strtol` become `__isoc23_*`
// under `_GNU_SOURCE` with glibc 2.38+, and `fopen` becomes `fopen64` under
// `_FILE_OFFSET_BITS=64`. The Rust side then links against musl, which only
// provides the plain names. These forwarders give the glibc names a
// definition on musl targets. build.rs compiles this file for cargo; the
// `aws_lc_musl_shims` target in BUILD.bazel does the same for Bazel.
//
// No libc header is included on purpose: the same glibc headers would rename
// the calls made here too. The musl functions are declared by hand under
// local names bound to the real symbols with asm labels (so the compiler does
// not treat them as builtins), and `FILE *` is passed through as an opaque
// pointer.

#include <stdarg.h>

extern int musl_vsscanf(const char *str, const char *format, va_list ap)
    __asm__("vsscanf");
extern long musl_strtol(const char *nptr, char **endptr, int base)
    __asm__("strtol");
extern void *musl_fopen(const char *pathname, const char *mode)
    __asm__("fopen");

int __isoc23_sscanf(const char *str, const char *format, ...) {
  va_list ap;
  va_start(ap, format);
  int ret = musl_vsscanf(str, format, ap);
  va_end(ap);
  return ret;
}

long __isoc23_strtol(const char *nptr, char **endptr, int base) {
  return musl_strtol(nptr, endptr, base);
}

void *fopen64(const char *pathname, const char *mode) {
  return musl_fopen(pathname, mode);
}
