#!/usr/bin/env python3
"""Isolated native CLI journey against the backend's synthetic local Worker fixture."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time
from urllib.parse import urlparse
from urllib.request import urlopen


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--node", type=Path, required=True)
    parser.add_argument("--issuer", required=True)
    args = parser.parse_args()
    issuer = urlparse(args.issuer)
    assert issuer.scheme == "http" and issuer.hostname == "127.0.0.1" and issuer.port
    assert issuer.path in ("", "/") and not issuer.query and not issuer.fragment and not issuer.username and not issuer.password
    origin = args.issuer.rstrip("/")
    with urlopen(origin + "/health", timeout=5) as response:
        assert json.load(response) == {"backend": "cloudflare", "status": "configuration-present"}
    root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="everett-native-worker-") as temp:
        homes = [Path(temp) / name for name in ("a", "b")]
        processes = []
        logs = []
        try:
            for home in homes:
                (home / ".everett").mkdir(parents=True)
                (home / ".everett/config.toml").write_text("[auth]\nprovider = 'cloudflare'\nissuer = '" + origin + "'\n")
                (home / ".everett/auth.json").write_text("synthetic-old-oidc-preserve")
                transcript = home / ".codex/sessions/2026/10/21/x-1.jsonl"
                transcript.parent.mkdir(parents=True)
                transcript.write_bytes((root / "everett-rs/tests/fixtures/codex.jsonl").read_bytes())
                (home / "bin").mkdir()
                opener = home / "bin" / ("open" if os.uname().sysname == "Darwin" else "xdg-open")
                opener.write_text("#!/bin/sh\nexec " + shlex.quote(str(args.node)) + " " + shlex.quote(str(args.fixture)) + ' approve "$1"\n')
                opener.chmod(0o700)

            def env(home):
                return {"HOME": str(home), "EVERETT_HOME": str(home), "PATH": str(home / "bin") + ":/usr/bin:/bin", "EVERETT_NOTIFY": "none", "CODEX_THREAD_ID": "x-1"}

            def run(home, *command, success=True, json_output=True):
                result = subprocess.run([str(args.binary), *command], env=env(home), capture_output=True, timeout=30)
                assert (result.returncode == 0) == success, "CLI command failed: " + command[0] + ": " + result.stderr.decode()
                return json.loads(result.stdout) if success and json_output else result

            def until(check, label):
                deadline = time.monotonic() + 25
                while time.monotonic() < deadline:
                    result = check()
                    if result:
                        return result
                    time.sleep(0.25)
                raise AssertionError("Timed out: " + label)

            a, b = homes
            identities = [run(home, "login", "--open", "--device", "synthetic computer " + home.name, "--json") for home in homes]
            assert identities[0]["owner"] == identities[1]["owner"]
            assert identities[0]["agent_id"] != identities[1]["agent_id"]
            bundles = [run(home, "relay", "register") for home in homes]
            assert len(run(a, "relay", "devices")) >= 2
            run(a, "peer", "add-relay", "remote", "--device", identities[1]["agent_id"], "--fingerprint", "0" * 64, success=False)
            for home, remote in [(a, bundles[1]), (b, bundles[0])]:
                run(home, "peer", "add-relay", "remote", "--device", remote["device"]["deviceId"], "--fingerprint", remote["fingerprint"])
            text = "synthetic encrypted native request 日本語"
            sent = run(a, "send", text, "--to", "remote/codex/x-1", "--mode", "inbox", "--idempotency-key", "native-offline", "--json")
            duplicate = run(a, "send", text, "--to", "remote/codex/x-1", "--mode", "inbox", "--idempotency-key", "native-offline", "--json")
            assert sent["message_id"] == duplicate["message_id"]
            assert run(a, "peer", "status", "remote", sent["message_id"])["state"] == "queued"
            for record in (a / ".everett/relay-cloudflare/outgoing").glob("*.json"):
                assert text not in record.read_text()
            print("PASS separate approvals, server identities, signed register/pins, offline encrypted queue and idempotent retry", flush=True)
            for home in homes:
                log = open(home / "listener.log", "wb")
                logs.append(log)
                processes.append(subprocess.Popen([str(args.binary), "relay"], env=env(home), stdout=subprocess.DEVNULL, stderr=log))
            received = until(lambda: run(b, "inbox", "--peek", "--json")["messages"], "recipient durable inbox")
            assert len(received) == 1 and received[0]["text"] == text
            assert received[0]["hops"] == 1
            run(b, "inbox", "--json")
            until(lambda: run(a, "peer", "status", "remote", sent["message_id"])["state"] == "claimed", "signed claimed observation")
            run(b, "reply", received[0]["id"], "synthetic encrypted native reply", json_output=False)
            replies = until(lambda: run(a, "inbox", "--peek", "--json")["messages"], "sender reply inbox")
            assert len(replies) == 1 and replies[0]["text"] == "synthetic encrypted native reply"
            assert replies[0]["hops"] == 2
            assert run(a, "peer", "status", "remote", sent["message_id"])["state"] == "replied"
            assert len(run(b, "inbox", "--peek", "--json")["messages"]) == 0
            print("PASS native listeners, durable read, signed claim, encrypted parent reply and replied status", flush=True)
            revoke = subprocess.run([str(args.node), str(args.fixture), "revoke", origin + "/", identities[1]["agent_id"]], capture_output=True, timeout=15)
            assert revoke.returncode == 0, "synthetic owner revocation failed"
            run(b, "whoami", "--json", success=False)
            until(lambda: processes[1].poll() is not None, "revoked listener exit")
            assert processes[1].returncode != 0
            run(a, "whoami", "--json")
            run(a, "send", "after revoke", "--to", "remote/codex/x-1", "--mode", "inbox", "--idempotency-key", "native-revoked", "--json", success=False)
            for home in homes:
                assert (home / ".everett/auth.json").read_text() == "synthetic-old-oidc-preserve"
                assert not (home / ".everett/relay").exists()
                assert (home / ".everett/cloudflare-auth.json").stat().st_mode & 0o777 == 0o600
            print("PASS independent owner revocation, revoked listener stop, other credential active and old OIDC untouched", flush=True)
            print("Synthetic local Worker/native CLI acceptance passed. Real GitHub, public network and physical-device acceptance remain separate.", flush=True)
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            for log in logs:
                log.close()


if __name__ == "__main__":
    main()
