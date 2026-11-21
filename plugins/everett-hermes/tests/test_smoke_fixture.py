import json
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import smoke


class SmokeFixtureTests(unittest.TestCase):
    def test_signed_out_requires_the_account_gate_error(self):
        message = smoke.ACCOUNT_ERROR + " Run Everett login in this profile, then retry."
        self.assertIn(smoke.ACCOUNT_ERROR, smoke._require_signed_out_account_error(json.dumps({"error": message})))
        with self.assertRaises(smoke.SmokeFailure):
            smoke._require_signed_out_account_error(json.dumps({"error": "other failure"}))

    def test_whoami_requires_verified_account_without_fabricating_client_identity(self):
        response = {
            "identity": {
                "account": {"status": "verified"},
                "client": {"status": "unbound"},
            },
        }
        _, identity = smoke._require_verified_whoami(json.dumps(response))
        self.assertEqual(identity["client"]["status"], "unbound")

        for invalid in (
            {"identity": {"account": {"status": "unverified"}}},
            {**response, "error": "verification failed"},
            {**response, "isError": True},
            {**response, "verified": False},
        ):
            with self.subTest(invalid=invalid), self.assertRaises(smoke.SmokeFailure):
                smoke._require_verified_whoami(json.dumps(invalid))

    def test_session_result_requires_the_state_db_hermes_session(self):
        sessions, hermes_sessions = smoke._require_hermes_sessions(
            json.dumps({"sessions": [{"id": "state-db-session", "harness": "hermes"}]}),
            "state-db-session",
        )
        self.assertEqual(len(sessions), 1)
        self.assertEqual(hermes_sessions[0]["id"], "state-db-session")

        for invalid in (
            {"error": "list failed"},
            {"sessions": []},
            {},
            {"sessions": "not an array"},
            {"sessions": [{"id": "other-session", "harness": "hermes"}]},
            {"sessions": [{"id": "state-db-session", "harness": "codex"}]},
            {"sessions": [{"id": "state-db-session", "harness": "hermes"}], "isError": True},
        ):
            with self.subTest(invalid=invalid), self.assertRaises(smoke.SmokeFailure):
                smoke._require_hermes_sessions(json.dumps(invalid), "state-db-session")

    def test_session_list_includes_automated_hermes_sessions(self):
        messages = [
            {"role": "assistant", "tool_calls": [{"function": {"name": smoke.TOOL_SEARCH, "arguments": {}}}]},
            {"role": "assistant", "tool_calls": [{
                "function": {"name": smoke.TOOL_DESCRIBE, "arguments": {"names": [smoke.WHOAMI, smoke.LIST]}}
            }]},
            {"role": "assistant", "tool_calls": [{
                "function": {"name": smoke.TOOL_CALL, "arguments": {"calls": [{"name": smoke.WHOAMI}]}}
            }]},
        ]
        name, arguments = smoke._next_call(smoke.SIGNED_IN, messages)
        self.assertEqual(name, smoke.TOOL_CALL)
        self.assertEqual(arguments["calls"][0]["arguments"], {"harness": "hermes", "all": True})

    def test_direct_native_tools_bypass_tool_search_when_exposed_directly(self):
        available = {smoke.WHOAMI, smoke.LIST}
        self.assertEqual(smoke._next_call(smoke.SIGNED_IN, [], available=available), (smoke.WHOAMI, {}))
        after_whoami = [{"role": "assistant", "tool_calls": [{"function": {"name": smoke.WHOAMI, "arguments": "{}"}}]}]
        self.assertEqual(
            smoke._next_call(smoke.SIGNED_IN, after_whoami, available=available),
            (smoke.LIST, {"harness": "hermes", "all": True}),
        )

    def test_auxiliary_model_request_has_no_smoke_marker(self):
        prompt = smoke.SIGNED_OUT + ": check identity"
        chat = {"messages": [{"role": "user", "content": prompt}]}
        self.assertIsNone(smoke._marker(chat))
        self.assertEqual(smoke._marker({**chat, "tools": [{"function": {"name": smoke.TOOL_SEARCH}}]}), smoke.SIGNED_OUT)
        self.assertIsNone(smoke._marker({"messages": [{"role": "user", "content": "Create a title for: " + prompt}]}))

    def test_cleanup_stops_servers_when_hermes_teardown_times_out(self):
        events = []
        server_ids = (object(), object())

        def run(_hermes, args, _env, _home, **options):
            events.append(("command", args, options["timeout"]))
            raise subprocess.TimeoutExpired(args, options["timeout"])

        def stop(server, _thread):
            events.append(("server", server))

        servers = ((server_ids[0], object()), (server_ids[1], object()))
        with patch.object(smoke, "_run", side_effect=run):
            with patch.object(smoke, "_stop_server", side_effect=stop):
                with self.assertRaises(subprocess.TimeoutExpired):
                    smoke._cleanup_profile(
                        Path("hermes"), {}, Path("home"), True, servers)

        self.assertEqual(events, [
            ("command", ["gateway", "stop"], 30),
            ("command", ["plugins", "remove", "everett-hermes"], 60),
            ("server", server_ids[0]),
            ("server", server_ids[1]),
        ])

    def test_loopback_issuer_accepts_only_synthetic_fixture_identity(self):
        server, thread = smoke._start_server(smoke.IssuerHandler)
        self.addCleanup(smoke._stop_server, server, thread)
        endpoint = f"http://127.0.0.1:{server.server_port}/v2/relay/whoami"

        request = Request(endpoint, data=b"{}", headers={"Authorization": f"Bearer {smoke.FIXTURE_TOKEN}"})
        with urlopen(request, timeout=3) as response:
            identity = json.load(response)
        self.assertEqual(identity["result"]["owner"], "fixture-owner")

        rejected = Request(endpoint, data=b"{}", headers={"Authorization": "Bearer not-a-fixture-token"})
        with self.assertRaises(HTTPError) as error:
            urlopen(rejected, timeout=3)
        self.assertEqual(error.exception.code, 401)


if __name__ == "__main__":
    unittest.main()
