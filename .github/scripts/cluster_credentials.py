#!/usr/bin/env python3
"""kubectl exec credential plugin using short-lived GitHub OIDC, never key files."""

import json
import os
import re
import sys
from urllib.parse import urlencode, urlsplit
from urllib.request import Request, urlopen


def request_json(url, headers, body=None):
    request = Request(url, headers=headers, data=None if body is None else json.dumps(body).encode())
    with urlopen(request, timeout=30) as response:
        return json.load(response)


def credential(environ):
    provider = environ["NL_GCP_PROVIDER"]
    account = environ["NL_GCP_SERVICE_ACCOUNT"]
    if not re.fullmatch(r"projects/\d+/locations/global/workloadIdentityPools/[\w-]+/providers/[\w-]+", provider):
        raise ValueError("Invalid identity provider")
    if not re.fullmatch(r"[a-z][a-z0-9-]+@[a-z0-9-]+\.iam\.gserviceaccount\.com", account):
        raise ValueError("Invalid service account")
    url = environ["ACTIONS_ID_TOKEN_REQUEST_URL"]
    parsed = urlsplit(url)
    if parsed.scheme != "https" or not (parsed.hostname or "").endswith(".actions.githubusercontent.com"):
        raise ValueError("Invalid GitHub identity endpoint")
    url += ("&" if parsed.query else "?") + urlencode({"audience": "https://github.com/TraceMachina/nativelink"})
    identity = request_json(url, {"Authorization": "Bearer " + environ["ACTIONS_ID_TOKEN_REQUEST_TOKEN"]})["value"]
    token = request_json("https://sts.googleapis.com/v1/token", {"Content-Type": "application/json"}, {
        "audience": "//iam.googleapis.com/" + provider,
        "grantType": "urn:ietf:params:oauth:grant-type:token-exchange",
        "requestedTokenType": "urn:ietf:params:oauth:token-type:access_token",
        "subjectTokenType": "urn:ietf:params:oauth:token-type:jwt",
        "subjectToken": identity,
        "scope": "https://www.googleapis.com/auth/cloud-platform"})["access_token"]
    result = request_json(f"https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/{account}:generateAccessToken",
                          {"Authorization": "Bearer " + token, "Content-Type": "application/json"},
                          # GKE resolves the RBAC username from the email scope:
                          # https://cloud.google.com/kubernetes-engine/docs/how-to/api-server-authentication
                          {"scope": ["https://www.googleapis.com/auth/cloud-platform",
                                     "https://www.googleapis.com/auth/userinfo.email"], "lifetime": "3600s"})
    return {"apiVersion": "client.authentication.k8s.io/v1", "kind": "ExecCredential",
            "status": {"token": result["accessToken"], "expirationTimestamp": result["expireTime"]}}


if __name__ == "__main__":
    try:
        json.dump(credential(os.environ), sys.stdout)
    except Exception:
        # Authentication responses can contain tokens: never print them.
        sys.exit("Short-lived cluster authentication failed")
