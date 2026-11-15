import contextlib
import importlib.util
import io
import json
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
