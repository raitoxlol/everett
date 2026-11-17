#!/usr/bin/env python3
"""Read-only host evidence. Never reads profiles, credentials, env or service definitions."""
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import sys
import time


def rfc3339_seconds(value):
    """Accept RFC3339 fractional seconds on macOS's Python 3.9 as well."""
    match = re.fullmatch(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})", value) if isinstance(value, str) else None
    if not match:
        raise ValueError("invalid_last_success")
    fraction = (match[2] or "").ljust(6, "0")[:6]
    offset = "+00:00" if match[3] == "Z" else match[3]
    if int(offset[1:3]) > 23 or int(offset[4:6]) > 59:
        raise ValueError("invalid_last_success")
    try:
        return datetime.datetime.fromisoformat(match[1] + "." + fraction + offset).timestamp()
    except ValueError:
        raise ValueError("invalid_last_success") from None


def poll_health(snapshot, expected_instance, now=None):
    """Summarize supplied operator health, without inferring RPC completion."""
    result = {"polling_fresh": False, "request_completion": "unconfirmed"}
    if not isinstance(snapshot, dict):
        return dict(result, reason="invalid_snapshot")
    runtime = snapshot.get("runtime")
    components = snapshot.get("components")
    if not isinstance(runtime, dict) or not isinstance(components, dict):
        return dict(result, reason="invalid_snapshot")
    if not isinstance(expected_instance, str) or not re.fullmatch(r"[!-~]{1,128}", expected_instance):
        return dict(result, reason="invalid_expected_instance")
    if runtime.get("instance_id") != expected_instance:
        return dict(result, reason="instance_mismatch")
    if snapshot.get("live") is not True or snapshot.get("ready") is not True:
        return dict(result, reason="not_ready")
    poll = components.get("control-plane")
    if not isinstance(poll, dict) or poll.get("status") != "ok":
        return dict(result, reason="polling_unconfirmed")
    details = poll.get("details")
    if not isinstance(details, dict):
        return dict(result, reason="invalid_poll_details")
    if "last_success" not in details:
        return dict(result, reason="no_successful_poll")
    try:
        age = (time.time() if now is None else now) - rfc3339_seconds(details["last_success"])
    except ValueError:
        return dict(result, reason="invalid_last_success")
    deadline = details.get("deadline_seconds")
    current = details.get("current_poll_age_seconds")
    finite = lambda value: type(value) is int or (type(value) is float and math.isfinite(value))
    if not finite(deadline) or not 0 < deadline <= 120 or not finite(current) or current < 0:
        return dict(result, reason="invalid_poll_deadline")
    result["poll_success_age_seconds"] = age
    if not -5 <= age <= 75:
        return dict(result, reason="stale_successful_poll")
    if current > deadline + 5:
        return dict(result, reason="poll_deadline_exceeded")
    return dict(result, polling_fresh=True, reason="fresh_polling")


def poll_health_stdin(expected_instance):
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536:
            raise ValueError("snapshot_too_large")
        result = poll_health(json.loads(raw.decode("utf-8")), expected_instance)
    except (ValueError, UnicodeError):
        result = {"polling_fresh": False, "request_completion": "unconfirmed", "reason": "invalid_snapshot_input"}
    print(json.dumps(result))
    return 0 if result["polling_fresh"] else 1


def capture(argv):
    try:
        out = subprocess.run(argv, capture_output=True, text=True, timeout=8)
        return out.stdout if out.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def artifact(path):
    result = {"path": str(path)}
    try:
        with open(path, "rb") as stream:
            digest = hashlib.sha256()
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        result["sha256"] = digest.hexdigest()
    except OSError:
        result["fingerprint"] = "unavailable"
    if Path(path).name.startswith("everett"):
        version = capture([str(path), "--version"]) or ""
        match = re.search(r"\beverett (\d+\.\d+\.\d+(?:[-+][\w.]+)?)", version)
        if match:
            result["version"] = match[1]
        revision = re.search(r"^revision: ([a-f0-9]{40})$", version, re.M)
        result["revision"] = revision[1] if revision else "unconfirmed"
        dirty = re.search(r"^dirty: (true|false)$", version, re.M)
        if dirty:
            result["dirty"] = dirty[1] == "true"
    return result


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--poll-health":
        return poll_health_stdin(sys.argv[2])
    if len(sys.argv) > 1:
        print("Usage: python3 scripts/diagnose-tunnel-host.py [--poll-health EXPECTED_INSTANCE < snapshot.json]\nRead-only JSON evidence; no credentials, profiles, service definitions, logs or environment are read.")
        return
    result = {"checked_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "host": socket.gethostname(), "platform": sys.platform,
              "processes": [], "services": [], "installed": [],
              "forwarding_health": "unconfirmed", "production_revision": "unconfirmed"}
    processes = capture(["ps", "-axo", "pid=,ppid=,comm="])
    if processes is None:
        result["process_inventory"] = "unavailable"
    for line in (processes or "").splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) != 3 or not parts[0].isdigit():
            continue
        pid, parent, command = parts
        name = Path(command).name
        if name not in {"tunnel-client", "cloudflared"} and not re.fullmatch(r"everett(?:-[A-Za-z0-9._-]+)?", name):
            continue
        # Inspect only known subcommand words and --config paths. Never emit argv.
        args = capture(["ps", "-p", pid, "-o", "args="])
        try:
            words = shlex.split(args or "")
        except ValueError:
            words = []
        roles = [word for word in words[1:] if word in {"gateway", "mcp", "relay", "tunnel", "watch", "run"}]
        exe = command
        if sys.platform.startswith("linux"):
            try:
                exe = os.readlink(f"/proc/{pid}/exe")
            except OSError:
                pass
        entry = {"pid": int(pid), "parent_pid": int(parent), "roles": roles, "executable": artifact(exe)}
        if "--config" in words:
            index = words.index("--config") + 1
            if index < len(words):
                # Report only its presence. The configuration may hold secrets.
                entry["explicit_config"] = True
        result["processes"].append(entry)
    if sys.platform == "darwin":
        services = capture(["launchctl", "list"])
        result["service_inventory"] = "available" if services is not None else "unavailable"
        for line in (services or "").splitlines():
            fields = line.split()
            if len(fields) == 3 and re.search(r"everett|tunnel", fields[2], re.I):
                result["services"].append({"pid": fields[0], "last_exit": fields[1], "label": fields[2]})
    elif shutil.which("systemctl"):
        for pattern in ["everett*", "*tunnel*"]:
            services = capture(["systemctl", "--user", "list-units", "--all", "--plain", "--no-legend", "--no-pager", pattern])
            for line in (services or "").splitlines():
                fields = line.split()
                if len(fields) >= 4:
                    result["services"].append(dict(zip(["unit", "load", "active", "sub"], fields[:4])))
    for name in ["everett", "tunnel-client"]:
        path = shutil.which(name)
        if path:
            result["installed"].append(artifact(Path(path).resolve()))
    result["note"] = "A PID, installed binary, or startup readiness does not prove forwarding. Compare the serving process artifact, then probe the existing authenticated client connection. No service or configuration was changed."
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    sys.exit(main())
