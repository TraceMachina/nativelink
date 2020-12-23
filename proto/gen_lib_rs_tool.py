# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.
"""Generates a lib.rs file from input proto files."""

import argparse
import os
from pathlib import Path

_HEADER = "// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved."


def print_package_part_to_mod(package_parts, path_and_name, indents = 0):
  tabs = "  " * indents
  if len(package_parts) <= 0:
      print('%sinclude!("%s.pb.rs");' % (tabs, path_and_name))
      return
  assert package_parts[0] not in ['.', '..'], "'%s.proto' is not in proper root directory" % (path_and_name, )
  print("%spub mod %s {" % (tabs, package_parts[0]))
  print_package_part_to_mod(package_parts[1:], path_and_name, indents + 1)
  print("%s}" % (tabs, ))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rootdir', default=os.getcwd(),
                        help='Module paths will be relative to this path. Default CWD.')
    parser.add_argument('files', nargs='+',
                        help='pb.rs files used to generate lib.rs')
    args = parser.parse_args()
    print(_HEADER)
    print("")
    for filepath in args.files:
        filepath = os.path.normpath(filepath)
        package_name = ""
        with open(filepath, mode="r") as f:
            for line in f.readlines():
              line = line.strip("\n\r\t ")
              if line.startswith("package "):
                  assert line.endswith(";"), "'package' line must end with a semicolon"
                  package_name = line[len("package "):-1]
        assert package_name, "Could not find 'package' line in proto file: %s" % filepath
        filepath = os.path.relpath(filepath, args.rootdir)
        dirname = os.path.dirname(filepath)
        file_parts = list(Path(dirname).parts)
        package_parts = file_parts + package_name.split('.')
        path_and_name = os.path.join(dirname, os.path.basename(filepath).split('.')[0])
        print_package_part_to_mod(package_parts, path_and_name)
        print("")


if __name__ == "__main__":
    main()
