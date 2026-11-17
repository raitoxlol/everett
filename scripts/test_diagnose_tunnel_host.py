import contextlib
import copy
import datetime
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("diagnose", Path(__file__).with_name("diagnose-tunnel-host.py"))
diagnose = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnose)


class HostEvidenceTests(unittest.TestCase):
    def run_probe(self, capture):
        stream = io.StringIO()
        with patch.object(diagnose, "capture", side_effect=capture), patch.object(diagnose.sys, "platform", "darwin"), patch.object(diagnose.sys, "argv", ["probe"]), patch.object(diagnose.shutil, "which", return_value=None), patch.object(diagnose, "artifact", side_effect=lambda p: {"path": str(p)}), contextlib.redirect_stdout(stream):
            diagnose.main()
        return stream.getvalue()

    def test_process_and_service_evidence_never_echoes_arguments_or_config(self):
        secret = "synthetic-do-not-print"
        def capture(argv):
            if argv[:2] == ["ps", "-axo"]:
                return "12 1 /usr/local/bin/everett\n13 1 /usr/local/bin/tunnel-client\n99 1 unrelated\n"
            if argv[0] == "launchctl":
                return "13 0 com.everett.tunnel\n12 0 com.everett.gateway\n"
            return f"everett gateway --config /private/{secret} --bearer {secret}"
        output = self.run_probe(capture)
        self.assertNotIn(secret, output)
        result = json.loads(output)
        self.assertEqual(len(result["processes"]), 2)
        self.assertTrue(result["processes"][0]["explicit_config"])
        self.assertEqual(result["services"][0]["label"], "com.everett.tunnel")
        self.assertEqual(result["forwarding_health"], "unconfirmed")

    def test_unavailable_inspection_never_implies_an_absent_or_healthy_service(self):
        result = json.loads(self.run_probe(lambda argv: None))
        self.assertEqual(result["process_inventory"], "unavailable")
        self.assertEqual(result["service_inventory"], "unavailable")
        self.assertEqual(result["production_revision"], "unconfirmed")

    def test_versioned_serving_artifacts_are_included_without_exposing_arguments(self):
        gateway = "/Users/owner/.local/share/everett-runtime/everett-identities-253349b0e855"
        secret = "synthetic-runtime-secret"
        def capture(argv):
            if argv[:2] == ["ps", "-axo"]:
                return f"862 1 {gateway}\n97949 1 {gateway}\n99 1 /bin/not-everett\n"
            if argv[0] == "launchctl":
                return "862 0 tech.raitox.everett.gateway\n97949 0 tech.raitox.everett.tunnel\n"
            role = "gateway" if argv[2] == "862" else "tunnel watch"
            return f"{gateway} {role} --config /private/{secret} --bearer {secret}"
        output = self.run_probe(capture)
        self.assertNotIn(secret, output)
        result = json.loads(output)
        self.assertEqual([p["pid"] for p in result["processes"]], [862, 97949])
        self.assertEqual(result["processes"][0]["roles"], ["gateway"])
        self.assertEqual(result["processes"][1]["roles"], ["tunnel", "watch"])
        self.assertEqual(result["processes"][0]["executable"]["path"], gateway)
        self.assertEqual(result["production_revision"], "unconfirmed")


class PollHealthTests(unittest.TestCase):
    def snapshot(self, stamp="2026-11-17T07:26:41.6Z"):
        return {"live": True, "ready": True, "runtime": {"instance_id": "owned-instance"},
                "components": {"control-plane": {"status": "ok", "details": {
                    "last_success": stamp, "deadline_seconds": 35, "current_poll_age_seconds": 3}}}}

    def test_all_fraction_lengths_and_offsets_describe_fresh_polling(self):
        now = 1794900402
        for fraction in [""] + ["." + "123456789"[:n] for n in range(1, 10)]:
            for base, offset in [("2026-11-17T07:26:41", "Z"), ("2026-11-17T16:26:41", "+09:00")]:
                with self.subTest(fraction=fraction, offset=offset):
                    result = diagnose.poll_health(self.snapshot(base + fraction + offset), "owned-instance", now)
                    self.assertTrue(result["polling_fresh"], result)
                    self.assertEqual(result["request_completion"], "unconfirmed")
                    self.assertGreaterEqual(result["poll_success_age_seconds"], 0)
                    self.assertLessEqual(result["poll_success_age_seconds"], 1)

    def test_invalid_dates_are_errors_and_missing_success_is_distinct(self):
        for stamp in [None, "2026-11-17T07:26:41", "2026-13-17T07:26:41Z", "2026-11-17T07:26:41.1234567890Z", "2026-11-17T07:26:41+00:60", "not-a-time"]:
            with self.subTest(stamp=stamp):
                result = diagnose.poll_health(self.snapshot(stamp), "owned-instance", 1794900402)
                self.assertFalse(result["polling_fresh"])
                self.assertEqual(result["reason"], "invalid_last_success")
        snapshot = self.snapshot()
        del snapshot["components"]["control-plane"]["details"]["last_success"]
        self.assertEqual(diagnose.poll_health(snapshot, "owned-instance")["reason"], "no_successful_poll")

    def test_stale_foreign_unready_and_late_poll_are_not_healthy(self):
        baseline = self.snapshot()
        cases = [(["runtime", "instance_id"], "foreign", "instance_mismatch"),
                 (["ready"], False, "not_ready"),
                 (["components", "control-plane", "status"], "unknown", "polling_unconfirmed"),
                 (["components", "control-plane", "details", "current_poll_age_seconds"], 41, "poll_deadline_exceeded")]
        for path, value, reason in cases:
            snapshot = copy.deepcopy(baseline)
            current = snapshot
            for key in path[:-1]:
                current = current[key]
            current[path[-1]] = value
            result = diagnose.poll_health(snapshot, "owned-instance", 1794900402)
            self.assertFalse(result["polling_fresh"])
            self.assertEqual(result["reason"], reason)
        for now in [1794900480, 1794900390]:
            result = diagnose.poll_health(baseline, "owned-instance", now)
            self.assertFalse(result["polling_fresh"])
            self.assertEqual(result["reason"], "stale_successful_poll")

    def test_malformed_health_and_nonfinite_deadlines_fail_closed(self):
        for value in [None, [], {}, {"runtime": [], "components": {}}]:
            self.assertFalse(diagnose.poll_health(value, "owned-instance")["polling_fresh"])
        for field in ["deadline_seconds", "current_poll_age_seconds"]:
            for value in [None, True, "35", float("nan"), float("inf"), -1, 10**400]:
                snapshot = self.snapshot()
                snapshot["components"]["control-plane"]["details"][field] = value
                self.assertFalse(diagnose.poll_health(snapshot, "owned-instance", 1794900402)["polling_fresh"])

    def test_stdin_reports_errors_without_echoing_input_or_claiming_completion(self):
        secret = "synthetic-private-input"
        for raw in [secret, json.dumps(self.snapshot(secret)), "x" * 65537]:
            stream = io.StringIO()
            with patch.object(diagnose.sys, "stdin", io.TextIOWrapper(io.BytesIO(raw.encode()))), contextlib.redirect_stdout(stream):
                code = diagnose.poll_health_stdin("owned-instance")
            self.assertEqual(code, 1)
            self.assertNotIn(secret, stream.getvalue())
            result = json.loads(stream.getvalue())
            self.assertFalse(result["polling_fresh"])
            self.assertEqual(result["request_completion"], "unconfirmed")

    def test_stdin_success_returns_zero_and_still_does_not_establish_delivery(self):
        stream = io.StringIO()
        with patch.object(diagnose.sys, "stdin", io.TextIOWrapper(io.BytesIO(json.dumps(self.snapshot()).encode()))), patch.object(diagnose.time, "time", return_value=1794900402), contextlib.redirect_stdout(stream):
            self.assertEqual(diagnose.poll_health_stdin("owned-instance"), 0)
        result = json.loads(stream.getvalue())
        self.assertTrue(result["polling_fresh"])
        self.assertEqual(result["request_completion"], "unconfirmed")

    def test_cli_reads_only_supplied_snapshot_and_reports_exit_status(self):
        stamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
        for raw, code in [(json.dumps(self.snapshot(stamp)), 0), ("malformed", 1)]:
            process = subprocess.run([sys.executable, spec.origin, "--poll-health", "owned-instance"],
                                     input=raw, text=True, capture_output=True, timeout=5)
            self.assertEqual(process.returncode, code, process.stderr)
            result = json.loads(process.stdout)
            self.assertEqual(result["polling_fresh"], code == 0)
            self.assertEqual(result["request_completion"], "unconfirmed")

if __name__ == "__main__":
    unittest.main()
