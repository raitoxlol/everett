"""Native `/everett` slash command across CLI, TUI, Desktop, and gateway."""

from __future__ import annotations

import json
from typing import Callable

from ._everett import card_status, run_cli
from ._inbox import InboxTracker, valid_session_id


HELP_TEXT = (
    "Everett commands:\n"
    "  /everett status     Show Everett binary, account, bound session, inbox, and card status\n"
    "  /everett ls [hours] List recent coding-agent sessions (default 72h)\n"
    "  /everett inbox      Peek waiting Everett inbox message IDs without marking them read\n"
    "  /everett core       Show the compact Everett shared core for this workspace\n"
    "  /everett help       Show this help"
)


def make_everett_command(
    *,
    binary_resolver: Callable[[], str | None],
    cwd_resolver: Callable[[], str],
    live_wake_enabled: Callable[[], bool],
    inbox_tracker: InboxTracker,
    on_session_key: Callable[[str, str | None], None] | None = None,
) -> Callable[..., str]:
    def handle(
        raw_args: str = "",
        *,
        session_id: str | None = None,
        session_key: str | None = None,
        platform: str | None = None,
    ) -> str:
        if on_session_key is not None and valid_session_id(session_id) and session_key:
            on_session_key(str(session_id), str(session_key))
        tokens = (raw_args or "").strip().split()
        sub = tokens[0].lower() if tokens else "status"
        if sub in ("help", "-h", "--help"):
            return HELP_TEXT

        binary = binary_resolver()
        if not binary:
            return (
                "Everett executable not found on PATH. Install Everett or set "
                "`plugins.entries.everett-hermes.settings.binary`."
            )

        cwd = cwd_resolver()
        if sub == "status":
            return _status(binary, session_id, platform, live_wake_enabled(), inbox_tracker)
        if sub == "ls":
            hours = 72
            if len(tokens) > 1:
                try:
                    hours = max(1, min(int(tokens[1]), 168))
                except ValueError:
                    return "Usage: /everett ls [hours] (integer from 1 to 168)"
            return _list_sessions(binary, hours, cwd)
        if sub == "inbox":
            return _peek_inbox(session_id, inbox_tracker)
        if sub == "core":
            return _show_core(binary, cwd)
        return f"Unknown subcommand `{sub}`.\n\n{HELP_TEXT}"

    return handle


def _status(
    binary: str,
    session_id: str | None,
    platform: str | None,
    live_wake: bool,
    inbox_tracker: InboxTracker,
) -> str:
    ver_res = run_cli(binary, ["--version"], timeout=3.0)
    version = ver_res.stdout.splitlines()[0].strip() if ver_res and ver_res.stdout else "unknown"

    who_res = run_cli(binary, ["whoami", "--json"], timeout=5.0)
    account = "unknown"
    if who_res is not None and who_res.stdout.strip():
        try:
            data = json.loads(who_res.stdout)
            if isinstance(data, dict):
                if data.get("signed_in") is True:
                    owner = data.get("owner") or "signed in"
                    device = data.get("device_name") or ""
                    account = f"{owner} ({device})" if device else str(owner)
                elif data.get("signed_in") is False:
                    account = "signed out (run `everett login`)"
        except json.JSONDecodeError:
            pass

    bound = session_id if valid_session_id(session_id) else "unbound"
    unread = len(inbox_tracker.peek_ids(session_id)) if valid_session_id(session_id) else 0
    card = card_status(session_id) if valid_session_id(session_id) else "none"
    surface = platform or "unknown"

    return "\n".join([
        f"Everett: {version} ({binary})",
        f"Account: {account}",
        f"Hermes session: {bound} (surface: {surface})",
        f"Unread inbox: {unread} | Card: {card} | Live wake: {'on' if live_wake else 'off'}",
    ])


def _list_sessions(binary: str, hours: int, cwd: str) -> str:
    res = run_cli(binary, ["ls", "--json", "--hours", str(hours)], cwd=cwd, timeout=8.0)
    if res is None or res.returncode != 0:
        return "Could not list Everett sessions. Run `everett ls` to inspect."
    try:
        rows = json.loads(res.stdout)
    except json.JSONDecodeError:
        return "Everett returned malformed session list output."
    if not isinstance(rows, list) or not rows:
        return f"No Everett sessions found in the last {hours}h."

    lines = [f"Recent Everett sessions (last {hours}h):"]
    for item in rows[:15]:
        if not isinstance(item, dict):
            continue
        sid = str(item.get("id") or "")
        harness = str(item.get("harness") or "?")
        dot = "●" if item.get("running") else "○"
        summary = str(item.get("title") or item.get("what") or item.get("first_user") or "").strip()
        summary = " ".join(summary.split())[:80]
        lines.append(f"  {dot} [{harness}] {sid} — {summary}" if summary else f"  {dot} [{harness}] {sid}")
    if len(rows) > 15:
        lines.append(f"  ... ({len(rows) - 15} more; call everett_ls for the full view)")
    return "\n".join(lines)


def _peek_inbox(session_id: str | None, inbox_tracker: InboxTracker) -> str:
    if not valid_session_id(session_id):
        return "No Hermes session ID is bound to this command context."
    assert session_id is not None
    ids = inbox_tracker.peek_ids(session_id)
    if not ids:
        return f"No unread Everett inbox messages for session {session_id}."
    shown = ", ".join(ids[:10])
    extra = f" (+{len(ids) - 10} more)" if len(ids) > 10 else ""
    return (
        f"Unread Everett inbox messages for {session_id}: {shown}{extra}\n"
        "Ask Hermes to call `everett_inbox` to read them as untrusted agent data."
    )


def _show_core(binary: str, cwd: str) -> str:
    res = run_cli(binary, ["core", "show"], cwd=cwd, timeout=5.0)
    if res is None or res.returncode != 0:
        return "Could not read Everett shared core."
    text = res.stdout.strip()
    if not text:
        return "Everett shared core is empty."
    return text[:3000]
