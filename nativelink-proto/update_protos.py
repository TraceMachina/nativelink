# Copyright 2020 The TensorFlow Authors. All Rights Reserved.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#   http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
# ==============================================================================
"""Update proto source files based on generated outputs."""

import os
import sys
import shutil

# Paths to "nativelink-proto" directory in (a) the Bazel runfiles tree,
# whence we can read data dependencies, and (b) the Git repository,
# whither we can write output files.
_BAZEL_DIR = os.path.join("nativelink-proto")
_REPO_DIR = os.path.join(os.path.dirname(os.path.realpath(__file__)), "genproto")

_RUST_LICENSE = """\
// Copyright 2024 The NativeLink Authors. All rights reserved.
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

"""


def expected_contents(pkg):
    src = runfiles_file_path(pkg)
    with open(src, "rb") as infile:
        contents = infile.read()
    return _RUST_LICENSE.encode("utf-8") + contents


def repo_file_path(pkg):
    return os.path.join(_REPO_DIR, "%s.pb.rs" % pkg)


def runfiles_file_path(pkg):
    return os.path.join(_BAZEL_DIR, "%s.pb.rs" % pkg)


def update(proto_packages):
    for pkg in proto_packages:
        with open(repo_file_path(pkg), "wb") as outfile:
            outfile.write(expected_contents(pkg))
    with open(_REPO_DIR + "/lib.rs", "wb") as outfile:
        with open(_BAZEL_DIR + "/lib.rs", "rb") as infile:
            outfile.write(infile.read())


def check(proto_packages):
    failed = False
    for pkg in proto_packages:
        dst = repo_file_path(pkg)
        try:
            expected = expected_contents(pkg)
            with open(dst, "rb") as infile:
                actual = infile.read()
        except OSError as e:
            failed = True
            print("Could not read package %s: %s" % (pkg, e))
            continue
        # Ignore differences between newlines on Unix and Windows.
        if expected.splitlines() == actual.splitlines():
            print("%s OK" % dst)
        else:
            print("%s out of date" % dst)
            failed = True

    # Now check the lib.rs file.
    dst = _REPO_DIR + "/lib.rs"
    try:
        with open(_BAZEL_DIR + "/lib.rs", "rb") as infile:
            expected = infile.read()
        with open(dst, "rb") as infile:
            actual = infile.read()
    except OSError as e:
        failed = True
        print("Could not read package lib.rs: %s" % e)
    # Ignore differences between newlines on Unix and Windows.
    if expected.splitlines() == actual.splitlines():
        print("%s OK" % dst)
    else:
        print("%s out of date" % dst)
        failed = True

    if failed:
        print("To update, run: 'bazel run nativelink-proto:update_protos'")
        raise SystemExit(1)


def main():
    (mode, *proto_packages) = sys.argv[1:]
    if mode == "--update":
        shutil.rmtree(_REPO_DIR, ignore_errors=True)
        os.mkdir(_REPO_DIR)
        return update(proto_packages)
    if mode == "--check":
        return check(proto_packages)
    raise ValueError("unknown mode: %r" % mode)


if __name__ == "__main__":
    main()
