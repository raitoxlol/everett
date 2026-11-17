#!/usr/bin/env python3
"""Run held-out offline projection checks against the built example."""
import json
import subprocess
import sys

binary = sys.argv[1]
account = {"issuer": "fixture-issuer", "subject": "fixture-owner"}
fixture = {
    "account": account,
    "snapshot": {
        "clients": [{"id": "plugin", "account": account, "revoked": False},
                    {"id": "device", "account": account, "revoked": False}],
        "sessions": [],
    },
    "native": [{"client_id": "device", "record": {
        "id": "codex-existing", "title": "Same name", "harness": "codex",
        "cwd": "/fixture", "path": "", "started": "", "last_active": 1,
        "running": True,
    }}],
    "registered": [{"client_id": "plugin", "record": {
        "session_id": ident, "name": "Same name", "kind": kind,
        "created_at": 1, "state": "registered", "availability": "unknown",
    }} for ident, kind in [("chat-one", "chat"), ("chat-two", "chat"), ("dot-one", "dot")]],
    "requests": [{"account": account, "from": "chat-one", "to": "codex-existing",
                  "capability": "message", "current": "deny"}],
}


def run(value):
    return subprocess.run([binary], input=json.dumps(value), text=True, capture_output=True)


p = run(fixture)
assert p.returncode == 0, p.stderr
out = json.loads(p.stdout)
assert len(out["discovery"]) == 4
assert len({row["label"] for row in out["discovery"]}) == 4
assert all(row["identity"]["reachability"] == "unknown" for row in out["discovery"])
assert out["comparisons"][0]["current"] == "deny"
assert out["comparisons"][0]["proposed"] == "allow"
assert out["delivery_observations"] == "not_observed"
assert out["shadow_only"] is True
fixture["registered"][0]["record"]["state"] = "revoked"
out = json.loads(run(fixture).stdout)
assert out["comparisons"][0]["proposed"] == "deny"
fixture["registered"][1]["record"]["session_id"] = "codex-existing"
p = run(fixture)
assert p.returncode != 0
assert "ambiguous session ID" in p.stderr
print("3 held-out executable scenarios passed (discovery/policy, revoked sender, ID collision)")
