#!/usr/bin/env python3
"""Attach a fresh verified GitHub build ticket to each CI Bazel invocation."""

import json
import os
from pathlib import Path
import re
import sys
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
from urllib.request import Request, urlopen


def read_json(request):
    with urlopen(request, timeout=12) as response:
        return json.load(response)


def build_keyword():
    origin = os.environ.get("NATIVELINK_GITHUB_APP_URL", "")
    oidc_url = os.environ.get("ACTIONS_ID_TOKEN_REQUEST_URL", "")
    bearer = os.environ.get("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "")
    if not origin or not oidc_url or not bearer:
        raise ValueError("GitHub OIDC configuration is missing")
    app = urlsplit(origin)
    if app.scheme != "https" or not app.netloc or app.query or app.fragment:
        raise ValueError("invalid GitHub App origin")
    oidc = urlsplit(oidc_url)
    query = [(k, v) for k, v in parse_qsl(oidc.query) if k != "audience"]
    query.append(("audience", "nativelink.com"))
    request_url = urlunsplit(oidc._replace(query=urlencode(query)))
    token = read_json(Request(request_url, headers={"Authorization": "Bearer " + bearer}))["value"]
    ticket = read_json(Request(
        origin.rstrip("/") + "/v1/ci/token-exchange",
        data=json.dumps({"token": token}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    ))
    ticket_id = ticket.get("ticketId", "")
    if not isinstance(ticket_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]{43}", ticket_id):
        raise ValueError("invalid build ticket")
    if ticket.get("keyword") != "nl_ticket:" + ticket_id:
        raise ValueError("invalid build keyword")
    # GitHub processes workflow commands on stderr too. Keep the ticket out of
    # public Bazel command-line and failure output for the rest of the job.
    print("::add-mask::" + ticket_id, file=sys.stderr, flush=True)
    return ticket["keyword"]


def attributed_args(args, bazelrc=Path("user.bazelrc")):
    if not args or args[0] not in ("build", "test", "run"):
        return args
    # Fork jobs intentionally have cache reads only, with no BES credential.
    # Read the configuration without logging any endpoint or credential.
    if not bazelrc.exists() or "--bes_backend=" not in bazelrc.read_text():
        return args
    try:
        keyword = build_keyword()
    except Exception:
        # Never echo HTTP exceptions: they can contain credential-bearing URLs.
        print("::warning::NativeLink GitHub attribution unavailable; check App permissions, repository enablement, and OIDC setup.", file=sys.stderr)
        return args
    # Insert before all Bazel/program arguments, including `run ... -- ...`.
    # A ticket binds to one invocation, so obtain it here, not once per job.
    return [args[0], "--bes_keywords=" + keyword, *args[1:]]


if __name__ == "__main__":
    os.execvp("bazel", ["bazel", *attributed_args(sys.argv[1:])])
