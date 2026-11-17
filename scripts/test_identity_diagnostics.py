#!/usr/bin/env python3
"""Held-out CLI checks using disposable profiles and no network service."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

BINARY = os.environ["EVERETT_VERIFY_BINARY"]

class IdentityDiagnostics(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        (self.home / ".everett").mkdir()
        self.env = {"HOME": str(self.home), "EVERETT_HOME": str(self.home), "PATH": "/nonexistent",
                    "EVERETT_SESSION_ID": "claimed-private-session", "EVERETT_HARNESS_NAME": "codex"}

    def run_cli(self, args=(), body=""):
        before = {p.relative_to(self.home): p.read_bytes() for p in self.home.rglob("*") if p.is_file()}
        result = subprocess.run([BINARY, "identity", "diagnose", *args], input=body, capture_output=True,
                                text=True, env=self.env, timeout=15)
        after = {p.relative_to(self.home): p.read_bytes() for p in self.home.rglob("*") if p.is_file()}
        self.assertEqual(before, after)
        for secret in ["claimed-private-session", "secret-credential-content", "private-expired-token", "forged-session"]:
            self.assertNotIn(secret, result.stdout + result.stderr)
        return result

    def test_unreadable_native_credential_has_safe_human_output(self):
        (self.home / ".everett/cloudflare-auth.json").write_text("secret-credential-content")
        result = self.run_cli()
        self.assertEqual(result.returncode, 2)
        self.assertIn("native_credential_unreadable", result.stdout)
        self.assertIn("Identity mapping:", result.stdout)

    def test_expired_native_credential_cannot_map_claimed_session(self):
        (self.home / ".everett/cloudflare-auth.json").write_text(json.dumps({
            "issuer": "http://127.0.0.1:1", "agent_id": "device", "owner": "owner",
            "device_name": "device", "token": "private-expired-token", "expires_at": 0}))
        result = self.run_cli(["--json"])
        report = json.loads(result.stdout)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(report["reason"], "native_credential_unverified")
        self.assertIsNone(report["session_id"])

    def test_fixed_legacy_grants_remain_unmapped(self):
        config = self.home / "binding.json"
        config.write_text(json.dumps({"agent_id": "legacy", "harness": "openai-dot", "cwd": str(self.home),
                                     "project": "fixture", "destinations": ["native"], "allow_shared_core": True}))
        result = self.run_cli(["--json", "--gateway-config", str(config), "--credentials-stdin"],
                              json.dumps({"access_token": "unused", "identity_handle": "unused"}))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout)["reason"], "legacy_binding_unmapped")

    def test_session_claim_is_not_an_identity_handle(self):
        result = self.run_cli(["--json", "--gateway-config", str(self.home / "missing"), "--credentials-stdin"],
                              json.dumps({"access_token": "unused", "identity_handle": "unused", "session_id": "forged-session"}))
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout)["reason"], "invalid_input")

if __name__ == "__main__":
    unittest.main()
