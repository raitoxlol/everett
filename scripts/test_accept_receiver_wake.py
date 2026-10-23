import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from types import SimpleNamespace
from unittest import mock


SCRIPT = Path(__file__).with_name("accept_receiver_wake.py")
SPEC = importlib.util.spec_from_file_location("accept_receiver_wake", SCRIPT)
accept = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = accept
SPEC.loader.exec_module(accept)


class AcceptanceRunnerTests(unittest.TestCase):
    def setUp(self):
        accept.MANIFEST_OWNERS.clear()

    def test_sender_environment_clears_every_known_caller_binding(self):
        original = os.environ.copy()
        try:
            for name in accept.CALLER_BINDINGS:
                os.environ[name] = "inherited-session"
            os.environ["ACCEPTANCE_TEST_KEEP"] = "present"
            env = accept.local_env("/tmp/profile", sender=True)
            self.assertEqual(env["EVERETT_HOME"], "/tmp/profile")
            self.assertEqual(env["ACCEPTANCE_TEST_KEEP"], "present")
            self.assertTrue(all(name not in env for name in accept.CALLER_BINDINGS))
        finally:
            os.environ.clear()
            os.environ.update(original)

    def test_send_argv_is_one_inbox_request_and_has_no_wake_flag(self):
        argv = accept.send_argv("nuc", "38cab978-79c0-4f42-9aee-5c30d8aa6e4c", "marker", "key")
        self.assertEqual(argv[0], "send")
        self.assertEqual(argv[argv.index("--to") + 1], "nuc/claude/38cab978-79c0-4f42-9aee-5c30d8aa6e4c")
        self.assertEqual(argv[argv.index("--mode") + 1], "inbox")
        self.assertEqual(argv[argv.index("--wait") + 1], "0")
        self.assertNotIn("--wake", argv)

    def test_send_accepts_only_committed_delivery_states(self):
        for state in ("queued", "delivered", "claimed", "replied"):
            self.assertTrue(accept.committed_send_state(state))
        for state in (None, "pending", "failed", "rejected"):
            self.assertFalse(accept.committed_send_state(state))

    def test_reply_correlation_requires_original_id_exact_session_and_marker(self):
        messages = [
            {"reply_to": "original", "from": "nuc-device/claude/session-uuid", "from_harness": "claude", "relay_device": "nuc-device", "text": "EVERETT-RECEIVER-WAKE-ABC"},
            {"reply_to": "other", "from": "nuc-device/claude/session-uuid", "text": "EVERETT-RECEIVER-WAKE-ABC"},
            {"reply_to": "original", "from": "other/claude/session-uuid", "text": "EVERETT-RECEIVER-WAKE-ABC"},
            {"reply_to": "original", "from": "nuc-device/claude/session-uuid", "text": "different"},
        ]
        result = accept.matching_replies(messages, "original", "session-uuid", "nuc-device", "EVERETT-RECEIVER-WAKE-ABC")
        self.assertEqual(len(result), 1)
        self.assertIs(result[0], messages[0])
        messages.append({"reply_to": "original", "from": "nuc-device/claude/session-uuid", "from_harness": "claude", "relay_device": "nuc-device", "text": " EVERETT-RECEIVER-WAKE-ABC "})
        self.assertEqual(len(accept.matching_replies(messages, "original", "session-uuid", "nuc-device", "EVERETT-RECEIVER-WAKE-ABC")), 1)

    def test_manifest_creation_is_exclusive_private_and_updates_owned_file(self):
        with tempfile.TemporaryDirectory() as directory:
            os.chmod(directory, 0o700)
            path = Path(directory) / "run.json"
            manifest = accept.RunManifest("run", 1.0, "preflight", "/mac", "/nuc", "mac-id", "nuc-id", "mac-fp", "nuc-fp", "session", "/cwd")
            accept.save_manifest(path, manifest)
            first = path.stat()
            self.assertEqual(stat.S_IMODE(first.st_mode), 0o600)
            manifest.phase = "prepared"
            accept.save_manifest(path, manifest)
            self.assertEqual(json.loads(path.read_text())["phase"], "prepared")
            with self.assertRaises(accept.AcceptanceError):
                accept.MANIFEST_OWNERS.clear()
                accept.save_manifest(path, manifest)

    def test_policy_install_and_restore_preserve_original_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile" / ".everett" / "relay-cloudflare" / "receiver-wake.json"
            path.parent.mkdir(parents=True, mode=0o700)
            original = b'{"version":1,"enabled":false,"allow":[]}'
            path.write_bytes(original)
            os.chmod(path, 0o600)
            installed = json.dumps({"version": 1, "enabled": True, "allow": [accept.exact_policy_rule("session", "/cwd", "mac-device")]}, separators=(",", ":")).encode()
            original_b64 = accept.base64.b64encode(original).decode()
            installed_b64 = accept.base64.b64encode(installed).decode()
            installed_meta = json.loads(self.run_remote_helper("install-policy", path, original_b64, f"{path.stat().st_dev}:{path.stat().st_ino}", installed_b64))
            self.assertEqual(path.read_bytes(), installed)
            self.run_remote_helper("restore-policy", path, installed_b64, installed_meta["identity"], original_b64, "384")
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_policy_reader_rejects_unknown_disabled_policy_fields_before_emitting_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text('{"version":1,"enabled":false,"allow":[],"note":"unknown"}')
            path.chmod(0o600)
            result = self.run_remote_helper_raw("policy", path)
            self.assertEqual(result.returncode, 31)
            self.assertEqual(result.stdout, "")
            self.assertFalse(accept.is_disabled_policy(json.loads(path.read_text())))

    def test_cleanup_leaves_policy_alone_if_another_writer_changed_it(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            installed = b'{"version":1,"enabled":true,"allow":[]}'
            changed = b'{"version":1,"enabled":false,"allow":[]}'
            path.write_bytes(changed)
            identity = f"{path.stat().st_dev}:{path.stat().st_ino}"
            output = self.run_remote_helper("restore-policy", path, accept.base64.b64encode(installed).decode(), identity, "-", "384")
            self.assertEqual(output, "changed")
            self.assertEqual(path.read_bytes(), changed)

    def test_policy_rule_does_not_match_another_sender_or_session(self):
        allowed = accept.exact_policy_rule("session", "/cwd", "mac-device")
        self.assertEqual(allowed, {"session_id": "session", "cwd": "/cwd", "sender_device_id": "mac-device"})
        self.assertNotEqual(allowed, accept.exact_policy_rule("other-session", "/cwd", "mac-device"))
        self.assertNotEqual(allowed, accept.exact_policy_rule("session", "/cwd", "other-device"))

    def test_policy_ancestors_must_be_canonical_private_directories(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = Path(directory) / "profile"
            policy_parent = profile / ".everett" / "relay-cloudflare"
            policy_parent.mkdir(parents=True, mode=0o700)
            os.chmod(profile, 0o700)
            os.chmod(profile / ".everett", 0o700)
            os.chmod(policy_parent, 0o700)
            policy = policy_parent / "receiver-wake.json"
            canonical_profile = profile.resolve(strict=True)
            canonical_policy = canonical_profile / ".everett" / "relay-cloudflare" / "receiver-wake.json"
            self.run_remote_helper("validate-policy-ancestors", canonical_profile, canonical_policy)
            os.chmod(policy_parent, 0o755)
            result = self.run_remote_helper_raw("validate-policy-ancestors", canonical_profile, canonical_policy)
            self.assertEqual(result.returncode, 42)

    def test_project_digest_covers_index_tracked_and_untracked_state_but_not_git_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve(strict=True)
            root = base / "repo"
            outside = base / "outside"
            root.mkdir()
            outside.mkdir()
            subprocess.run(["git", "init", str(root)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["git", "-C", str(root), "config", "user.name", "Test"], check=True)
            subprocess.run(["git", "-C", str(root), "config", "user.email", "test@example.invalid"], check=True)
            tracked = root / "tracked.txt"
            tracked.write_text("initial")
            (root / ".gitignore").write_text("ignored.txt\n")
            subprocess.run(["git", "-C", str(root), "add", "tracked.txt", ".gitignore"], check=True)
            subprocess.run(["git", "-C", str(root), "commit", "-m", "baseline"], check=True, stdout=subprocess.DEVNULL)
            def digest():
                result = self.run_remote_helper_raw("cwd-digest", root)
                self.assertEqual(result.returncode, 0, result.stderr)
                return json.loads(result.stdout)["sha256"]
            baseline = digest()
            ignored = root / "ignored.txt"
            ignored.write_text("ignored baseline")
            ignored_digest = digest()
            self.assertNotEqual(baseline, ignored_digest)
            ignored.write_text("ignored changed")
            self.assertNotEqual(ignored_digest, digest())
            ignored.unlink()
            outside_file = outside / "secret.txt"
            outside_file.write_text("outside original")
            (root / "outside-link").symlink_to(outside)
            symlink_digest = digest()
            outside_file.write_text("outside changed")
            self.assertEqual(symlink_digest, digest())
            (root / "outside-link").unlink()
            (root / "untracked.txt").write_text("new")
            untracked = digest()
            self.assertNotEqual(baseline, untracked)
            (root / "untracked.txt").unlink()
            tracked.write_text("staged")
            subprocess.run(["git", "-C", str(root), "add", "tracked.txt"], check=True)
            staged = digest()
            self.assertNotEqual(baseline, staged)
            (root / ".git" / "acceptance-test-metadata").write_text("ignored")
            self.assertEqual(staged, digest())

    def test_remote_agents_uses_explicit_claude_binary_and_listener_path_preserves_system_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve(strict=True)
            binary = root / "bin" / "claude"
            binary.parent.mkdir()
            binary.write_text(f"#!{sys.executable}\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n")
            binary.chmod(0o700)
            invoked = self.run_remote_helper_raw("agents", binary, root)
            self.assertEqual(invoked.returncode, 0, invoked.stderr)
            self.assertEqual(json.loads(invoked.stdout), ["agents", "--json"])
            system_path = "/usr/bin:/bin"
            env = os.environ.copy()
            env["PATH"] = system_path
            listener_path = self.run_remote_helper_raw("listener-path", binary, env=env)
            self.assertEqual(listener_path.returncode, 0, listener_path.stderr)
            self.assertEqual(listener_path.stdout.strip(), str(binary.parent) + os.pathsep + system_path)

    @unittest.skipUnless(Path("/proc").is_dir() and shutil.which("cc"), "NUC listener ownership helper requires Linux /proc and a C compiler")
    def test_remote_listener_start_has_durable_owner_record_for_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            profile = root / "profile"
            profile.mkdir(mode=0o700)
            binary = root / "fake-everett"
            subprocess.run([shutil.which("cc"), "-x", "c", "-o", str(binary), "-"], input="#include <unistd.h>\nint main(void) { for (;;) sleep(1); }\n", check=True, text=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            binary.chmod(0o700)
            claude = root / "bin" / "claude"
            claude.parent.mkdir()
            claude.write_text("#!/bin/sh\nexit 0\n")
            claude.chmod(0o700)
            log = profile / ".everett" / "relay-cloudflare" / "receiver-wake" / "listener.log"
            env = os.environ.copy()
            env["EVERETT_HOME"] = str(profile)
            started = self.run_remote_helper_raw("start", binary, log, claude, env=env)
            self.assertEqual(started.returncode, 0, started.stderr)
            owner = json.loads(Path(str(log) + ".owner.json").read_text())
            self.assertEqual(owner["pid"], json.loads(started.stdout)["pid"])
            stopped = self.run_remote_helper_raw("stop-log", log, profile, env=env)
            self.assertEqual(stopped.returncode, 0, stopped.stderr)
            self.assertEqual(stopped.stdout.strip(), "stopped")

    def test_keyboard_interrupt_during_run_still_cleans_up_and_never_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            os.chmod(directory, 0o700)
            manifest_path = Path(directory) / "run.json"
            cleaned = []
            def preflight(run):
                run.manifest = accept.RunManifest("run", 1.0, "preflight", "/mac", "/nuc", "mac", "nuc", "mac-fp", "nuc-fp", "session", "/cwd")
                accept.save_manifest(manifest_path, run.manifest)
            def interrupt(_run):
                raise KeyboardInterrupt()
            def cleanup(run):
                cleaned.append(True)
                run.manifest.cleanup["attempted"] = True
                accept.save_manifest(manifest_path, run.manifest)
            argv = ["--nuc-ssh", "nucbox", "--mac-binary", sys.executable, "--nuc-binary", "/bin/true", "--nuc-claude-binary", "/home/nuc/.local/bin/claude", "--nuc-sha256", "0" * 64, "--source-commit", "a" * 40, "--mac-device-id", "mac", "--nuc-device-id", "nuc", "--mac-fingerprint", "mac-fp", "--nuc-fingerprint", "nuc-fp", "--session-id", "38cab978-79c0-4f42-9aee-5c30d8aa6e4c", "--denied-session-id", "89cab978-79c0-4f42-9aee-5c30d8aa6e4c", "--cwd", "/tmp", "--manifest", str(manifest_path)]
            stdout, stderr = StringIO(), StringIO()
            with mock.patch.object(accept.Acceptance, "preflight", preflight), mock.patch.object(accept.Acceptance, "prepare", interrupt), mock.patch.object(accept.Acceptance, "cleanup", cleanup), redirect_stdout(stdout), redirect_stderr(stderr):
                code = accept.main(argv)
            self.assertEqual(code, 1)
            self.assertTrue(cleaned)
            self.assertEqual(json.loads(stderr.getvalue())["status"], "interrupted")
            self.assertNotIn('"status": "passed"', stdout.getvalue())
            self.assertTrue(json.loads(manifest_path.read_text())["cleanup"]["attempted"])

    def test_cleanup_attempts_policy_restore_after_listener_stop_error(self):
        with tempfile.TemporaryDirectory() as directory:
            os.chmod(directory, 0o700)
            manifest_path = Path(directory) / "run.json"
            args = SimpleNamespace(manifest=str(manifest_path), nuc_profile="/nuc-profile")
            runner = accept.Acceptance(args)
            runner.manifest = accept.RunManifest("run", 1.0, "prepare", "/mac", "/nuc", "mac", "nuc", "mac-fp", "nuc-fp", "session", "/cwd")
            runner.manifest.policy_owned = True
            runner.manifest.policy_installed_b64 = accept.base64.b64encode(b"installed").decode()
            runner.manifest.policy_installed_identity = "1:2"
            runner.manifest.listeners.append({"host": "nuc", "pid": 12, "log": "/log"})
            runner.listeners.append(accept.Listener("nuc", 12, 1.0))
            accept.save_manifest(manifest_path, runner.manifest)
            runner.helper = mock.Mock(side_effect=[accept.AcceptanceError("stop unavailable"), "restored"])
            runner.cleanup()
            self.assertEqual(runner.manifest.cleanup["policy"], "restored")
            self.assertEqual(runner.helper.call_count, 2)

    @staticmethod
    def run_remote_helper(operation, *args):
        result = AcceptanceRunnerTests.run_remote_helper_raw(operation, *args)
        if result.returncode:
            raise AssertionError(f"remote helper {operation} failed with {result.returncode}")
        return result.stdout.strip()

    @staticmethod
    def run_remote_helper_raw(operation, *args, env=None):
        return subprocess.run([sys.executable, "-c", accept.REMOTE_HELPER, operation, *map(str, args)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=8)


if __name__ == "__main__":
    unittest.main()
