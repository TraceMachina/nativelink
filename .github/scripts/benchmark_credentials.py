#!/usr/bin/env python3
"""Bazel credential helper: keep the CI key in the process environment only."""

import json
import os
import sys
from urllib.parse import urlsplit


def credentials(uri, endpoints, key):
    target = urlsplit(uri)
    allowed = {(urlsplit(value).hostname, urlsplit(value).port or 443)
               for value in endpoints.split()}
    if target.scheme not in ("https", "grpcs") or (
        target.hostname, target.port or 443
    ) not in allowed:
        return {}
    if not key or any(char in key for char in "\r\n"):
        raise ValueError("A valid CI API key is required")
    return {"headers": {"x-nativelink-api-key": [key]}}


if __name__ == "__main__":
    try:
        if sys.argv[1:] != ["get"]:
            raise ValueError("Expected the credential helper get command")
        request = json.load(sys.stdin)
        response = credentials(request["uri"], os.environ["NL_ENDPOINTS"],
                               os.environ.get("NL_API_KEY", ""))
        json.dump(response, sys.stdout)
    except (ValueError, KeyError, TypeError):
        # Never echo the request, environment, key or exception payload.
        sys.exit("CI credential lookup failed")
