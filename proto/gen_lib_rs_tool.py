# Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.
"""Generates a lib.rs file from input proto files."""

import argparse
import os
from pathlib import Path

_HEADER = """\
// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.
// *** DO NOT MODIFY ***
// This file is auto-generated to provide the libs for the proto files.
"""


def print_package_part_to_mod(tree, indents = 0):
  tabs = "  " * indents
  if tree["filename"] is not None:
      print('%sinclude!("%s");' % (tabs, tree["filename"]))

  for mod_name, value in tree["children"].items():
      print("%spub mod %s {" % (tabs, mod_name))
      print_package_part_to_mod(value, indents + 1)
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
    tree_root = { "children": {}, "filename": None }
    for filepath in args.files:
        filepath = os.path.relpath(os.path.normpath(filepath), args.rootdir)
        assert filepath.endswith('.pb.rs'), "Expected " + filepath + " to end in '.pb.rs'"
        package_parts = filepath.split('.')[:-2]  # Remove `.pb.rs'.
        assert '.' not in package_parts and '..' not in package_parts, \
            "'%s' is not in proper root directory" % (filepath, )
        cur_node = tree_root
        for part in package_parts:
            if part not in cur_node["children"]:
                cur_node["children"][part] = { "children": {}, "filename": None }
            cur_node = cur_node["children"][part]
        assert not cur_node["filename"], "Duplicate package '%s'" % ('.'.join(package_parts), )
        cur_node["filename"] = '.'.join(package_parts) + '.pb.rs'

    print_package_part_to_mod(tree_root)
    print("")


if __name__ == "__main__":
    main()
