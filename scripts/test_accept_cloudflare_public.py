import contextlib
import io
import json
from pathlib import Path
import shlex
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import accept_cloudflare_public as acceptance


class PublicAcceptanceTests(unittest.TestCase):
    def test_run_uses_unique_local_ids_and_peer_alias(self):
        run = acceptance.make_run("012345abcdef")
        self.assertEqual(run.peer_name, "public-012345abcdef")
        self.assertEqual(run.inbox_session, "public012345abcdef")
        self.assertIn(run.run_id, run.idempotency_key)
        self.assertIn(run.run_id, run.message)

    def test_invalid_run_id_is_rejected(self):
        for value in ("ABCDEF012345", "short", "0123456789ab;echo"):
            with self.subTest(value=value), self.assertRaises(acceptance.AcceptanceError):
                acceptance.make_run(value)

    def test_identity_accepts_only_verified_public_cloudflare_identity(self):
        value = {
            "signed_in": True,
            "verified": True,
            "provider": "cloudflare",
            "issuer": "https://everett.raitox.tech/",
            "owner": "owner-public-id",
            "agent_id": "device-public-id",
        }
        identity = acceptance.parse_identity(value)
        self.assertEqual(identity.issuer, acceptance.PUBLIC_ISSUER)
        self.assertFalse(hasattr(identity, "token"))

        for key, wrong in (("issuer", "https://example.test"), ("provider", "oidc"), ("verified", False)):
            invalid = dict(value, **{key: wrong})
            with self.subTest(key=key), self.assertRaises(acceptance.AcceptanceError):
                acceptance.parse_identity(invalid)

    def test_identity_rejects_untrusted_or_incomplete_metadata(self):
        valid = {
            "signed_in": True,
            "verified": True,
            "provider": "cloudflare",
            "issuer": acceptance.PUBLIC_ISSUER,
            "owner": "owner-public-id",
            "agent_id": "device-public-id",
        }
        for invalid in (
            None,
            [],
            dict(valid, signed_in=1),
            dict(valid, owner=""),
            dict(valid, agent_id=None),
            dict(valid, issuer="https://everett.raitox.tech.attacker.test"),
        ):
            with self.subTest(invalid=invalid), self.assertRaises(acceptance.AcceptanceError):
                acceptance.parse_identity(invalid)

    def test_bundle_requires_public_device_fields_and_lowercase_fingerprint(self):
        valid = {
            "device": {"deviceId": "device-public-id", "owner": "owner-public-id"},
            "fingerprint": "a" * 64,
        }
        bundle = acceptance.parse_bundle(valid)
        self.assertEqual(bundle.device_id, "device-public-id")
        self.assertEqual(bundle.owner, "owner-public-id")
        self.assertEqual(bundle.fingerprint, "a" * 64)
        self.assertFalse(hasattr(bundle, "private_key"))

        for invalid in (
            None,
            {"device": []},
            {"device": {"deviceId": "", "owner": "owner-public-id"}, "fingerprint": "a" * 64},
            {"device": {"deviceId": "d", "owner": ""}, "fingerprint": "a" * 64},
            {"device": {"deviceId": "d", "owner": "o"}, "fingerprint": "A" * 64},
            {"device": {"deviceId": "d", "owner": "o"}, "fingerprint": "a" * 63},
        ):
            with self.subTest(invalid=invalid), self.assertRaises(acceptance.AcceptanceError):
                acceptance.parse_bundle(invalid)

    def test_ssh_command_quotes_remote_arguments_and_rejects_host_options(self):
        arguments = ["reply", "message id", "reply; touch /tmp/nope", "$(id)", "'quoted'"]
        command = acceptance.ssh_command("nucbox", "everett", arguments)
        self.assertEqual(command[:3], ["ssh", "-T", "nucbox"])
        self.assertEqual(shlex.split(command[3]), ["everett", *arguments])
        with self.assertRaises(acceptance.AcceptanceError):
            acceptance.ssh_command("-oProxyCommand=bad", "everett", ["whoami"])
        for host in ("bad host", "nucbox;touch /tmp/nope", "-F/etc/ssh/config"):
            with self.subTest(host=host), self.assertRaises(acceptance.AcceptanceError):
                acceptance.ssh_command(host, "everett", ["whoami"])
        for binary in ("everett --version", "everett\nwhoami", ""):
            with self.subTest(binary=binary), self.assertRaises(acceptance.AcceptanceError):
                acceptance.ssh_command("nucbox", binary, ["whoami"])

    def test_command_json_does_not_expose_command_output_or_stderr_on_failure(self):
        sentinel = "PRIVATE_TOKEN_SENTINEL_MUST_NOT_LEAK"
        mocked = unittest.mock.Mock(returncode=23, stdout=sentinel.encode())
        with patch.object(acceptance.subprocess, "run", return_value=mocked) as run:
            with self.assertRaises(acceptance.AcceptanceError) as caught:
                acceptance.command_json(["everett", "whoami", sentinel])
        self.assertNotIn(sentinel, str(caught.exception))
        self.assertIn("status 23", str(caught.exception))
        self.assertIs(run.call_args.kwargs["stdin"], acceptance.subprocess.DEVNULL)
        self.assertIs(run.call_args.kwargs["stderr"], acceptance.subprocess.DEVNULL)

    def test_command_json_does_not_echo_malformed_output_or_timeout_details(self):
        sentinel = "PRIVATE_RESPONSE_SENTINEL_MUST_NOT_LEAK"
        with patch.object(
            acceptance.subprocess,
            "run",
            return_value=unittest.mock.Mock(returncode=0, stdout=sentinel.encode()),
        ):
            with self.assertRaises(acceptance.AcceptanceError) as caught:
                acceptance.command_json(["everett", "whoami"])
        self.assertNotIn(sentinel, str(caught.exception))
        self.assertIn("did not return JSON", str(caught.exception))

        timeout = acceptance.subprocess.TimeoutExpired(
            cmd=["ssh", "nucbox", sentinel], timeout=1, output=sentinel.encode(), stderr=sentinel.encode()
        )
        with patch.object(acceptance.subprocess, "run", side_effect=timeout):
            with self.assertRaises(acceptance.AcceptanceError) as timed_out:
                acceptance.command_json(["ssh", "nucbox", sentinel])
        self.assertNotIn(sentinel, str(timed_out.exception))
        self.assertIn("remote Everett command", str(timed_out.exception))

    def test_command_json_returns_only_parsed_json(self):
        payload = {"state": "queued", "message_id": "public-id"}
        with patch.object(
            acceptance.subprocess,
            "run",
            return_value=unittest.mock.Mock(returncode=0, stdout=json.dumps(payload).encode()),
        ):
            self.assertEqual(acceptance.command_json(["everett", "peer", "status"]), payload)

    def test_command_success_checks_exit_without_reading_or_printing_output(self):
        sentinel = "PRIVATE_REPLY_TEXT_MUST_NOT_LEAK"
        mocked = unittest.mock.Mock(returncode=0, stdout=sentinel.encode())
        with patch.object(acceptance.subprocess, "run", return_value=mocked) as run:
            self.assertIsNone(acceptance.command_success(["ssh", "nucbox", "everett reply"]))
        self.assertIs(run.call_args.kwargs["stdout"], acceptance.subprocess.DEVNULL)
        self.assertIs(run.call_args.kwargs["stderr"], acceptance.subprocess.DEVNULL)
        self.assertIs(run.call_args.kwargs["stdin"], acceptance.subprocess.DEVNULL)

        failed = unittest.mock.Mock(returncode=4, stdout=sentinel.encode())
        with patch.object(acceptance.subprocess, "run", return_value=failed):
            with self.assertRaises(acceptance.AcceptanceError) as caught:
                acceptance.command_success(["ssh", "nucbox", "everett reply"])
        self.assertNotIn(sentinel, str(caught.exception))
        self.assertIn("status 4", str(caught.exception))

    def test_plan_does_not_call_any_process(self):
        output = io.StringIO()
        with patch.object(sys, "argv", ["accept_cloudflare_public.py", "plan"]), \
                patch.object(acceptance.subprocess, "run", side_effect=AssertionError("process launched")), \
                contextlib.redirect_stdout(output):
            self.assertEqual(acceptance.main(), 0)
        self.assertIn("No network or Everett command", output.getvalue())
        self.assertIn("Device revocation is a separate owner action", output.getvalue())


if __name__ == "__main__":
    unittest.main()
