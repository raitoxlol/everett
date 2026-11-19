import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class PackageTests(unittest.TestCase):
    def test_server_uses_everett_hermes_command_without_identity_override(self):
        mcp = json.loads((ROOT / "mcp.json").read_text(encoding="utf-8"))
        server = mcp["mcpServers"]["everett_hermes"]

        self.assertEqual(server["type"], "stdio")
        self.assertEqual(server["command"], "everett")
        self.assertEqual(server["args"], ["mcp", "--harness", "hermes"])
        self.assertNotIn("env", server)


if __name__ == "__main__":
    unittest.main()
