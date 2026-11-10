import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('installer', ROOT / 'install.py')
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='everett-plugin-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / 'runtime with spaces'
        self.binary.write_text('#!/bin/sh\nif [ "$1" = "--version" ]; then echo "everett 2.2.1"; else printf "mcp-started:%s" "$1"; fi\n')
        self.binary.chmod(0o700)
        self.env = patch.dict(os.environ, {}, clear=False)
        self.env.start()
        self.addCleanup(self.env.stop)
        os.environ.pop('EVERETT_BIN', None)

    def install(self):
        return installer.install(self.root / 'installation with spaces', self.binary)

    def command(self, installed):
        manifest = json.loads((Path(installed['plugin']) / '.mcp.json').read_text())
        c = manifest['mcpServers']['everett_local']
        return [c['command'], *c['args']]

    def test_install_records_binary_flag_not_everett_bin_override(self):
        override = self.root / 'override'
        override.write_text('#!/bin/sh\necho "everett 9.9.9"\n')
        override.chmod(0o700)
        os.environ['EVERETT_BIN'] = str(override)
        installed = self.install()
        recorded = json.loads((self.root / 'installation with spaces/runtime.json').read_text())['binary']
        self.assertEqual(recorded, str(self.binary.absolute()))
        self.assertEqual(installed['runtime'], str(self.binary.absolute()))
        self.assertEqual(installed['version'], 'everett 2.2.1')

    def test_installed_command_executes_runtime_with_spaces(self):
        installed = self.install()
        result = subprocess.run(self.command(installed), capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, 'mcp-started:mcp')

    def test_missing_override_fails_without_falling_back(self):
        installed = self.install()
        env = dict(os.environ, EVERETT_BIN=str(self.root / 'missing'))
        result = subprocess.run(self.command(installed), env=env, capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, '')
        self.assertIn('absolute executable path', result.stderr)

    def test_old_runtime_cannot_replace_working_install(self):
        installed = self.install()
        config = Path(installed['marketplace']) / 'runtime.json'
        before = config.read_bytes()
        old = self.root / 'old'
        old.write_text('#!/bin/sh\necho "everett 2.1.0"\n')
        old.chmod(0o700)
        with self.assertRaisesRegex(ValueError, '2.2.1'):
            installer.install(Path(installed['marketplace']), old)
        self.assertEqual(before, config.read_bytes())

    def test_doctor_does_not_start_mcp(self):
        installed = self.install()
        result = subprocess.run([*self.command(installed), '--doctor'], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['transport'], 'stdio')
        self.assertFalse(json.loads(result.stdout)['browser_oauth'])

    def test_default_install_has_no_root_manifests_and_removes_stale_ones(self):
        plugin = Path(self.install()['plugin'])
        for name in ('plugin.json', 'mcp.json'):
            (plugin / name).write_text('{}')  # as a 0.3.0 install left them
        installed = self.install()
        self.assertFalse(installed['portable'])
        for name in ('plugin.json', 'mcp.json'):
            self.assertFalse((plugin / name).exists(), name)
        self.assertEqual(json.loads((plugin / '.codex-plugin/plugin.json').read_text())['mcpServers'], './.mcp.json')

    def test_portable_package_launches_the_same_runtime(self):
        installed = installer.install(self.root / 'installation with spaces', self.binary, portable=True)
        plugin = Path(installed['plugin'])
        manifest = json.loads((plugin / 'plugin.json').read_text())
        self.assertTrue(manifest['extensions']['com.openai']['requires_local_executor'])
        self.assertEqual(manifest['version'], json.loads((plugin / '.codex-plugin/plugin.json').read_text())['version'])
        server = json.loads((plugin / 'mcp.json').read_text())['mcpServers']['everett_local']
        self.assertEqual(server['type'], 'stdio')
        result = subprocess.run([server['command'], *server['args']], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, 'mcp-started:mcp')

if __name__ == '__main__':
    unittest.main()
