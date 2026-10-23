#!/usr/bin/env python3
"""Run one real Mac-to-NUC Cloudflare receiver-wake acceptance journey."""

from __future__ import annotations

import argparse
import base64
import hashlib
from dataclasses import asdict, dataclass, field
import json
import os
from pathlib import Path
import re
import shlex
import signal
import stat
import subprocess
import sys
import time
import uuid


CALLER_BINDINGS = (
    "EVERETT_SESSION_ID",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_SESSION_ID",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "GROK_SESSION_ID",
    "PI_SESSION_ID",
    "PI_SESSION_FILE",
    "HERMES_SESSION_ID",
    "OMP_SESSION_ID",
)
UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


class AcceptanceError(RuntimeError):
    pass


class AcceptanceInterrupted(AcceptanceError):
    pass


@dataclass
class Listener:
    host: str
    pid: int
    started: float
    process: subprocess.Popen | None = field(default=None, repr=False)


@dataclass
class RunManifest:
    run_id: str
    created_at: float
    phase: str
    mac_profile: str
    nuc_profile: str
    mac_device_id: str
    nuc_device_id: str
    mac_fingerprint: str
    nuc_fingerprint: str
    session_id: str
    cwd: str
    sender: str = "human"
    marker: str = ""
    idempotency_key: str = ""
    message_id: str = ""
    policy_original_b64: str | None = None
    policy_original_mode: int | None = None
    policy_original_identity: str | None = None
    policy_installed_b64: str | None = None
    policy_installed_identity: str | None = None
    policy_owned: bool = False
    listeners: list[dict] = field(default_factory=list)
    results: dict = field(default_factory=dict)
    cleanup: dict = field(default_factory=dict)


MANIFEST_OWNERS: dict[str, tuple[int, int]] = {}


REMOTE_HELPER = r'''import base64, json, os, pathlib, signal, stat, subprocess, sys, time
op = sys.argv[1]
def disabled_policy(value):
    return type(value) is dict and set(value) == {"version", "enabled", "allow"} and type(value.get("version")) is int and value["version"] == 1 and type(value.get("enabled")) is bool and value["enabled"] is False and type(value.get("allow")) is list and value["allow"] == []
if op == "agents":
    binary = pathlib.Path(sys.argv[2])
    cwd = sys.argv[3]
    if not binary.is_absolute() or binary.name != "claude" or not binary.is_file() or not os.access(binary, os.X_OK): raise SystemExit(38)
    p = subprocess.run([binary, "agents", "--json"], cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=12)
    if p.returncode: raise SystemExit(30)
    print(p.stdout)
elif op == "claude-info":
    binary = pathlib.Path(sys.argv[2])
    if not binary.is_absolute() or binary.name != "claude" or not binary.is_file() or not os.access(binary, os.X_OK): raise SystemExit(38)
    print(json.dumps({"path": str(binary), "resolved_path": str(binary.resolve(strict=True)), "version": "not queried"}))
elif op == "listener-path":
    binary = pathlib.Path(sys.argv[2])
    if not binary.is_absolute() or binary.name != "claude": raise SystemExit(38)
    print(str(binary.parent) + os.pathsep + os.environ.get("PATH", ""))
elif op == "profile":
    path = pathlib.Path(sys.argv[2])
    st = path.lstat()
    if str(path.resolve(strict=True)) != str(path) or not stat.S_ISDIR(st.st_mode) or st.st_uid != os.getuid() or stat.S_IMODE(st.st_mode) & 0o077: raise SystemExit(39)
    print("ready")
elif op == "validate-policy-ancestors":
    profile = pathlib.Path(sys.argv[2])
    policy = pathlib.Path(sys.argv[3])
    if str(profile.resolve(strict=True)) != str(profile): raise SystemExit(42)
    if policy != profile / ".everett" / "relay-cloudflare" / "receiver-wake.json": raise SystemExit(42)
    current = profile
    for part in (".everett", "relay-cloudflare"):
        current = current / part
        st = current.lstat()
        if not stat.S_ISDIR(st.st_mode) or st.st_uid != os.getuid() or stat.S_IMODE(st.st_mode) & 0o077: raise SystemExit(42)
    print("ready")
elif op == "canonical-cwd":
    path = pathlib.Path(sys.argv[2])
    if not path.is_dir() or str(path.resolve(strict=True)) != str(path): raise SystemExit(40)
    print("ready")
elif op == "cwd-digest":
    root = pathlib.Path(sys.argv[2])
    if str(root.resolve(strict=True)) != str(root): raise SystemExit(44)
    def git_bytes(*args):
        try: result = subprocess.run(["git", "-C", str(root), *args], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=8)
        except subprocess.TimeoutExpired: raise SystemExit(44)
        if result.returncode: raise SystemExit(44)
        return result.stdout
    digest = __import__("hashlib").sha256()
    for label, data in ((b"status", git_bytes("status", "--porcelain=v1", "-z", "--untracked-files=all")), (b"index", git_bytes("ls-files", "--stage", "-z", "--", ".")), (b"tracked-diff", git_bytes("diff", "--binary", "HEAD", "--", "."))):
        digest.update(label + b"\0" + data + b"\0")
    paths = git_bytes("ls-files", "--others", "--exclude-standard", "-z", "--", ".").split(b"\0")
    deadline = time.monotonic() + 38
    entries = 0
    def add_entry(path):
        global entries
        if time.monotonic() >= deadline: raise SystemExit(44)
        relative = os.fsencode(str(path.relative_to(root)))
        st = path.lstat()
        digest.update(b"tree\0" + relative + b"\0" + str(stat.S_IFMT(st.st_mode)).encode() + b":" + str(stat.S_IMODE(st.st_mode)).encode() + b"\0")
        if stat.S_ISLNK(st.st_mode): digest.update(os.fsencode(os.readlink(path)))
        elif stat.S_ISREG(st.st_mode):
            with path.open("rb") as f:
                for block in iter(lambda: f.read(1024 * 1024), b""):
                    if time.monotonic() >= deadline: raise SystemExit(44)
                    digest.update(block)
        elif not stat.S_ISDIR(st.st_mode): raise SystemExit(44)
        entries += 1
    try:
        def walk_error(_error): raise SystemExit(44)
        for directory, dirnames, filenames in os.walk(root, topdown=True, followlinks=False, onerror=walk_error):
            if time.monotonic() >= deadline: raise SystemExit(44)
            dirnames[:] = sorted(name for name in dirnames if name != ".git")
            filenames = sorted(name for name in filenames if name != ".git")
            current = pathlib.Path(directory)
            for name in dirnames: add_entry(current / name)
            for name in filenames: add_entry(current / name)
    except OSError: raise SystemExit(44)
    print(json.dumps({"sha256": digest.hexdigest(), "untracked_paths": len([path for path in paths if path]), "tree_entries": entries}))
elif op == "pid-alive":
    pid = int(sys.argv[2])
    try: os.kill(pid, 0); print("alive")
    except (ProcessLookupError, PermissionError): raise SystemExit(41)
elif op == "policy":
    path = pathlib.Path(sys.argv[2])
    try:
        st = path.lstat()
        if not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid(): raise SystemExit(31)
        raw = path.read_bytes()
        if st.st_mode & 0o077: raise SystemExit(32)
        value = json.loads(raw)
        if not disabled_policy(value): raise SystemExit(31)
        print(json.dumps({"exists": True, "b64": base64.b64encode(raw).decode(), "mode": stat.S_IMODE(st.st_mode), "identity": f"{st.st_dev}:{st.st_ino}"}))
    except FileNotFoundError:
        print(json.dumps({"exists": False, "b64": None, "mode": None, "identity": None}))
elif op == "install-policy":
    path = pathlib.Path(sys.argv[2]); original_b64 = sys.argv[3]; original_identity = sys.argv[4]; content_b64 = sys.argv[5]
    original = None if original_b64 == "-" else base64.b64decode(original_b64)
    try:
        st = path.lstat()
        if not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid(): raise SystemExit(33)
        current = path.read_bytes()
        identity = f"{st.st_dev}:{st.st_ino}"
    except FileNotFoundError: current = None; identity = "-"
    if current != original or identity != original_identity: raise SystemExit(33)
    if current is not None:
        value = json.loads(current)
        if not disabled_policy(value):
            raise SystemExit(34)
    parent_st = path.parent.lstat()
    if not stat.S_ISDIR(parent_st.st_mode) or parent_st.st_uid != os.getuid() or stat.S_IMODE(parent_st.st_mode) & 0o077: raise SystemExit(42)
    tmp = path.with_name(path.name + ".accept-install")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as f: f.write(base64.b64decode(content_b64)); f.flush(); os.fsync(f.fileno())
    try:
        latest = path.lstat()
        latest_raw = path.read_bytes() if stat.S_ISREG(latest.st_mode) and latest.st_uid == os.getuid() else None
        latest_identity = f"{latest.st_dev}:{latest.st_ino}"
    except FileNotFoundError: latest_raw = None; latest_identity = "-"
    if latest_raw != original or latest_identity != original_identity:
        tmp.unlink(); raise SystemExit(33)
    os.replace(tmp, path)
    st = path.lstat()
    print(json.dumps({"identity": f"{st.st_dev}:{st.st_ino}"}))
elif op == "restore-policy":
    path = pathlib.Path(sys.argv[2]); installed = base64.b64decode(sys.argv[3]); installed_identity = sys.argv[4]; original_b64 = sys.argv[5]; original_mode = int(sys.argv[6])
    try:
        st = path.lstat()
        current = path.read_bytes() if stat.S_ISREG(st.st_mode) and st.st_uid == os.getuid() and stat.S_IMODE(st.st_mode) == 0o600 else None
        identity = f"{st.st_dev}:{st.st_ino}"
    except FileNotFoundError: current = None; identity = "-"
    if current != installed or identity != installed_identity: print("changed"); raise SystemExit(0)
    if original_b64 == "-":
        latest = path.lstat()
        if not stat.S_ISREG(latest.st_mode) or latest.st_uid != os.getuid() or stat.S_IMODE(latest.st_mode) != 0o600 or f"{latest.st_dev}:{latest.st_ino}" != installed_identity or path.read_bytes() != installed:
            print("changed"); raise SystemExit(0)
        path.unlink(); print("removed")
    else:
        raw = base64.b64decode(original_b64)
        tmp = path.with_name(path.name + ".accept-restore")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, original_mode)
        with os.fdopen(fd, "wb") as f: f.write(raw); f.flush(); os.fsync(f.fileno())
        latest = path.lstat()
        if not stat.S_ISREG(latest.st_mode) or latest.st_uid != os.getuid() or stat.S_IMODE(latest.st_mode) != 0o600 or f"{latest.st_dev}:{latest.st_ino}" != installed_identity or path.read_bytes() != installed:
            tmp.unlink(); print("changed"); raise SystemExit(0)
        os.chmod(tmp, original_mode)
        os.replace(tmp, path); print("restored")
elif op == "start":
    binary, logfile, claude_binary = sys.argv[2:5]
    env = os.environ.copy()
    claude_path = pathlib.Path(claude_binary)
    if not claude_path.is_absolute() or claude_path.name != "claude" or not claude_path.is_file() or not os.access(claude_path, os.X_OK): raise SystemExit(38)
    env["PATH"] = str(claude_path.parent) + os.pathsep + env.get("PATH", "")
    log_path = pathlib.Path(logfile)
    log_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    log = os.fdopen(fd, "wb")
    p = subprocess.Popen([binary, "relay"], stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True, env=env, close_fds=True)
    try:
        statline = pathlib.Path(f"/proc/{p.pid}/stat").read_text().split()
        owner_path = pathlib.Path(logfile + ".owner.json")
        owner = json.dumps({"pid": p.pid, "started": time.time(), "start_ticks": statline[21], "profile": env["EVERETT_HOME"]}).encode()
        owner_fd = os.open(owner_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(owner_fd, "wb") as f: f.write(owner); f.flush(); os.fsync(f.fileno())
    except BaseException:
        if p.poll() is None:
            p.terminate()
            try: p.wait(timeout=5)
            except subprocess.TimeoutExpired: p.kill(); p.wait(timeout=2)
        raise
    time.sleep(1.0)
    if p.poll() is not None: print(json.dumps({"pid": p.pid, "started": time.time(), "start_ticks": statline[21], "exited": True})); raise SystemExit(35)
    print(json.dumps({"pid": p.pid, "started": time.time(), "start_ticks": statline[21]}))
elif op == "inspect-pid":
    pid = int(sys.argv[2]); profile = sys.argv[3]
    try:
        raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        env = dict(item.split(b"=", 1) for item in raw if b"=" in item)
        argv = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        statline = pathlib.Path(f"/proc/{pid}/stat").read_text().split()
        okay = env.get(b"EVERETT_HOME", b"").decode() == profile and len(argv) >= 2 and argv[1] == b"relay"
        print(json.dumps({"owned": okay, "start_ticks": statline[21] if len(statline) > 21 else ""}))
    except FileNotFoundError: print(json.dumps({"owned": False, "start_ticks": ""}))
elif op == "stop":
    pid = int(sys.argv[2]); profile = sys.argv[3]; expected = sys.argv[4]
    try:
        raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        env = dict(item.split(b"=", 1) for item in raw if b"=" in item)
        argv = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        statline = pathlib.Path(f"/proc/{pid}/stat").read_text().split()
        owned = env.get(b"EVERETT_HOME", b"").decode() == profile and len(argv) >= 2 and argv[1] == b"relay" and statline[21] == expected
        if not owned: print("changed"); raise SystemExit(0)
        os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                statline = pathlib.Path(f"/proc/{pid}/stat").read_text().split()
                if len(statline) < 3 or statline[2] in ("Z", "X"): break
                os.kill(pid, 0)
            except (FileNotFoundError, ProcessLookupError): break
            time.sleep(.1)
        else: print("still-running"); raise SystemExit(0)
        print("stopped")
    except FileNotFoundError: print("already-stopped")
elif op == "stop-log":
    owner_path = pathlib.Path(sys.argv[2] + ".owner.json")
    st = owner_path.lstat()
    if not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid() or stat.S_IMODE(st.st_mode) != 0o600: raise SystemExit(43)
    owner = json.loads(owner_path.read_text())
    if owner.get("profile") != sys.argv[3]: raise SystemExit(43)
    pid = int(owner["pid"]); profile = owner["profile"]; expected = owner["start_ticks"]
    try:
        raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        env = dict(item.split(b"=", 1) for item in raw if b"=" in item)
        argv = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        statline = pathlib.Path(f"/proc/{pid}/stat").read_text().split()
        owned = env.get(b"EVERETT_HOME", b"").decode() == profile and len(argv) >= 2 and argv[1] == b"relay" and statline[21] == expected
        if not owned: print("changed"); raise SystemExit(0)
        os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                statline = pathlib.Path(f"/proc/{pid}/stat").read_text().split()
                if len(statline) < 3 or statline[2] in ("Z", "X"): break
                os.kill(pid, 0)
            except (FileNotFoundError, ProcessLookupError): break
            time.sleep(.1)
        else: print("still-running"); raise SystemExit(0)
        print("stopped")
    except FileNotFoundError: print("already-stopped")
elif op == "jobs":
    root = pathlib.Path(sys.argv[2]); rows = []
    for path in sorted((root / "receiver-wake" / "jobs").glob("*.json")):
        try:
            job = json.loads(path.read_text())
            imported = job.get("imported", {})
            state = job.get("state", {})
            rows.append({"local_id": imported.get("local_id"), "relay_message_id": imported.get("relay_message_id"), "sender_device_id": imported.get("sender_device_id"), "session_id": imported.get("session_id"), "cwd": imported.get("cwd"), "state": state.get("state"), "target_pid": state.get("target_pid")})
        except (OSError, ValueError): raise SystemExit(36)
    print(json.dumps(rows))
else: raise SystemExit(37)
'''


def save_manifest(path: Path, manifest: RunManifest) -> None:
    data = json.dumps(asdict(manifest), indent=2, sort_keys=True).encode()
    key = str(path)
    try:
        current = path.lstat()
    except FileNotFoundError:
        current = None
    if key not in MANIFEST_OWNERS:
        require(current is None, "manifest path already exists; refusing to replace it")
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        owner = os.fstat(fd)
        require(stat.S_ISREG(owner.st_mode) and owner.st_uid == os.getuid(), "new manifest is not an owner-owned regular file")
        MANIFEST_OWNERS[key] = (owner.st_dev, owner.st_ino)
    else:
        require(current is not None and stat.S_ISREG(current.st_mode) and current.st_uid == os.getuid() and stat.S_IMODE(current.st_mode) == 0o600, "manifest changed type, owner, or permissions")
        require((current.st_dev, current.st_ino) == MANIFEST_OWNERS[key], "manifest path no longer names the file created by this run")
        temp = path.with_name(path.name + f".tmp.{os.getpid()}")
        fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    os.chmod(path if key not in MANIFEST_OWNERS or current is None else temp, 0o600)
    if current is not None:
        latest = path.lstat()
        require((latest.st_dev, latest.st_ino) == MANIFEST_OWNERS[key], "manifest path changed before update")
        os.replace(temp, path)
        updated = path.lstat()
        MANIFEST_OWNERS[key] = (updated.st_dev, updated.st_ino)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AcceptanceError(message)


def json_value(raw: str, label: str):
    try:
        return json.loads(raw)
    except (json.JSONDecodeError, TypeError) as error:
        raise AcceptanceError(f"{label} returned invalid JSON") from error


def exact_policy_rule(session_id: str, cwd: str, sender_device_id: str) -> dict[str, str]:
    return {"session_id": session_id, "cwd": cwd, "sender_device_id": sender_device_id}


def is_disabled_policy(value) -> bool:
    return type(value) is dict and set(value) == {"version", "enabled", "allow"} and type(value.get("version")) is int and value["version"] == 1 and type(value.get("enabled")) is bool and value["enabled"] is False and type(value.get("allow")) is list and value["allow"] == []


def matching_replies(messages, message_id: str, session_id: str, device_id: str, marker: str):
    expected_from = f"{device_id}/claude/{session_id}"
    return [message for message in messages if message.get("reply_to") == message_id and message.get("from") == expected_from and message.get("from_harness") == "claude" and message.get("relay_device") == device_id and message.get("text") == marker]


def committed_send_state(value):
    return value in {"queued", "delivered", "claimed", "replied"}


def send_argv(alias: str, session_id: str, prompt: str, idempotency_key: str) -> list[str]:
    return ["send", prompt, "--to", f"{alias}/claude/{session_id}", "--mode", "inbox", "--wait", "0", "--idempotency-key", idempotency_key, "--json"]


def local_env(profile: str, sender: bool = False) -> dict[str, str]:
    env = os.environ.copy()
    env["EVERETT_HOME"] = profile
    env["EVERETT_AUTH_PROVIDER"] = "cloudflare"
    if sender:
        for name in CALLER_BINDINGS:
            env.pop(name, None)
    return env


class Acceptance:
    def __init__(self, args):
        self.args = args
        self.manifest_path = Path(args.manifest).expanduser().absolute()
        self.manifest: RunManifest | None = None
        self.listeners: list[Listener] = []
        self.policy_path = f"{args.nuc_profile}/.everett/relay-cloudflare/receiver-wake.json"
        self.mac_identity = None
        self.nuc_identity = None
        self.claude_row = None

    def local(self, argv, timeout=20, sender=False, parse_json=True):
        command = [self.args.mac_binary, *map(str, argv)]
        result = subprocess.run(command, env=local_env(self.args.mac_profile, sender), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=timeout)
        require(result.returncode == 0, f"Mac Everett command failed: {argv[0]}")
        return json_value(result.stdout, f"Mac Everett {argv[0]}") if parse_json else result.stdout

    def ssh(self, remote_argv, timeout=25, input_text=None):
        command = ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", self.args.nuc_ssh, shlex.join(list(map(str, remote_argv)))]
        result = subprocess.run(command, input=input_text, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=timeout)
        require(result.returncode == 0, f"NUC remote operation failed: {remote_argv[0]}")
        return result.stdout

    def remote_cli(self, argv, timeout=20, parse_json=True):
        remote = ["env", f"EVERETT_HOME={self.args.nuc_profile}", "EVERETT_AUTH_PROVIDER=cloudflare", self.args.nuc_binary, *map(str, argv)]
        raw = self.ssh(remote, timeout=timeout)
        return json_value(raw, f"NUC Everett {argv[0]}") if parse_json else raw

    def helper(self, op, *values, timeout=20):
        remote = ["env", f"EVERETT_HOME={self.args.nuc_profile}", "EVERETT_AUTH_PROVIDER=cloudflare", "python3", "-c", REMOTE_HELPER, op, *map(str, values)]
        return self.ssh(remote, timeout=timeout)

    def preflight(self):
        a = self.args
        require(Path(a.mac_binary).is_file() and os.access(a.mac_binary, os.X_OK), "Mac Everett binary is unavailable")
        mac_profile = Path(a.mac_profile).lstat()
        require(stat.S_ISDIR(mac_profile.st_mode) and mac_profile.st_uid == os.getuid() and stat.S_IMODE(mac_profile.st_mode) & 0o077 == 0, "Mac Everett profile must be an owner-owned private directory")
        require(self.manifest_path.parent.is_dir(), "manifest parent directory must exist")
        manifest_parent = self.manifest_path.parent.stat()
        require(manifest_parent.st_uid == os.getuid() and stat.S_IMODE(manifest_parent.st_mode) & 0o077 == 0, "manifest parent directory must be owner-only")
        require(a.nuc_profile.startswith("/") and a.nuc_binary.startswith("/"), "NUC profile and binary paths must be absolute")
        require(re.fullmatch(r"[0-9a-f]{64}", a.nuc_sha256) is not None, "expected NUC candidate SHA-256 must be 64 lowercase hexadecimal characters")
        require(re.fullmatch(r"[0-9a-f]{40}", a.source_commit) is not None, "candidate source commit must be a full lowercase Git SHA-1")
        actual_nuc_sha256 = self._remote_sha256(a.nuc_binary)
        require(actual_nuc_sha256 == a.nuc_sha256, "NUC candidate binary SHA-256 does not match the explicitly approved value")
        require(UUID_RE.fullmatch(a.session_id) is not None, "session ID must be a canonical lowercase UUID")
        require(UUID_RE.fullmatch(a.denied_session_id) is not None and a.denied_session_id != a.session_id, "denied session ID must be a different canonical UUID")
        require(Path(a.cwd).is_absolute(), "Claude cwd must be absolute")
        require(bool(a.mac_device_id and a.nuc_device_id and a.mac_fingerprint and a.nuc_fingerprint), "both paired device IDs and fingerprints are required")
        require(a.mac_device_id != a.nuc_device_id and a.mac_fingerprint != a.nuc_fingerprint, "paired relay devices must have distinct IDs and fingerprints")
        require(self.manifest_path.exists() is False, "manifest path already exists")
        self.manifest = RunManifest(str(uuid.uuid4()), time.time(), "preflight", a.mac_profile, a.nuc_profile, a.mac_device_id, a.nuc_device_id, a.mac_fingerprint, a.nuc_fingerprint, a.session_id, a.cwd)
        save_manifest(self.manifest_path, self.manifest)
        self.helper("profile", a.nuc_profile)
        claude_info = json_value(self.helper("claude-info", a.nuc_claude_binary), "NUC Claude executable")
        self.manifest.results["nuc_claude_executable"] = claude_info
        self.helper("validate-policy-ancestors", a.nuc_profile, self.policy_path)
        self.helper("canonical-cwd", a.cwd)
        cwd_baseline = json_value(self.helper("cwd-digest", a.cwd, timeout=70), "NUC project state baseline")
        self.manifest.results["cwd_state_before_sha256"] = cwd_baseline["sha256"]
        self.manifest.results["cwd_untracked_paths_before"] = cwd_baseline["untracked_paths"]

        self.mac_identity = self.local(["relay", "whoami"])
        self.nuc_identity = self.remote_cli(["relay", "whoami"])
        require(self.mac_identity.get("deviceId") == a.mac_device_id, "Mac relay identity does not match the supplied device ID")
        require(self.nuc_identity.get("deviceId") == a.nuc_device_id, "NUC relay identity does not match the supplied device ID")
        require(self.mac_identity.get("owner") == self.nuc_identity.get("owner"), "Mac and NUC relay owners differ")

        mac_peers = self.local(["peer", "list"])
        nuc_peers = self.remote_cli(["peer", "list"])
        self._assert_pin(mac_peers, a.nuc_alias, a.nuc_device_id, a.nuc_fingerprint, "Mac")
        self._assert_pin(nuc_peers, a.mac_alias, a.mac_device_id, a.mac_fingerprint, "NUC")

        self._preflight_claude()
        self.denied_claude_row = self._claude_row(a.denied_session_id, self.agent_rows)
        require(self.denied_claude_row.get("cwd") == a.cwd, "denied Claude session must use the same canonical project cwd")
        policy = json_value(self.helper("policy", self.policy_path), "NUC receiver policy")
        if policy["exists"]:
            existing = json_value(base64.b64decode(policy["b64"]).decode(), "NUC receiver policy")
            require(is_disabled_policy(existing), "NUC policy must use the exact disabled v1 schema with no allow rules or extra fields")
            self.manifest.policy_original_b64 = policy["b64"]
            self.manifest.policy_original_mode = policy["mode"]
            self.manifest.policy_original_identity = policy["identity"]
        self.manifest.results["owner"] = self.mac_identity["owner"]
        self.manifest.results["candidate_source_commit"] = a.source_commit
        self.manifest.results["binary_sha256"] = {"mac": self._sha256(a.mac_binary), "nuc": actual_nuc_sha256, "expected_nuc": a.nuc_sha256}
        self.manifest.results["draft_preservation"] = "not verified; separate native Claude composer evidence is required"
        self.manifest.results["preflight"] = "passed"
        save_manifest(self.manifest_path, self.manifest)

    @staticmethod
    def _assert_pin(peers, alias, expected_id, expected_fingerprint, host):
        pin = peers.get(alias)
        require(isinstance(pin, dict), f"{host} has no pinned relay peer named {alias}")
        require(pin.get("transport") == "cloudflare" and pin.get("device_id") == expected_id and pin.get("fingerprint") == expected_fingerprint, f"{host} relay pin does not match the exact supplied device fingerprint")

    @staticmethod
    def _sha256(path):
        import hashlib
        digest = hashlib.sha256()
        with open(path, "rb") as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        return digest.hexdigest()

    def _remote_sha256(self, path):
        fields = self.ssh(["sha256sum", path], timeout=15).split()
        require(fields and re.fullmatch(r"[0-9a-f]{64}", fields[0]), "could not record NUC candidate binary SHA-256")
        return fields[0]

    def _preflight_claude(self):
        self.agent_rows = self._read_agents()
        self.claude_row = self._claude_row(self.args.session_id, self.agent_rows)

    def _read_agents(self):
        raw = self.helper("agents", self.args.nuc_claude_binary, self.args.cwd)
        agents = json_value(raw, "NUC claude agents --json")
        rows = agents if isinstance(agents, list) else agents.get("agents")
        require(isinstance(rows, list), "claude agents --json did not return an agents list")
        return rows

    def _claude_row(self, session_id, rows):
        matches = [row for row in rows if row.get("sessionId") == session_id]
        require(len(matches) == 1, "Claude must report exactly one matching background session")
        row = matches[0]
        require(row.get("kind") == "background", "target Claude session is not a background session")
        require(row.get("state") == "done" and row.get("status") == "idle" and row.get("waitingFor") in (None, ""), "target Claude session is active or waiting for input")
        require(row.get("cwd") == self.args.cwd, "Claude cwd does not exactly match the requested canonical cwd")
        require(isinstance(row.get("pid"), int) and row["pid"] > 0, "Claude did not report a valid target PID")
        self.helper("pid-alive", row["pid"])
        return row

    def prepare(self):
        a = self.args
        rule = exact_policy_rule(a.session_id, a.cwd, a.mac_device_id)
        content = json.dumps({"version": 1, "enabled": True, "allow": [rule]}, separators=(",", ":")).encode()
        content_b64 = base64.b64encode(content).decode()
        original = self.manifest.policy_original_b64 or "-"
        original_identity = self.manifest.policy_original_identity or "-"
        self.manifest.policy_installed_b64 = content_b64
        self.manifest.policy_owned = True
        self.manifest.phase = "prepare"
        save_manifest(self.manifest_path, self.manifest)
        installed = json_value(self.helper("install-policy", self.policy_path, original, original_identity, content_b64), "NUC receiver policy install")
        self.manifest.policy_installed_identity = installed["identity"]
        save_manifest(self.manifest_path, self.manifest)
        self._start_local_listener()
        remote_log = f"{a.nuc_profile}/.everett/relay-cloudflare/receiver-wake/acceptance-{self.manifest.run_id}.log"
        self.manifest.listeners.append({"host": "nuc", "state": "starting", "log": remote_log})
        save_manifest(self.manifest_path, self.manifest)
        remote = json_value(self.helper("start", a.nuc_binary, remote_log, a.nuc_claude_binary), "NUC relay listener start")
        pid = int(remote["pid"])
        self.listeners.append(Listener("nuc", pid, float(remote["started"])))
        entry = next(item for item in self.manifest.listeners if item.get("host") == "nuc")
        entry.update({"pid": pid, "started": float(remote["started"])})
        save_manifest(self.manifest_path, self.manifest)
        inspect = json_value(self.helper("inspect-pid", pid, a.nuc_profile), "NUC relay listener ownership")
        entry["start_ticks"] = inspect.get("start_ticks", "")
        require(inspect.get("owned"), "NUC relay process ownership could not be proved")
        local_listener = next(listener for listener in self.listeners if listener.host == "mac")
        self.manifest.listeners.append({"host": "mac", "pid": local_listener.pid, "started": local_listener.started, "start_ticks": "", "log": self.local_log})
        self.manifest.phase = "prepared"
        self.manifest.results["policy"] = "exact allow rule installed with mode 0600"
        self.manifest.results["listeners"] = "Mac and NUC owned relay processes started"
        save_manifest(self.manifest_path, self.manifest)

    def _start_local_listener(self):
        env = local_env(self.args.mac_profile)
        self.local_log = str(self.manifest_path.with_name(self.manifest_path.name + ".mac-relay.log"))
        fd = os.open(self.local_log, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
        log = os.fdopen(fd, "wb")
        self.manifest.listeners.append({"host": "mac", "state": "starting", "log": self.local_log})
        save_manifest(self.manifest_path, self.manifest)
        process = subprocess.Popen([self.args.mac_binary, "relay"], env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True, close_fds=True)
        listener = Listener("mac", process.pid, time.time(), process)
        self.listeners.append(listener)
        time.sleep(1)
        self.manifest.listeners[0].update({"pid": process.pid, "started": time.time()})
        save_manifest(self.manifest_path, self.manifest)
        require(process.poll() is None, "Mac Everett relay exited during startup")

    def send(self):
        self._set_phase("send")
        marker = "EVERETT-RECEIVER-WAKE-" + uuid.uuid4().hex.upper()
        key = "receiver-wake-" + uuid.uuid4().hex
        prompt = f"This is a live Everett receiver-wake acceptance request. Reply with the exact marker {marker} and no other text."
        argv = send_argv(self.args.nuc_alias, self.args.session_id, prompt, key)
        first = self.local(argv, timeout=25, sender=True)
        message_id = first.get("message_id")
        require(committed_send_state(first.get("state")) and "wake" not in first and UUID_RE.fullmatch(message_id or "") is not None, "Mac send did not return one committed inbox message without a sender-side wake")
        retry = self.local(argv, timeout=25, sender=True)
        require(retry.get("message_id") == message_id, "same-key retry returned a different message ID")
        self.manifest.marker = marker
        self.manifest.idempotency_key = key
        self.manifest.message_id = message_id
        self.manifest.phase = "sent"
        self.manifest.results["duplicate_retry"] = "same key returned the original message ID"
        save_manifest(self.manifest_path, self.manifest)
        initial_job = self._await_prompt_sent(message_id)
        self._restart_nuc_listener_and_retry(argv, message_id, initial_job)

    def _await_prompt_sent(self, message_id):
        deadline = time.monotonic() + self.args.wait_seconds
        while time.monotonic() < deadline:
            jobs = json_value(self.helper("jobs", f"{self.args.nuc_profile}/.everett/relay-cloudflare"), "NUC receiver ledger")
            matches = [row for row in jobs if row.get("relay_message_id") == message_id]
            require(len(matches) <= 1, "NUC ledger contains duplicate jobs for the same public message")
            if matches and matches[0].get("state") == "prompt_sent":
                require(matches[0].get("target_pid") == self.claude_row["pid"], "NUC ledger target PID changed after preflight")
                return matches[0]
            time.sleep(1)
        raise AcceptanceError("timed out waiting for the first receiver ledger job to reach prompt_sent")

    def _restart_nuc_listener_and_retry(self, argv, message_id, original_job):
        listener = next(item for item in self.listeners if item.host == "nuc")
        recorded = next(item for item in self.manifest.listeners if item.get("host") == "nuc" and item.get("pid") == listener.pid)
        stopped = self.helper("stop", listener.pid, self.args.nuc_profile, recorded.get("start_ticks", ""))
        require(stopped.strip() in ("stopped", "already-stopped"), "owned NUC relay listener could not be safely stopped for the idempotent retry check")
        self.listeners.remove(listener)
        self.manifest.cleanup["nuc_listener_first"] = stopped.strip()
        self.manifest.results["listener_restart"] = "first owned NUC listener stopped"
        self.manifest.phase = "retry_after_restart"
        save_manifest(self.manifest_path, self.manifest)
        log_path = f"{self.args.nuc_profile}/.everett/relay-cloudflare/receiver-wake/acceptance-{self.manifest.run_id}-retry.log"
        self.manifest.listeners.append({"host": "nuc", "state": "starting", "log": log_path})
        save_manifest(self.manifest_path, self.manifest)
        started = json_value(self.helper("start", self.args.nuc_binary, log_path, self.args.nuc_claude_binary), "NUC relay listener restart")
        process = Listener("nuc", int(started["pid"]), float(started["started"]))
        self.listeners.append(process)
        entry = self.manifest.listeners[-1]
        entry.update({"pid": process.pid, "started": process.started})
        save_manifest(self.manifest_path, self.manifest)
        inspection = json_value(self.helper("inspect-pid", process.pid, self.args.nuc_profile), "NUC restarted relay ownership")
        require(inspection.get("owned"), "restarted NUC relay process ownership could not be proved")
        entry["start_ticks"] = inspection.get("start_ticks", "")
        save_manifest(self.manifest_path, self.manifest)
        retry = self.local(argv, timeout=25, sender=True)
        require(retry.get("message_id") == message_id, "retry after listener restart created a second public request")
        time.sleep(2)
        jobs = json_value(self.helper("jobs", f"{self.args.nuc_profile}/.everett/relay-cloudflare"), "NUC receiver ledger after restart retry")
        matches = [row for row in jobs if row.get("relay_message_id") == message_id]
        require(len(matches) == 1 and matches[0] == original_job, "same-key retry after listener restart changed or duplicated the receiver attempt")
        self.manifest.results["duplicate_retry_after_restart"] = "same idempotency key returned original message ID"
        self.manifest.results["receiver_attempt_after_restart"] = "ledger unchanged after listener restart and duplicate retry"
        save_manifest(self.manifest_path, self.manifest)

    def verify(self):
        self._set_phase("verify")
        deadline = time.monotonic() + self.args.wait_seconds
        reply = None
        status_value = None
        jobs = []
        while time.monotonic() < deadline:
            status_value = self.local(["peer", "status", self.args.nuc_alias, self.manifest.message_id], timeout=18)
            inbox = self.local(["inbox", "--session", "human", "--peek", "--json"], timeout=12, sender=True)
            messages = inbox.get("messages", []) if isinstance(inbox, dict) else []
            matches = matching_replies(messages, self.manifest.message_id, self.args.session_id, self.args.nuc_device_id, self.manifest.marker)
            if matches:
                require(len(matches) == 1, "Mac human inbox contains duplicate matching replies")
                reply = matches[0]
            jobs = json_value(self.helper("jobs", f"{self.args.nuc_profile}/.everett/relay-cloudflare"), "NUC receiver ledger")
            if reply is not None and status_value and status_value.get("state") == "replied":
                break
            time.sleep(min(2.0, max(0.0, deadline - time.monotonic())))
        require(reply is not None and status_value and status_value.get("state") == "replied", "timed out waiting for both the correlated Claude reply and signed replied status")
        current_claude = self._claude_row(self.args.session_id, self._read_agents())
        require(current_claude.get("pid") == self.claude_row["pid"], "real Claude session PID changed before returned-reply verification")
        jobs = json_value(self.helper("jobs", f"{self.args.nuc_profile}/.everett/relay-cloudflare"), "NUC receiver ledger")
        matching = [row for row in jobs if row.get("relay_message_id") == self.manifest.message_id]
        require(len(matching) == 1, "NUC receiver ledger must contain exactly one matching imported message")
        job = matching[0]
        require(job.get("session_id") == self.args.session_id and job.get("cwd") == self.args.cwd and job.get("sender_device_id") == self.args.mac_device_id, "receiver ledger import does not match the exact approved session, cwd, and sender")
        require(job.get("state") == "prompt_sent" and job.get("target_pid") == self.claude_row["pid"], "receiver ledger does not prove an attach to the preflighted Claude background PID")
        self.manifest.phase = "verified"
        self.manifest.results.update({"relay_status": status_value.get("state"), "receiver_job_state": job["state"], "target_pid": job["target_pid"], "target_inbox_message_id": job["local_id"], "target_inbox_session": self.args.session_id, "reply_to": reply["reply_to"], "reply_from": reply["from"], "reply_marker_found": True, "sender_inbox": "human", "duplicate_job_count": len(matching)})
        save_manifest(self.manifest_path, self.manifest)
        self.negative_probe()

    def negative_probe(self):
        self._set_phase("negative_probe")
        marker = "EVERETT-DENIED-WAKE-" + uuid.uuid4().hex.upper()
        key = "receiver-denied-" + uuid.uuid4().hex
        prompt = f"This is an Everett deny-path check. If this message reaches a model, reply with exactly {marker}."
        argv = send_argv(self.args.nuc_alias, self.args.denied_session_id, prompt, key)
        sent = self.local(argv, timeout=25, sender=True)
        message_id = sent.get("message_id")
        require(committed_send_state(sent.get("state")) and "wake" not in sent and UUID_RE.fullmatch(message_id or "") is not None, "deny probe was not one committed inbox request without sender-side wake")
        deadline = time.monotonic() + self.args.wait_seconds
        state = None
        while time.monotonic() < deadline:
            status_value = self.local(["peer", "status", self.args.nuc_alias, message_id], timeout=18)
            state = status_value.get("state")
            if state in ("delivered", "claimed", "replied"):
                break
            time.sleep(1)
        require(state in ("delivered", "claimed"), "deny probe did not reach a durable NUC inbox import")
        jobs = json_value(self.helper("jobs", f"{self.args.nuc_profile}/.everett/relay-cloudflare"), "NUC receiver ledger after deny probe")
        require(not any(row.get("relay_message_id") == message_id for row in jobs), "nonallowlisted request created a receiver wake job")
        rows = self._read_agents()
        denied_after = self._claude_row(self.args.denied_session_id, rows)
        require(denied_after.get("pid") == self.denied_claude_row.get("pid") and denied_after.get("state") == self.denied_claude_row.get("state") and denied_after.get("status") == self.denied_claude_row.get("status") and denied_after.get("waitingFor") in (None, ""), "nonallowlisted Claude session state changed during the deny probe")
        human = self.local(["inbox", "--session", "human", "--peek", "--json"], timeout=12, sender=True)
        denied_replies = matching_replies(human.get("messages", []), message_id, self.args.denied_session_id, self.args.nuc_device_id, marker)
        require(not denied_replies and state != "replied", "nonallowlisted Claude session replied, so denial was not proved")
        self.manifest.results["nonallowlisted_probe"] = {"message_id": message_id, "relay_state": state, "receiver_job_count": 0, "target_pid_unchanged": True, "target_state_unchanged": True, "reply_seen": False}
        cwd_after = json_value(self.helper("cwd-digest", self.args.cwd, timeout=70), "NUC project state after acceptance")
        self.manifest.results["cwd_state_after_sha256"] = cwd_after["sha256"]
        self.manifest.results["cwd_untracked_paths_after"] = cwd_after["untracked_paths"]
        require(cwd_after["sha256"] == self.manifest.results["cwd_state_before_sha256"], "NUC Claude project state changed during acceptance")
        self.manifest.results["cwd_state_unchanged"] = True
        save_manifest(self.manifest_path, self.manifest)

    def _set_phase(self, phase):
        self.manifest.phase = phase
        save_manifest(self.manifest_path, self.manifest)

    def cleanup(self):
        if not self.manifest:
            return
        self.manifest.phase = "cleanup"
        for listener in reversed(self.listeners):
            try:
                if listener.host == "mac" and listener.process is not None:
                    if listener.process.poll() is None:
                        listener.process.terminate()
                        try:
                            listener.process.wait(timeout=5)
                            self.manifest.cleanup["mac_listener"] = "stopped"
                        except subprocess.TimeoutExpired:
                            self.manifest.cleanup["mac_listener"] = "still-running"
                    else:
                        self.manifest.cleanup["mac_listener"] = "already-stopped"
                elif listener.host == "nuc":
                    entry = next((item for item in self.manifest.listeners if item.get("host") == "nuc" and item.get("pid") == listener.pid), {})
                    outcome = self.helper("stop-log", entry["log"], self.args.nuc_profile)
                    self.manifest.cleanup[f"nuc_listener_{listener.pid}"] = outcome.strip()
            except BaseException as error:
                self.manifest.cleanup[f"listener_{listener.host}_{listener.pid}_error"] = str(error)
        for entry in self.manifest.listeners:
            if entry.get("host") == "nuc" and not entry.get("pid") and entry.get("log"):
                try:
                    outcome = self.helper("stop-log", entry["log"], self.args.nuc_profile)
                    self.manifest.cleanup[f"nuc_listener_unreturned_{len(self.manifest.cleanup)}"] = outcome.strip()
                except BaseException as error:
                    self.manifest.cleanup[f"nuc_listener_unreturned_error_{len(self.manifest.cleanup)}"] = str(error)
        nuc_results = [value for key, value in self.manifest.cleanup.items() if key.startswith("nuc_listener_") and key != "nuc_listener_first"]
        self.manifest.cleanup["nuc_listener"] = "stopped" if nuc_results and all(value in ("stopped", "already-stopped") for value in nuc_results) else "unconfirmed"
        if self.manifest.policy_owned and self.manifest.policy_installed_b64 is not None:
            try:
                outcome = self.helper("restore-policy", self.policy_path, self.manifest.policy_installed_b64, self.manifest.policy_installed_identity or "-", self.manifest.policy_original_b64 or "-", self.manifest.policy_original_mode or 0o600)
                self.manifest.cleanup["policy"] = outcome.strip()
            except BaseException as error:
                self.manifest.cleanup["policy_error"] = str(error)
        save_manifest(self.manifest_path, self.manifest)


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nuc-ssh", required=True, help="existing SSH alias for the enrolled NUC")
    parser.add_argument("--nuc-alias", default="nuc", help="existing Everett relay peer alias on the Mac")
    parser.add_argument("--mac-alias", default="mac", help="existing Everett relay peer alias on the NUC")
    parser.add_argument("--mac-binary", required=True, help="explicit released Mac Everett binary path")
    parser.add_argument("--nuc-binary", required=True, help="explicit candidate NUC Everett binary path")
    parser.add_argument("--nuc-claude-binary", required=True, help="explicit absolute NUC Claude CLI path; its directory is prepended to the relay listener PATH")
    parser.add_argument("--nuc-sha256", required=True, help="expected SHA-256 of the candidate NUC binary")
    parser.add_argument("--source-commit", required=True, help="full source commit used to build the candidate NUC binary")
    parser.add_argument("--mac-profile", default="/var/folders/28/_6z07g0n5wnb458cz61h1y6r0000gn/T/everett-public-first-run-q84e6mu7")
    parser.add_argument("--nuc-profile", default="/tmp/everett-v2-release-twg4ygpr/public-home")
    parser.add_argument("--mac-device-id", required=True)
    parser.add_argument("--nuc-device-id", required=True)
    parser.add_argument("--mac-fingerprint", required=True)
    parser.add_argument("--nuc-fingerprint", required=True)
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--denied-session-id", required=True, help="a separate user-owned, idle Claude background session for the no-attach probe")
    parser.add_argument("--cwd", required=True)
    parser.add_argument("--manifest", required=True, help="new path for the owner-only run manifest")
    parser.add_argument("--wait-seconds", type=int, default=240)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    require(15 <= args.wait_seconds <= 600, "wait-seconds must be between 15 and 600")
    run = Acceptance(args)
    interrupted = False
    cleaning = False
    succeeded = False
    failure = None
    cleanup_error = None
    old_handlers = {}

    def on_signal(signum, _frame):
        nonlocal interrupted
        interrupted = True
        if not cleaning:
            raise AcceptanceInterrupted(f"interrupted by signal {signum}")

    for signum in (signal.SIGINT, signal.SIGTERM):
        old_handlers[signum] = signal.signal(signum, on_signal)
    try:
        run.preflight()
        run.prepare()
        run.send()
        run.verify()
        succeeded = True
    except BaseException as error:
        failure = error
    finally:
        cleaning = True
        owns_manifest = str(run.manifest_path) in MANIFEST_OWNERS
        if run.manifest and owns_manifest:
            if failure is not None:
                run.manifest.results["failure"] = str(failure) or failure.__class__.__name__
            try:
                run.cleanup()
            except BaseException as error:
                cleanup_error = error
                run.manifest.cleanup["error"] = str(error) or error.__class__.__name__
                try:
                    save_manifest(run.manifest_path, run.manifest)
                except BaseException:
                    pass
        for signum, handler in old_handlers.items():
            signal.signal(signum, handler)

    if failure is not None:
        status = "interrupted" if isinstance(failure, (AcceptanceInterrupted, KeyboardInterrupt)) else "failed"
        print(json.dumps({"status": status, "phase": run.manifest.phase if run.manifest else "preflight", "manifest": str(run.manifest_path), "error": str(failure) or failure.__class__.__name__, "cleanup": run.manifest.cleanup if run.manifest else {}}, sort_keys=True), file=sys.stderr)
        return 1
    if cleanup_error is not None:
        print(json.dumps({"status": "incomplete", "manifest": str(run.manifest_path), "error": str(cleanup_error) or cleanup_error.__class__.__name__, "cleanup": run.manifest.cleanup}, sort_keys=True))
        return 1
    if not run.manifest:
        print(json.dumps({"status": "failed", "phase": "preflight", "manifest": str(run.manifest_path), "error": "preflight ended without a manifest"}, sort_keys=True), file=sys.stderr)
        return 1
    try:
        complete = succeeded and not interrupted and run.manifest.results.get("draft_preservation") == "verified_external_native_composer_receipt" and run.manifest.results.get("cwd_state_unchanged") is True and run.manifest.cleanup.get("mac_listener") in ("stopped", "already-stopped") and run.manifest.cleanup.get("nuc_listener_first") in ("stopped", "already-stopped") and run.manifest.cleanup.get("nuc_listener") == "stopped" and run.manifest.cleanup.get("policy") in ("removed", "restored")
        if not complete:
            run.manifest.results["cleanup_complete"] = False
            save_manifest(run.manifest_path, run.manifest)
            print(json.dumps({"status": "incomplete", "manifest": str(run.manifest_path), "results": run.manifest.results, "cleanup": run.manifest.cleanup}, sort_keys=True))
            return 1
        run.manifest.results["cleanup_complete"] = True
        save_manifest(run.manifest_path, run.manifest)
        print(json.dumps({"status": "passed", "manifest": str(run.manifest_path), "message_id": run.manifest.message_id, "session_id": run.manifest.session_id, "reply_marker": run.manifest.marker, "results": run.manifest.results, "cleanup": run.manifest.cleanup}, sort_keys=True))
        return 0
    except BaseException as error:
        run.manifest.cleanup["error"] = str(error) or error.__class__.__name__
        run.manifest.results["cleanup_complete"] = False
        try:
            save_manifest(run.manifest_path, run.manifest)
        except BaseException:
            pass
        print(json.dumps({"status": "incomplete", "manifest": str(run.manifest_path), "error": str(error) or error.__class__.__name__, "cleanup": run.manifest.cleanup}, sort_keys=True))
        return 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AcceptanceError as error:
        print(json.dumps({"status": "failed", "phase": "arguments", "error": str(error)}), file=sys.stderr)
        raise SystemExit(2)
