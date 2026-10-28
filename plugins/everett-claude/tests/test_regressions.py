import importlib.util
import json
import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from test_support import BASE_ENV


PLUGIN = Path(__file__).resolve().parents[1]
EVERETT = shutil.which("everett")
SID = "fix-own"
LOCAL_ID = "m0123456789"
RELAY_ID = "relay-" + "a" * 64
spec = importlib.util.spec_from_file_location("packet_f_wake", PLUGIN / "scripts/wake.py")
wake = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wake)


@unittest.skipUnless(EVERETT, "installed Everett binary required; do not build one")
class InboxRegressions(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="everett-f-fix-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.inbox = self.home / ".everett/inbox"
        self.inbox.parent.mkdir(mode=0o700)
        self.inbox.mkdir(mode=0o700)
        self.env = {**BASE_ENV, "EVERETT_HOME": str(self.home),
                    "EVERETT_SESSION_ID": SID, "EVERETT_CLAUDE_CHANNEL": "1"}
        self.write(SID, [self.message(LOCAL_ID)])

    def message(self, mid, to=SID, sender="fix-sender"):
        return {"id": mid, "to": to, "from": sender, "text": "inert fixture",
                "ts": time.time(), "hops": 1, "kind": "message"}

    def write(self, sid, messages):
        path = self.inbox / (sid + ".jsonl")
        path.write_text("".join(json.dumps(message) + "\n" for message in messages))
        path.chmod(0o600)
        return path

    def stop(self):
        result = subprocess.run([sys.executable, str(PLUGIN / "scripts/stop.py")],
                                input=json.dumps({"session_id": SID}), env=self.env,
                                capture_output=True, text=True, timeout=2)
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        return result.stdout

    def peek(self):
        with patch.dict(os.environ, self.env, clear=True):
            return wake.peek_ids(SID)

    def test_local_and_relay_ids_wake_stop_and_channel(self):
        invalid = ["--help", "relay-" + "A" * 64, "relay-" + "g" * 64,
                   "relay-" + "a" * 63, "relay-" + "a" * 65, "m" + "a" * 128]
        self.write(SID, [self.message(mid) for mid in [LOCAL_ID, RELAY_ID] + invalid])
        reason = json.loads(self.stop())["reason"]
        self.assertIn(LOCAL_ID, reason)
        self.assertIn(RELAY_ID, reason)
        self.assertEqual(re.findall(r'reply_to="([^"]+)"', reason), [LOCAL_ID, RELAY_ID])
        from test_wake import Channel
        client = Channel(self.env)
        try:
            client.initialize()
            content = client.messages.get(timeout=2)["params"]["content"]
            self.assertIn(LOCAL_ID, content)
            self.assertIn(RELAY_ID, content)
            self.assertEqual(re.findall(r'reply_to="([^"]+)"', content), [LOCAL_ID, RELAY_ID])
        finally:
            client.close()

    def test_misaddressed_record_cannot_generate_reply(self):
        self.write(SID, [self.message(LOCAL_ID, to="fix-victim")])
        self.assertEqual(self.stop(), "")
        self.write(SID, [self.message(LOCAL_ID)])
        self.assertIn(LOCAL_ID, json.loads(self.stop())["reason"])

    def mcp_reply(self, mid):
        messages = [
            {"id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-03-26", "capabilities": {},
                "clientInfo": {"name": "packet-f-fix", "version": "1"}}},
            {"method": "notifications/initialized"},
            {"id": 2, "method": "tools/call", "params": {"name": "everett_send",
                "arguments": {"reply_to": mid, "text": "inert fixture reply", "session_id": SID}}},
        ]
        result = subprocess.run([EVERETT, "mcp"], env=self.env,
                                input="".join(json.dumps({"jsonrpc": "2.0", **m}) + "\n" for m in messages),
                                capture_output=True, text=True, timeout=4)
        self.assertEqual(result.returncode, 0, result.stderr)
        return next(json.loads(line)["result"] for line in result.stdout.splitlines()
                    if json.loads(line).get("id") == 2)

    def test_reply_suggestion_uses_bound_api_and_collisions_stay_in_own_inbox(self):
        reason = json.loads(self.stop())["reason"]
        self.assertNotIn("everett reply", reason)
        self.assertIn("everett_whoami", reason)
        self.assertIn('everett_send(reply_to="m0123456789",', reason)
        self.assertIn('session_id="fix-own"', reason)
        self.write("fix-victim", [self.message(LOCAL_ID, to="fix-victim", sender="victim-sender")])
        self.write(SID, [])
        self.assertTrue(self.mcp_reply(LOCAL_ID)["isError"])
        self.assertFalse((self.inbox / "victim-sender.jsonl").exists())
        self.write(SID, [self.message(LOCAL_ID)])
        self.assertFalse(self.mcp_reply(LOCAL_ID).get("isError", False))
        reply = json.loads((self.inbox / "fix-sender.jsonl").read_text())
        self.assertEqual((reply["to"], reply["from"], reply["reply_to"]),
                         ("fix-sender", SID, LOCAL_ID))
        self.assertFalse((self.inbox / "victim-sender.jsonl").exists())

    def test_replacement_before_cli_launch_cannot_change_opened_inbox(self):
        own = self.inbox / (SID + ".jsonl")
        victim = self.write("fix-victim", [self.message("m9876543210", to="fix-victim")])
        popen = subprocess.Popen

        def swap(*args, **kwargs):
            own.unlink()
            own.symlink_to(victim)
            return popen(*args, **kwargs)

        with patch.object(wake.subprocess, "Popen", side_effect=swap):
            self.assertEqual(self.peek(), [LOCAL_ID])
        self.assertTrue(own.is_symlink())

    def test_symlink_swapped_at_open_is_never_read(self):
        self.assertEqual(self.peek(), [LOCAL_ID])
        own = self.inbox / (SID + ".jsonl")
        victim = self.write("fix-victim", [self.message("m9876543210")])
        victim_inode = victim.stat().st_ino
        original_open, original_read = os.open, os.read
        read_inodes = []

        def swap(name, *args, **kwargs):
            if name == SID + ".jsonl":
                own.unlink()
                own.symlink_to(victim)
            return original_open(name, *args, **kwargs)

        def record(fd, count):
            read_inodes.append(os.fstat(fd).st_ino)
            return original_read(fd, count)

        with patch.object(wake.os, "open", side_effect=swap), patch.object(wake.os, "read", side_effect=record):
            self.assertEqual(self.peek(), [])
        self.assertTrue(own.is_symlink())
        self.assertNotIn(victim_inode, read_inodes)

    def test_opened_directory_is_pinned_across_rename_and_symlink(self):
        original_open = wake.open_private
        pinned = self.inbox.with_name("original-inbox")
        victim = self.home / "victim-inbox"
        victim.mkdir(mode=0o700)
        path = victim / (SID + ".jsonl")
        path.write_text(json.dumps(self.message("m9876543210")) + "\n")
        path.chmod(0o600)

        def swap(stack, name, parent, directory=False):
            fd = original_open(stack, name, parent, directory=directory)
            if name == "inbox":
                self.inbox.rename(pinned)
                self.inbox.symlink_to(victim, target_is_directory=True)
            return fd

        with patch.object(wake, "open_private", side_effect=swap):
            self.assertEqual(self.peek(), [LOCAL_ID])
        self.assertTrue(self.inbox.is_symlink())
        self.assertEqual(json.loads((pinned / (SID + ".jsonl")).read_text())["id"], LOCAL_ID)

    def test_fstat_rejects_public_replacement_at_open(self):
        self.assertEqual(self.peek(), [LOCAL_ID])
        own = self.inbox / (SID + ".jsonl")
        original_open = os.open

        def swap(name, *args, **kwargs):
            if name == SID + ".jsonl":
                own.unlink()
                own.write_text(json.dumps(self.message("m9876543210")) + "\n")
                own.chmod(0o644)
            return original_open(name, *args, **kwargs)

        with patch.object(wake.os, "open", side_effect=swap):
            self.assertEqual(self.peek(), [])
        self.assertEqual(own.stat().st_mode & 0o777, 0o644)

    def test_done_expiry_and_snapshot_do_not_modify_original_state(self):
        expired = {**self.message("mExpired"), "ts": time.time() - 8 * 24 * 3600}
        self.write(SID, [self.message(LOCAL_ID), self.message(RELAY_ID), expired])
        done = self.inbox / (SID + ".done")
        done.write_text(LOCAL_ID + "\n")
        done.chmod(0o600)
        before = {p.name: p.read_bytes() for p in self.inbox.iterdir()}
        self.assertEqual(self.peek(), [RELAY_ID])
        self.assertEqual({p.name: p.read_bytes() for p in self.inbox.iterdir()}, before)

    def test_linked_or_nonregular_done_and_oversized_inbox_fail_open(self):
        self.assertEqual(self.peek(), [LOCAL_ID])
        done = self.inbox / (SID + ".done")
        target = self.home / "state-target"
        target.write_text("unchanged\n")
        target.chmod(0o600)
        done.symlink_to(target)
        self.assertEqual(self.peek(), [])
        self.assertEqual(target.read_text(), "unchanged\n")
        done.unlink()
        os.mkfifo(done, 0o600)
        self.assertEqual(self.peek(), [])
        done.unlink()
        (self.inbox / (SID + ".jsonl")).write_bytes(b"x" * (1024 * 1024 + 1))
        self.assertEqual(self.peek(), [])


class ChannelEOFRegression(unittest.TestCase):
    def test_eof_exits_with_stdout_full_and_undrained(self):
        output_read, output_write = os.pipe()
        child = subprocess.Popen([sys.executable, str(PLUGIN / "scripts/channel.py")],
                                 env=BASE_ENV, stdin=subprocess.PIPE, stdout=output_write, stderr=subprocess.PIPE)
        payload = (json.dumps({"jsonrpc": "2.0", "id": "x" * 40000, "method": "ping"}) + "\n").encode() * 2
        errors = []
        accepted = threading.Event()
        close_input = threading.Event()

        def write():
            try:
                child.stdin.write(payload)
                child.stdin.flush()
                accepted.set()
                close_input.wait(timeout=3)
                child.stdin.close()
            except Exception as error:
                errors.append(error)

        writer = threading.Thread(target=write, daemon=True)
        writer.start()
        try:
            self.assertTrue(accepted.wait(timeout=2), "channel stopped reading requests while stdout was full")
            deadline = time.monotonic() + 0.5
            while select.select([], [output_write], [], 0)[1] and time.monotonic() < deadline:
                time.sleep(0.005)
            self.assertEqual(select.select([], [output_write], [], 0)[1], [], "stdout pipe must be backpressured before EOF")
            close_input.set()
            writer.join(timeout=2)
            self.assertFalse(writer.is_alive(), "channel stopped reading requests while stdout was full")
            self.assertEqual(errors, [])
            self.assertEqual(child.wait(timeout=1), 0, "EOF must exit within one second without draining stdout")
            self.assertEqual(child.stderr.read(), b"")
        finally:
            close_input.set()
            if child.poll() is None:
                child.kill()
                child.wait()
            writer.join(timeout=2)
            child.stdin.close()
            os.close(output_write)
            os.close(output_read)
            child.stderr.close()


if __name__ == "__main__":
    unittest.main()
