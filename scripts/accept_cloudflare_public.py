#!/usr/bin/env python3
"""Drive a bounded message round trip through the public Cloudflare issuer."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import time
import uuid

PUBLIC_ISSUER = "https://everett.raitox.tech"
RUN_RE = re.compile(r"^[a-f0-9]{12}$")
HOST_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.@-]*$")


class AcceptanceError(Exception):
    pass


@dataclass(frozen=True)
class Identity:
    owner: str
    device_id: str
    issuer: str


@dataclass(frozen=True)
class Bundle:
    owner: str
    device_id: str
    fingerprint: str


@dataclass(frozen=True)
class TestRun:
    run_id: str
    peer_name: str
    inbox_session: str
    idempotency_key: str
    message: str
    reply: str


def make_run(run_id: str | None = None) -> TestRun:
    value = run_id or uuid.uuid4().hex[:12]
    if not RUN_RE.fullmatch(value):
        raise AcceptanceError("run id must be 12 lowercase hexadecimal characters")
    return TestRun(
        run_id=value,
        peer_name=f"public-{value}",
        inbox_session=f"public{value}",
        idempotency_key=f"public-accept-{value}",
        message=f"Everett public relay acceptance {value}",
        reply=f"Everett public relay reply {value}",
    )


def parse_identity(value: object) -> Identity:
    if not isinstance(value, dict):
        raise AcceptanceError("whoami did not return a JSON object")
    if value.get("signed_in") is not True or value.get("verified") is not True:
        raise AcceptanceError("device is not signed in with a verified account")
    if value.get("provider") != "cloudflare":
        raise AcceptanceError("device is not using native Cloudflare sign-in")
    owner, device_id, issuer = (value.get(key) for key in ("owner", "agent_id", "issuer"))
    if not all(isinstance(item, str) and item for item in (owner, device_id, issuer)):
        raise AcceptanceError("whoami omitted public account metadata")
    if issuer.rstrip("/") != PUBLIC_ISSUER:
        raise AcceptanceError("device is not signed in to the public Everett issuer")
    return Identity(owner=owner, device_id=device_id, issuer=issuer.rstrip("/"))


def parse_bundle(value: object) -> Bundle:
    if not isinstance(value, dict) or not isinstance(value.get("device"), dict):
        raise AcceptanceError("relay register did not return a public device bundle")
    device_id = value["device"].get("deviceId")
    owner = value["device"].get("owner")
    fingerprint = value.get("fingerprint")
    if not isinstance(device_id, str) or not device_id:
        raise AcceptanceError("relay register omitted the device id")
    if not isinstance(owner, str) or not owner:
        raise AcceptanceError("relay register omitted the owner id")
    if not isinstance(fingerprint, str) or not re.fullmatch(r"[a-f0-9]{64}", fingerprint):
        raise AcceptanceError("relay register omitted a valid public fingerprint")
    return Bundle(owner=owner, device_id=device_id, fingerprint=fingerprint)


def validate_bundle_pair(local: Bundle, remote: Bundle) -> None:
    if local.owner != remote.owner:
        raise AcceptanceError("registered devices are not on the same account")
    if local.device_id == remote.device_id:
        raise AcceptanceError("registered devices report the same device identity")
    if local.fingerprint == remote.fingerprint:
        raise AcceptanceError("registered devices report the same public fingerprint")


def parse_peers(value: object) -> dict[str, dict[str, object]]:
    if not isinstance(value, dict) or any(
        not isinstance(name, str) or not isinstance(peer, dict) for name, peer in value.items()
    ):
        raise AcceptanceError("peer list did not return a JSON object of peer records")
    return value


def ensure_alias_available(peers: dict[str, dict[str, object]], alias: str) -> None:
    if alias in peers:
        raise AcceptanceError(f"temporary peer alias {alias} already exists; choose a new run id")


def peer_matches(peers: dict[str, dict[str, object]], alias: str, device_id: str, fingerprint: str) -> bool:
    peer = peers.get(alias)
    return isinstance(peer, dict) and peer.get("device_id") == device_id and peer.get("fingerprint") == fingerprint


def manifest_path(run_id: str) -> Path:
    home = Path(os.environ.get("EVERETT_HOME") or os.environ.get("HOME") or Path.home())
    return home / ".everett" / "public-acceptance" / f"{run_id}.json"


def save_manifest(path: Path, manifest: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        json.dump(manifest, output, sort_keys=True)
        output.write("\n")


def load_manifest(path: Path, run_id: str, ssh_host: str) -> dict[str, str]:
    try:
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "r", encoding="utf-8") as source:
            value = json.load(source)
    except (OSError, json.JSONDecodeError, UnicodeDecodeError) as error:
        raise AcceptanceError("no valid local harness ownership record exists for this run") from error
    fields = (
        "run_id", "ssh_host", "local_device_id", "remote_device_id",
        "local_fingerprint", "remote_fingerprint",
    )
    if not isinstance(value, dict) or any(not isinstance(value.get(field), str) for field in fields):
        raise AcceptanceError("local harness ownership record is incomplete")
    if value["run_id"] != run_id or value["ssh_host"] != ssh_host:
        raise AcceptanceError("run id or NUC SSH host does not match its local ownership record")
    if value["local_device_id"] == value["remote_device_id"]:
        raise AcceptanceError("local ownership record has duplicate device identities")
    if value["local_fingerprint"] == value["remote_fingerprint"]:
        raise AcceptanceError("local ownership record has duplicate device fingerprints")
    return value


def verify_run_pins(binary: str, host: str, remote_binary: str, run: TestRun) -> tuple[dict[str, str], dict[str, dict[str, object]], dict[str, dict[str, object]]]:
    manifest = load_manifest(manifest_path(run.run_id), run.run_id, host)
    local_peers = parse_peers(local_json(binary, "peer", "list"))
    remote_peers = parse_peers(remote_json(host, remote_binary, "peer", "list"))
    if not peer_matches(local_peers, run.peer_name, manifest["remote_device_id"], manifest["remote_fingerprint"]):
        raise AcceptanceError("Mac temporary alias is missing or no longer matches its harness-owned NUC pin")
    if not peer_matches(remote_peers, run.peer_name, manifest["local_device_id"], manifest["local_fingerprint"]):
        raise AcceptanceError("NUC temporary alias is missing or no longer matches its harness-owned Mac pin")
    return manifest, local_peers, remote_peers


def ssh_command(host: str, remote_binary: str, argv: list[str]) -> list[str]:
    if not HOST_RE.fullmatch(host):
        raise AcceptanceError("SSH host must be a plain host alias or user@host")
    if not remote_binary or any(ch.isspace() for ch in remote_binary):
        raise AcceptanceError("remote binary must be one executable name or path")
    remote = shlex.join([remote_binary, *argv])
    return ["ssh", "-T", host, remote]


def command_json(command: list[str], *, env: dict[str, str] | None = None, timeout: int = 30) -> object:
    try:
        result = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=env,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AcceptanceError(f"command failed to start or timed out: {command_label(command)}") from error
    if result.returncode != 0:
        raise AcceptanceError(f"{command_label(command)} exited with status {result.returncode}")
    try:
        return json.loads(result.stdout)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise AcceptanceError(f"{command_label(command)} did not return JSON") from error


def command_success(command: list[str], *, timeout: int = 30) -> None:
    try:
        result = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AcceptanceError(f"command failed to start or timed out: {command_label(command)}") from error
    if result.returncode != 0:
        raise AcceptanceError(f"{command_label(command)} exited with status {result.returncode}")


def command_label(command: list[str]) -> str:
    return command[0] if command and command[0] != "ssh" else "remote Everett command"


def local_json(binary: str, *argv: str, session_id: str | None = None, timeout: int = 30) -> object:
    env = os.environ.copy()
    if session_id is not None:
        env["EVERETT_SESSION_ID"] = session_id
    return command_json([binary, *argv], env=env, timeout=timeout)


def remote_json(host: str, remote_binary: str, *argv: str, timeout: int = 30) -> object:
    return command_json(ssh_command(host, remote_binary, list(argv)), timeout=timeout)


def remote_success(host: str, remote_binary: str, *argv: str, timeout: int = 30) -> None:
    command_success(ssh_command(host, remote_binary, list(argv)), timeout=timeout)


def identities(binary: str, host: str, remote_binary: str) -> tuple[Identity, Identity]:
    local = parse_identity(local_json(binary, "whoami", "--json"))
    remote = parse_identity(remote_json(host, remote_binary, "whoami", "--json"))
    if local.owner != remote.owner:
        raise AcceptanceError("Mac and NUC are signed in to different Everett accounts")
    if local.device_id == remote.device_id:
        raise AcceptanceError("Mac and NUC report the same device identity")
    return local, remote


def bundle(binary: str, host: str, remote_binary: str) -> tuple[Bundle, Bundle]:
    local = parse_bundle(local_json(binary, "relay", "register"))
    remote = parse_bundle(remote_json(host, remote_binary, "relay", "register"))
    return local, remote


def require_confirmation(prompt: str, expected: str) -> None:
    try:
        entered = input(f"{prompt}\nType {expected!r} to continue: ")
    except EOFError as error:
        raise AcceptanceError("interactive confirmation is required") from error
    if entered != expected:
        raise AcceptanceError("confirmation did not match; no next action was run")


def parse_messages(value: object) -> list[dict[str, object]]:
    if not isinstance(value, dict) or not isinstance(value.get("messages"), list):
        raise AcceptanceError("inbox did not return a messages array")
    messages = value["messages"]
    if not all(isinstance(message, dict) for message in messages):
        raise AcceptanceError("inbox returned an invalid message row")
    return messages


def peer_status(binary: str, run: TestRun, message_id: str) -> str:
    value = local_json(binary, "peer", "status", run.peer_name, message_id)
    if not isinstance(value, dict) or not isinstance(value.get("state"), str):
        raise AcceptanceError("peer status did not return a state")
    return value["state"]


def plan() -> int:
    print("No network or Everett command is run by this plan.")
    print("1. Run preflight to verify existing Mac and NUC credentials.")
    print("2. Run prepare to register the existing device keys and add temporary peer pins.")
    print("3. Stop only the NUC relay listener, then run queue to send twice with one idempotency key.")
    print("4. Start everett relay on both devices, then run deliver to read once and reply.")
    print("5. Run cleanup to remove the temporary peer pins. Device revocation is a separate owner action.")
    return 0


def common_parser(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--ssh-host", required=True, help="SSH alias for the NUC, such as nuc")
    parser.add_argument("--remote-everett", default="everett", help="Everett executable on the NUC")
    parser.add_argument("--everett", default=shutil.which("everett") or "everett", help="local Everett executable")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("plan", help="show the staged procedure without running commands")
    preflight = commands.add_parser("preflight", help="read and verify existing device identities")
    common_parser(preflight)
    prepare = commands.add_parser("prepare", help="register existing devices and add temporary peer pins")
    common_parser(prepare)
    prepare.add_argument("--run-id", help="12 lowercase hex characters; generated when omitted")
    queue = commands.add_parser("queue", help="send one idempotent message while the NUC listener is stopped")
    common_parser(queue)
    queue.add_argument("--run-id", required=True)
    queue.add_argument("--timeout", type=int, default=30)
    deliver = commands.add_parser("deliver", help="read the queued message once and send its reply")
    common_parser(deliver)
    deliver.add_argument("--run-id", required=True)
    deliver.add_argument("--timeout", type=int, default=180)
    cleanup = commands.add_parser("cleanup", help="remove the temporary peer pins only")
    common_parser(cleanup)
    cleanup.add_argument("--run-id", required=True)
    return parser


def run(args: argparse.Namespace) -> int:
    if args.command == "plan":
        return plan()

    try:
        if args.command == "preflight":
            local, remote = identities(args.everett, args.ssh_host, args.remote_everett)
            print("PASS: both existing devices are verified on the public issuer and share one account.")
            print(f"Device identities are distinct. Mac {local.device_id[:8]}…, NUC {remote.device_id[:8]}…")
            return 0

        test = make_run(args.run_id)

        if args.command == "prepare":
            local_identity, remote_identity = identities(args.everett, args.ssh_host, args.remote_everett)
            state_path = manifest_path(test.run_id)
            if state_path.exists():
                raise AcceptanceError("a local ownership record already exists; choose a new run id")
            local_peers = parse_peers(local_json(args.everett, "peer", "list"))
            remote_peers = parse_peers(remote_json(args.ssh_host, args.remote_everett, "peer", "list"))
            ensure_alias_available(local_peers, test.peer_name)
            ensure_alias_available(remote_peers, test.peer_name)
            require_confirmation(
                "This registers the current public keys and writes one temporary peer pin on each device. It does not enroll or revoke a device.",
                args.ssh_host,
            )
            local_bundle, remote_bundle = bundle(args.everett, args.ssh_host, args.remote_everett)
            if local_bundle.owner != local_identity.owner or local_bundle.device_id != local_identity.device_id:
                raise AcceptanceError("Mac relay registration does not match its verified account identity")
            if remote_bundle.owner != remote_identity.owner or remote_bundle.device_id != remote_identity.device_id:
                raise AcceptanceError("NUC relay registration does not match its verified account identity")
            validate_bundle_pair(local_bundle, remote_bundle)
            local_json(
                args.everett, "peer", "add-relay", test.peer_name,
                "--device", remote_bundle.device_id, "--fingerprint", remote_bundle.fingerprint,
            )
            try:
                remote_json(
                    args.ssh_host, args.remote_everett, "peer", "add-relay", test.peer_name,
                    "--device", local_bundle.device_id, "--fingerprint", local_bundle.fingerprint,
                )
            except AcceptanceError:
                current_local = parse_peers(local_json(args.everett, "peer", "list"))
                if peer_matches(current_local, test.peer_name, remote_bundle.device_id, remote_bundle.fingerprint):
                    local_json(args.everett, "peer", "remove", test.peer_name)
                raise
            try:
                save_manifest(state_path, {
                    "run_id": test.run_id,
                    "ssh_host": args.ssh_host,
                    "local_device_id": local_bundle.device_id,
                    "remote_device_id": remote_bundle.device_id,
                    "local_fingerprint": local_bundle.fingerprint,
                    "remote_fingerprint": remote_bundle.fingerprint,
                })
            except OSError as error:
                current_local = parse_peers(local_json(args.everett, "peer", "list"))
                current_remote = parse_peers(remote_json(args.ssh_host, args.remote_everett, "peer", "list"))
                if peer_matches(current_local, test.peer_name, remote_bundle.device_id, remote_bundle.fingerprint):
                    local_json(args.everett, "peer", "remove", test.peer_name)
                if peer_matches(current_remote, test.peer_name, local_bundle.device_id, local_bundle.fingerprint):
                    remote_json(args.ssh_host, args.remote_everett, "peer", "remove", test.peer_name)
                raise AcceptanceError("could not record harness ownership; matching new pins were rolled back") from error
            print(f"PASS: temporary pins added. Run id: {test.run_id}; peer alias: {test.peer_name}.")
            print("The local ownership record contains public device IDs and fingerprints only.")
            print("No login, enrollment, or revocation command was run.")
            return 0

        if args.command == "queue":
            verify_run_pins(args.everett, args.ssh_host, args.remote_everett, test)
            require_confirmation(
                f"Stop the NUC's everett relay listener for run {test.run_id}. This sends the same test message twice.",
                f"QUEUE {test.run_id}",
            )
            first = local_json(
                args.everett, "send", test.message,
                "--to", f"{test.peer_name}/codex/{test.inbox_session}",
                "--mode", "inbox", "--idempotency-key", test.idempotency_key, "--json",
                session_id=test.inbox_session,
            )
            duplicate = local_json(
                args.everett, "send", test.message,
                "--to", f"{test.peer_name}/codex/{test.inbox_session}",
                "--mode", "inbox", "--idempotency-key", test.idempotency_key, "--json",
                session_id=test.inbox_session,
            )
            first_id = first.get("message_id") if isinstance(first, dict) else None
            duplicate_id = duplicate.get("message_id") if isinstance(duplicate, dict) else None
            if not isinstance(first_id, str) or not first_id or first_id != duplicate_id:
                raise AcceptanceError("repeat send did not return the same message id")
            state = peer_status(args.everett, test, first_id)
            if state != "queued":
                raise AcceptanceError(f"expected queued while NUC listener was stopped, observed {state}")
            print(f"PASS: one queued message after identical retry. Run id: {test.run_id}; message id: {first_id}.")
            print("Start `everett relay` on both Mac and NUC, then run the deliver command with this run id.")
            return 0

        if args.command == "deliver":
            verify_run_pins(args.everett, args.ssh_host, args.remote_everett, test)
            require_confirmation(
                f"Start the test relay listeners on both devices for run {test.run_id}. The driver will read one inbox message and reply.",
                f"DELIVER {test.run_id}",
            )
            deadline = time.monotonic() + args.timeout
            inbound: list[dict[str, object]] = []
            while time.monotonic() < deadline:
                inbox = remote_json(
                    args.ssh_host, args.remote_everett, "inbox", "--session", test.inbox_session, "--peek", "--json"
                )
                inbound = parse_messages(inbox)
                if inbound:
                    break
                time.sleep(2)
            matching = [message for message in inbound if message.get("text") == test.message]
            if len(inbound) != 1 or len(matching) != 1:
                raise AcceptanceError(f"expected one copy of the test message in the NUC inbox, found {len(inbound)}")
            message_id = matching[0].get("id")
            if not isinstance(message_id, str) or not message_id:
                raise AcceptanceError("received message has no id")
            relay_message_id = matching[0].get("relay_message_id")
            if not isinstance(relay_message_id, str) or not relay_message_id:
                raise AcceptanceError("received message has no relay id")
            consumed = parse_messages(remote_json(
                args.ssh_host, args.remote_everett, "inbox", "--session", test.inbox_session, "--json"
            ))
            if len(consumed) != 1 or consumed[0].get("id") != message_id:
                raise AcceptanceError("mark-read returned a different inbox message")
            remaining = parse_messages(remote_json(
                args.ssh_host, args.remote_everett, "inbox", "--session", test.inbox_session, "--peek", "--json"
            ))
            if remaining:
                raise AcceptanceError(f"NUC inbox still has {len(remaining)} pending message(s) after read")
            remote_success(args.ssh_host, args.remote_everett, "reply", message_id, test.reply)

            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                local_inbox = parse_messages(local_json(
                    args.everett, "inbox", "--session", test.inbox_session, "--peek", "--json"
                ))
                state = peer_status(args.everett, test, relay_message_id)
                replies = [message for message in local_inbox if message.get("text") == test.reply]
                if state == "replied" and len(replies) == 1 and len(local_inbox) == 1:
                    print(f"PASS: NUC read once, replied once, Mac observed replied state. Run id: {test.run_id}.")
                    print(f"Remove temporary peer pins with cleanup --run-id {test.run_id} after preserving this output.")
                    return 0
                time.sleep(2)
            raise AcceptanceError("timed out waiting for one reply and signed replied status on the Mac")

        if args.command == "cleanup":
            manifest = load_manifest(manifest_path(test.run_id), test.run_id, args.ssh_host)
            local_peers = parse_peers(local_json(args.everett, "peer", "list"))
            remote_peers = parse_peers(remote_json(args.ssh_host, args.remote_everett, "peer", "list"))
            if test.peer_name in local_peers and not peer_matches(
                local_peers, test.peer_name, manifest["remote_device_id"], manifest["remote_fingerprint"]
            ):
                raise AcceptanceError("Mac alias differs from the recorded harness pin; nothing was removed")
            if test.peer_name in remote_peers and not peer_matches(
                remote_peers, test.peer_name, manifest["local_device_id"], manifest["local_fingerprint"]
            ):
                raise AcceptanceError("NUC alias differs from the recorded harness pin; nothing was removed")
            require_confirmation(
                f"Remove only the peer aliases created by this harness run ({test.peer_name}). Device credentials and server records remain.",
                f"REMOVE {test.peer_name}",
            )
            if test.peer_name in local_peers:
                local_json(args.everett, "peer", "remove", test.peer_name)
            if test.peer_name in remote_peers:
                remote_json(args.ssh_host, args.remote_everett, "peer", "remove", test.peer_name)
            manifest_path(test.run_id).unlink()
            print(f"PASS: removed temporary peer pins for run {test.run_id}.")
            print("The run inbox and expired relay record remain as evidence. The local ownership record was removed; credentials were not accessed.")
            return 0

        raise AcceptanceError("unknown command")
    except AcceptanceError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1


def main() -> int:
    return run(build_parser().parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
