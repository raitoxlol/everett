import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('installer', ROOT / 'install.py')
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)
SPEC = importlib.util.spec_from_file_location('checker', ROOT / 'check.py')
checker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checker)


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='everett-plugin-check-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / 'runtime with spaces'
        self.binary.write_text('#!/bin/sh\nif [ "$1" = "--version" ]; then echo "everett 2.2.1"; fi\n')
        self.binary.chmod(0o700)
        self.destination = self.root / 'installation with spaces'

    def install(self, **kwargs):
        return installer.install(self.destination, self.binary, **kwargs)

    def plugin_dir(self):
        return Path(self.install()['plugin'])

    def statuses(self, plugin):
        result = checker.check(plugin)
        return result['ready'], {row['name']: row['ok'] for row in result['checks']}

    def test_fresh_install_passes_every_check(self):
        ready, statuses = self.statuses(self.plugin_dir())
        self.assertTrue(ready)
        self.assertTrue(all(statuses.values()), statuses)

    def test_portable_manifests_fail_for_chatgpt_desktop(self):
        installed = self.install(portable=True)
        plugin = Path(installed['plugin'])
        self.assertTrue((plugin / 'plugin.json').is_file())
        ready, statuses = self.statuses(plugin)
        self.assertFalse(ready)
        self.assertFalse(statuses['no_portable_manifests'])

    def test_stale_portable_file_from_030_install_fails(self):
        plugin = self.plugin_dir()
        (plugin / 'plugin.json').write_text('{}')
        ready, statuses = self.statuses(plugin)
        self.assertFalse(ready)
        self.assertFalse(statuses['no_portable_manifests'])

    def test_invalid_runtime_configuration_fails_with_remedy(self):
        plugin = self.plugin_dir()
        (self.destination / 'runtime.json').write_text(json.dumps({'binary': '/nonexistent/everett'}))
        result = checker.check(plugin)
        self.assertFalse(result['ready'])
        runtime = next(row for row in result['checks'] if row['name'] == 'runtime')
        self.assertFalse(runtime['ok'])
        self.assertIn('remedy', runtime)

    def test_missing_marketplace_registration_fails(self):
        plugin = self.plugin_dir()
        (self.destination / '.agents/plugins/marketplace.json').unlink()
        ready, statuses = self.statuses(plugin)
        self.assertFalse(ready)
        self.assertFalse(statuses['marketplace'])

    def test_cli_reports_ready_and_exit_codes(self):
        plugin = self.plugin_dir()
        run = subprocess.run([sys.executable, str(ROOT / 'check.py'), '--plugin', str(plugin)],
                             capture_output=True, text=True, timeout=15)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertTrue(json.loads(run.stdout)['ready'])
        (plugin / 'plugin.json').write_text('{}')
        run = subprocess.run([sys.executable, str(ROOT / 'check.py'), '--plugin', str(plugin)],
                             capture_output=True, text=True, timeout=15)
        self.assertEqual(run.returncode, 1)
        self.assertFalse(json.loads(run.stdout)['ready'])


if __name__ == '__main__':
    unittest.main()
