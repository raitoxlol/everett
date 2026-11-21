"""Specification tests for native everett-hermes plugin improvements (hill-climb)."""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


PLUGIN_ROOT = Path(__file__).resolve().parents[1]


def _load_plugin():
    name = "everett_hermes_plugin_under_test"
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(
        name,
        PLUGIN_ROOT / "__init__.py",
        submodule_search_locations=[str(PLUGIN_ROOT)],
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


PLUGIN = _load_plugin()
EVERETT_MOD = sys.modules["everett_hermes_plugin_under_test._everett"]
INBOX_MOD = sys.modules["everett_hermes_plugin_under_test._inbox"]
WAKE_MOD = sys.modules["everett_hermes_plugin_under_test._wake"]


class _FakeCtx:
    def __init__(self, settings: dict | None = None, *, cwd: str = ""):
        self.settings = dict(settings or {})
        self.cwd = cwd
        self.tools: dict[str, dict] = {}
        self.hooks: dict[str, list] = {}
        self.commands: dict[str, dict] = {}
        self.prompt_sections: dict[str, dict] = {}
        self.skills: dict[str, dict] = {}
        self.unload_callbacks: list = []
        self.injected: list[dict] = []

    def get_config(self, key: str, default=None):
        return self.settings.get(key, default)

    def session_cwd(self) -> str:
        return self.cwd

    def register_tool(self, *, name, toolset, schema, handler, description="", **_kw):
        self.tools[name] = {"toolset": toolset, "schema": schema, "handler": handler, "description": description}

    def register_hook(self, hook_name, callback, **_kw):
        self.hooks.setdefault(hook_name, []).append(callback)

    def register_command(self, name, handler, description="", args_hint="", **_kw):
        self.commands[name] = {"handler": handler, "description": description, "args_hint": args_hint}

    def register_system_prompt_section(self, section_id, content, *, position="after_memory", max_chars=4000):
        self.prompt_sections[section_id] = {"content": content, "position": position, "max_chars": max_chars}

    def register_skill(self, name, path, description="", **_kw):
        self.skills[name] = {"path": Path(path), "description": description}

    def inject_message(self, content, role="user", *, session_key=None, origin=None):
        self.injected.append({"content": content, "role": role, "session_key": session_key})
        return True

    def on_unload(self, callback):
        self.unload_callbacks.append(callback)

    def unload(self):
        for cb in list(self.unload_callbacks):
            cb()


def _write_fake_everett(directory: Path) -> Path:
    script = directory / "everett"
    script.write_text(
        """#!/usr/bin/env python3
import json, os, sys
from pathlib import Path

args = sys.argv[1:]
home = Path(os.environ.get("EVERETT_HOME", os.environ["HOME"]))
if args == ["--version"]:
    print("everett 2.4.0-fake")
    raise SystemExit(0)
if args[:2] == ["hook", "hermes_bind"]:
    sid = args[2]
    live = home / ".everett" / "inbox" / "live" / f"{sid}.json"
    live.parent.mkdir(parents=True, exist_ok=True)
    live.write_text(json.dumps({"harness": "hermes", "sid": sid}), encoding="utf-8")
    raise SystemExit(0)
if args[:2] == ["hook", "hermes_release"]:
    sid = args[2]
    live = home / ".everett" / "inbox" / "live" / f"{sid}.json"
    live.unlink(missing_ok=True)
    raise SystemExit(0)
if args[:1] == ["sent"] and "--json" in args:
    sid = os.environ.get("EVERETT_SESSION_ID", "")
    print(json.dumps({
        "session": sid,
        "requests": [
            {"message_id": "mreq101", "to": "peer-codex-1", "ts": 1700000000.0},
        ],
    }))
    raise SystemExit(0)
if args[:1] == ["mcp"]:
    for raw in sys.stdin:
        msg = json.loads(raw)
        method, req_id = msg.get("method"), msg.get("id")
        if method == "initialize":
            print(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}}}), flush=True)
        elif method == "tools/list":
            print(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": {"tools": [
                {"name": "everett_whoami", "description": "whoami", "inputSchema": {"type": "object", "properties": {}}},
            ]}}), flush=True)
        elif method == "tools/call":
            payload = {"session_id": os.environ.get("EVERETT_SESSION_ID"), "harness": "hermes"}
            print(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": {"content": [{"type": "text", "text": json.dumps(payload)}], "structuredContent": payload}}), flush=True)
    raise SystemExit(0)
raise SystemExit(2)
""",
        encoding="utf-8",
    )
    script.chmod(0o755)
    return script


def _write_inbox(home: Path, sid: str, records: list[dict], *, done_ids: list[str] | None = None) -> None:
    inbox = home / ".everett" / "inbox"
    inbox.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(home / ".everett", 0o700)
    os.chmod(inbox, 0o700)
    jsonl = inbox / f"{sid}.jsonl"
    fd = os.open(jsonl, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write("".join(json.dumps(r) + "\n" for r in records).encode("utf-8"))
    if done_ids is not None:
        done = inbox / f"{sid}.done"
        dfd = os.open(done, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(dfd, "wb") as stream:
            stream.write("".join(f"{mid}\n" for mid in done_ids).encode("utf-8"))


class HermesPluginImprovementSpecs(unittest.TestCase):
    def test_session_reset_cleans_up_old_session_and_binds_new_session(self):
        manifest = (PLUGIN_ROOT / "plugin.yaml").read_text(encoding="utf-8")
        self.assertIn("- on_session_reset", manifest)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_bin = _write_fake_everett(root)
            ctx = _FakeCtx(settings={"binary": str(fake_bin), "live_wake": True, "wake_poll_seconds": 60.0}, cwd=str(root))
            with patch.dict(os.environ, {"EVERETT_HOME": str(root), "HOME": str(root)}, clear=False):
                os.environ.pop("EVERETT_SEND", None)
                PLUGIN.register(ctx)
                self.addCleanup(ctx.unload)

                self.assertIn("on_session_reset", ctx.hooks)
                ctx.hooks["on_session_start"][0](session_id="hermes-old", platform="telegram", session_key="tg:1")
                old_live = root / ".everett" / "inbox" / "live" / "hermes-old.json"
                new_live = root / ".everett" / "inbox" / "live" / "hermes-new.json"
                self.assertTrue(old_live.is_file())

                ctx.hooks["on_session_reset"][0](
                    session_id="hermes-new",
                    old_session_id="hermes-old",
                    new_session_id="hermes-new",
                    platform="telegram",
                )
                self.assertFalse(old_live.is_file())
                self.assertTrue(new_live.is_file())

    def test_session_delete_cleans_up_all_deleted_ids_without_writing_auto_card(self):
        manifest = (PLUGIN_ROOT / "plugin.yaml").read_text(encoding="utf-8")
        self.assertIn("- on_session_delete", manifest)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_bin = _write_fake_everett(root)
            ctx = _FakeCtx(settings={"binary": str(fake_bin)}, cwd=str(root))
            with patch.dict(os.environ, {"EVERETT_HOME": str(root), "HOME": str(root)}, clear=False):
                os.environ.pop("EVERETT_SEND", None)
                PLUGIN.register(ctx)
                self.addCleanup(ctx.unload)

                self.assertIn("on_session_delete", ctx.hooks)
                for sid in ("hermes-del-1", "hermes-del-2"):
                    ctx.hooks["on_session_start"][0](session_id=sid, platform="cli")
                    self.assertTrue((root / ".everett" / "inbox" / "live" / f"{sid}.json").is_file())

                ctx.hooks["on_session_delete"][0](
                    session_id="hermes-del-1",
                    deleted_ids=["hermes-del-1", "hermes-del-2"],
                )
                for sid in ("hermes-del-1", "hermes-del-2"):
                    self.assertFalse((root / ".everett" / "inbox" / "live" / f"{sid}.json").is_file())
                    self.assertFalse((root / ".everett" / "cards" / f"{sid}.md").exists())

    def test_inbox_tracker_prunes_acknowledged_ids_so_long_lived_sessions_never_go_deaf(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sid = "hermes-long-lived"
            now = time.time()
            tracker = INBOX_MOD.InboxTracker(max_seen_per_session=4)

            batch1 = [{"id": f"mfirst{i}", "to": sid, "text": "hi", "ts": now} for i in range(4)]
            _write_inbox(root, sid, batch1)
            self.assertEqual(tracker.claim_new_ids(sid, home=root), [f"mfirst{i}" for i in range(4)])

            # Acknowledge all 4 messages into .done and append a 5th unread message.
            batch2 = batch1 + [{"id": "mfifth5", "to": sid, "text": "still listening", "ts": now}]
            _write_inbox(root, sid, batch2, done_ids=[f"mfirst{i}" for i in range(4)])
            self.assertEqual(tracker.unseen_ids(sid, home=root), ["mfifth5"])
            self.assertEqual(tracker.claim_new_ids(sid, home=root), ["mfifth5"])

    def test_everett_slash_command_supports_card_and_sent_subcommands(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_bin = _write_fake_everett(root)
            sid = "hermes-cmd-extra"
            cards_dir = root / ".everett" / "cards"
            cards_dir.mkdir(mode=0o700, parents=True)
            (cards_dir / f"{sid}.md").write_text(
                "What: Hardening Hermes plugin\nState: Adding slash subcommands\nNext: Run smoke\n",
                encoding="utf-8",
            )
            ctx = _FakeCtx(settings={"binary": str(fake_bin)}, cwd=str(root))
            with patch.dict(os.environ, {"EVERETT_HOME": str(root), "HOME": str(root)}, clear=False):
                os.environ.pop("EVERETT_SEND", None)
                PLUGIN.register(ctx)
                self.addCleanup(ctx.unload)

                cmd = ctx.commands["everett"]["handler"]
                help_out = cmd("help")
                self.assertIn("/everett card", help_out)
                self.assertIn("/everett sent", help_out)

                card_out = cmd("card", session_id=sid)
                self.assertIn("agent", card_out)
                self.assertIn("Hardening Hermes plugin", card_out)

                sent_out = cmd("sent", session_id=sid)
                self.assertIn("mreq101", sent_out)
                self.assertIn("peer-codex-1", sent_out)

    def test_lifecycle_maps_stay_bounded_across_many_finalized_sessions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fake_bin = _write_fake_everett(root)
            ctx = _FakeCtx(settings={"binary": str(fake_bin), "auto_cards": False}, cwd=str(root))
            watcher = WAKE_MOD.InboxWakeWatcher(ctx, poll_seconds=60.0)
            self.addCleanup(watcher.stop)
            pool = EVERETT_MOD.McpPool(lambda: str(fake_bin), max_clients=4)
            self.addCleanup(pool.close_all)

            for i in range(600):
                sid = f"hermes-churn-{i}"
                watcher.track(sid, session_key=f"tg:{i}", platform="telegram")
                watcher.untrack(sid)
                pool.close_session(sid)

            # Untracked sessions must not leak per-session locks in InboxWakeWatcher.
            self.assertEqual(len(watcher._session_locks), 0)
            # McpPool._session_generations must be bounded rather than growing without limit.
            self.assertLessEqual(len(pool._session_generations), 512)


if __name__ == "__main__":
    unittest.main()
