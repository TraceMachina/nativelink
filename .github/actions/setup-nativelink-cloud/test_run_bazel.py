import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

import run_bazel


class AttributionTest(unittest.TestCase):
    def test_each_invocation_gets_its_own_ticket_before_program_arguments(self):
        with tempfile.TemporaryDirectory() as temp:
            rc = Path(temp, "user.bazelrc")
            rc.write_text("common --bes_backend=grpcs://example.invalid\n")
            with patch.object(run_bazel, "build_keyword", side_effect=["nl_ticket:first", "nl_ticket:second"]):
                first = run_bazel.attributed_args(["run", "//:tester", "--", "--mode", "sequential"], rc)
                second = run_bazel.attributed_args(["test", "//..."], rc)
            self.assertEqual(first, ["run", "--bes_keywords=nl_ticket:first", "//:tester", "--", "--mode", "sequential"])
            self.assertEqual(second, ["test", "--bes_keywords=nl_ticket:second", "//..."])

    def test_fork_cache_only_does_not_request_identity(self):
        with tempfile.TemporaryDirectory() as temp:
            rc = Path(temp, "user.bazelrc")
            rc.write_text("common --remote_cache=grpcs://example.invalid\n")
            with patch.object(run_bazel, "build_keyword") as request:
                self.assertEqual(run_bazel.attributed_args(["test", "//..."], rc), ["test", "//..."])
                request.assert_not_called()

    def test_failure_preserves_build_and_hides_exception(self):
        with tempfile.TemporaryDirectory() as temp:
            rc = Path(temp, "user.bazelrc")
            rc.write_text("common --bes_backend=grpcs://example.invalid\n")
            output = io.StringIO()
            with patch.object(run_bazel, "build_keyword", side_effect=ValueError("secret")), contextlib.redirect_stderr(output):
                self.assertEqual(run_bazel.attributed_args(["build", "//..."], rc), ["build", "//..."])
            self.assertNotIn("secret", output.getvalue())

    def test_oidc_audience_exchange_and_response_validation(self):
        env = {"NATIVELINK_GITHUB_APP_URL": "https://app.example", "ACTIONS_ID_TOKEN_REQUEST_URL": "https://oidc.example/token?api-version=1", "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-token"}
        ticket = "t" * 43
        output = io.StringIO()
        with patch.dict(os.environ, env), patch.object(run_bazel, "read_json", side_effect=[{"value": "signed-token"}, {"ticketId": ticket, "keyword": "nl_ticket:" + ticket}]) as fetch, contextlib.redirect_stderr(output):
            self.assertEqual(run_bazel.build_keyword(), "nl_ticket:" + ticket)
        oidc, exchange = [call.args[0] for call in fetch.call_args_list]
        self.assertEqual(parse_qs(urlsplit(oidc.full_url).query)["audience"], ["nativelink.com"])
        self.assertEqual(oidc.get_header("Authorization"), "Bearer request-token")
        self.assertEqual(json.loads(exchange.data), {"token": "signed-token"})
        self.assertEqual(exchange.full_url, "https://app.example/v1/ci/token-exchange")
        self.assertEqual(output.getvalue(), "::add-mask::" + ticket + "\n")
        with patch.dict(os.environ, env), patch.object(run_bazel, "read_json", side_effect=[{"value": "signed-token"}, {"ticketId": ticket, "keyword": "nl_ticket:" + ticket + "\n--remote_cache=evil"}]):
            with self.assertRaises(ValueError):
                run_bazel.build_keyword()


if __name__ == "__main__":
    unittest.main()
