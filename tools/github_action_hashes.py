# Check all Github workflow actions uses hashes, not version pins

import os
import sys
from types import NoneType
from typing import Any
import yaml

def walk_yaml(top: Any) -> bool:
    errors = False

    if isinstance(top, dict):
        for key, value in top.items():
            if key == "uses":
                if ".github/actions/" in value:
                    continue
                try:
                    _, version = value.strip().split("@")
                except ValueError:
                    print(value)
                    raise
                if len(version) != 40:
                    print(f"Version not a hash!: {value}")
                    errors = True
            elif isinstance(value, dict):
                errors = walk_yaml(value) or errors
            elif isinstance(value, list):
                for item in value:
                    errors = walk_yaml(item) or errors
            elif isinstance(value, (str, bool, int, NoneType)):
                pass
            else:
                raise Exception((key, value))

    return errors

errors = False
for dirpath, dirnames, filenames in os.walk(".github"):
    for f in filenames:
        if not (f.endswith("yaml") or f.endswith("yml")):
            continue

        fullpath = os.path.join(dirpath, f)
        data = yaml.safe_load(open(fullpath))
        new_errors = walk_yaml(data)
        if new_errors:
            print(f"Error in {fullpath}")
            errors = True

if errors:
    sys.exit(1)
