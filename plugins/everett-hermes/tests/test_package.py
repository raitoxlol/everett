import json
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT.parent) not in sys.path:
    sys.path.insert(0, str(ROOT.parent))


def _yaml_list(text: str, key: str) -> list[str]:
    lines = text.splitlines()
    items: list[str] = []
    collecting = False
    for line in lines:
        if re.match(rf"^{re.escape(key)}:\s*$", line):
            collecting = True
            continue
        if collecting:
            if re.match(r"^\s+-\s+", line):
                items.append(re.sub(r"^\s+-\s+", "", line).strip())
            elif line.strip() and not line.startswith(" "):
                break
    return items


class PackageTests(unittest.TestCase):
    def test_native_manifest_declares_tools_hooks_and_schema(self):
        text = (ROOT / "plugin.yaml").read_text(encoding="utf-8")
        self.assertIn("manifest_version: 2", text)
        self.assertIn("name: everett-hermes", text)
        self.assertIn("version: 0.2.0", text)
        self.assertIn("kind: standalone", text)
        self.assertIn("config_schema:", text)

        declared_tools = _yaml_list(text, "provides_tools")
        declared_hooks = _yaml_list(text, "provides_hooks")
        self.assertEqual(
            declared_hooks,
            [
                "on_session_start",
                "on_session_end",
                "on_session_finalize",
                "on_session_reset",
                "on_session_delete",
                "pre_llm_call",
                "transform_tool_result",
                "pre_verify",
            ],
        )
        self.assertIn("everett_whoami", declared_tools)
        self.assertIn("everett_ls", declared_tools)
        self.assertIn("everett_send", declared_tools)
        self.assertIn("everett_inbox", declared_tools)
        self.assertIn("everett_card", declared_tools)
        self.assertIn("everett_core", declared_tools)
        self.assertEqual(len(declared_tools), len(set(declared_tools)))

        for module in ("__init__.py", "_everett.py", "_inbox.py", "_wake.py", "_commands.py"):
            self.assertTrue((ROOT / module).is_file(), f"missing {module}")

    def test_portable_fallback_manifest_matches_version_and_omits_identity_override(self):
        plugin = json.loads((ROOT / "plugin.json").read_text(encoding="utf-8"))
        self.assertEqual(plugin["name"], "everett-hermes")
        self.assertEqual(plugin["version"], "0.2.0")

        mcp = json.loads((ROOT / "mcp.json").read_text(encoding="utf-8"))
        server = mcp["mcpServers"]["everett_hermes"]
        self.assertEqual(server["type"], "stdio")
        self.assertEqual(server["command"], "everett")
        self.assertEqual(server["args"], ["mcp", "--harness", "hermes"])
        self.assertNotIn("env", server)

    def test_bundled_skill_has_frontmatter_without_env_dump_workaround(self):
        skill = (ROOT / "skills" / "everett-hermes" / "SKILL.md").read_text(encoding="utf-8")
        self.assertTrue(skill.startswith("---\n"))
        self.assertIn("name: everett-hermes", skill)
        self.assertNotIn("printenv", skill)


if __name__ == "__main__":
    unittest.main()
