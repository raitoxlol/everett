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
        self.codex = self.root / 'codex home'
        self.codex.mkdir()
        self.env = patch.dict(os.environ, {'CODEX_HOME': str(self.codex)}, clear=False)
        self.env.start()
        self.addCleanup(self.env.stop)
        os.environ.pop('EVERETT_BIN', None)

    def install(self, register=True, **kwargs):
        installed = installer.install(self.destination, self.binary, **kwargs)
        if register:
            self.register()
        return installed

    def register(self, source=None, enabled=True):
        """Write what `codex plugin marketplace add` and `codex plugin add` record."""
        source = str(self.destination) if source is None else source
        (self.codex / 'config.toml').write_text(
            'model = "gpt"\n\n'
            '[marketplaces.other]\nsource_type = "local"\nsource = "/elsewhere"\n\n'
            f'[marketplaces.everett-direct-local]\nsource_type = "local"\nsource = {json.dumps(source)}\n\n'
            f'[plugins."everett-direct@everett-direct-local"]\nenabled = {"true" if enabled else "false"}\n')

    def plugin_dir(self):
        return Path(self.install()['plugin'])

    def result(self, plugin):
        return checker.check(plugin)

    def statuses(self, plugin):
        result = self.result(plugin)
        return result['ready'], {row['name']: row['ok'] for row in result['checks']}

    def row(self, plugin, name):
        return next(row for row in self.result(plugin)['checks'] if row['name'] == name)

    def mcp(self, plugin, mutate):
        path = plugin / '.mcp.json'
        data = json.loads(path.read_text())
        mutate(data)
        path.write_text(json.dumps(data))

    def run_installed(self, plugin, *args):
        """The real user path: python3 <installed>/check.py with no arguments."""
        return subprocess.run([sys.executable, str(plugin / 'check.py'), *args],
                              capture_output=True, text=True, timeout=15)

    def test_fresh_registered_install_passes_every_check(self):
        ready, statuses = self.statuses(self.plugin_dir())
        self.assertTrue(ready, statuses)
        self.assertTrue(all(statuses.values()), statuses)

    def test_fallback_toml_scan_matches_tomllib(self):
        plugin = self.plugin_dir()
        with patch.object(checker, 'tomllib', None):
            ready, statuses = self.statuses(plugin)
            self.assertTrue(ready, statuses)
            self.register(enabled=False)
            ready, statuses = self.statuses(plugin)
            self.assertFalse(ready)
            self.assertFalse(statuses['codex_plugin'])
            # A following [[array.table]] must not leak its keys into the plugin table.
            config = self.codex / 'config.toml'
            config.write_text(config.read_text() + '\n[[skills.config]]\npath = "x"\nenabled = true\n')
            ready, statuses = self.statuses(plugin)
            self.assertFalse(ready)
            self.assertFalse(statuses['codex_plugin'])

    def test_install_without_codex_registration_is_not_ready(self):
        plugin = Path(self.install(register=False)['plugin'])
        result = self.result(plugin)
        self.assertFalse(result['ready'])
        rows = {row['name']: row for row in result['checks']}
        self.assertTrue(rows['marketplace']['ok'])
        self.assertFalse(rows['codex_marketplace']['ok'])
        self.assertIn(f'codex plugin marketplace add "{self.destination.resolve()}" --json', rows['codex_marketplace']['remedy'])
        self.assertFalse(rows['codex_plugin']['ok'])
        self.assertIn('codex plugin add everett-direct@everett-direct-local --json', rows['codex_plugin']['remedy'])

    def test_marketplace_registered_at_another_destination_fails(self):
        plugin = self.plugin_dir()
        self.register(source=str(self.root / 'old install'))
        ready, statuses = self.statuses(plugin)
        self.assertFalse(ready)
        self.assertFalse(statuses['codex_marketplace'])
        self.assertTrue(statuses['codex_plugin'])

    def test_disabled_plugin_fails(self):
        plugin = self.plugin_dir()
        self.register(enabled=False)
        row = self.row(plugin, 'codex_plugin')
        self.assertFalse(row['ok'])
        self.assertIn('disabled', row['detail'])

    def test_corrupt_codex_config_reports_remedy(self):
        plugin = self.plugin_dir()
        (self.codex / 'config.toml').write_text('[marketplaces\n')
        ready, statuses = self.statuses(plugin)
        self.assertFalse(ready)
        self.assertFalse(statuses['codex_marketplace'])

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

    def test_cached_root_plugin_json_warns_without_failing(self):
        plugin = self.plugin_dir()
        cached = self.codex / 'plugins/cache/everett-direct-local/everett-direct/0.3.1'
        cached.mkdir(parents=True)
        (cached / 'plugin.json').write_text('{}')
        result = self.result(plugin)
        self.assertTrue(result['ready'])
        self.assertEqual([w['name'] for w in result['warnings']], ['cached_portable_manifest'])

    def test_invalid_runtime_configuration_fails_with_remedy(self):
        plugin = self.plugin_dir()
        (self.destination / 'runtime.json').write_text(json.dumps({'binary': '/nonexistent/everett'}))
        row = self.row(plugin, 'runtime')
        self.assertFalse(row['ok'])
        self.assertIn('remedy', row)

    def test_missing_generated_marketplace_file_fails_with_reinstall_remedy(self):
        plugin = self.plugin_dir()
        (self.destination / '.agents/plugins/marketplace.json').unlink()
        row = self.row(plugin, 'marketplace')
        self.assertFalse(row['ok'])
        remedy = row['remedy']
        self.assertIn(f'install.py --binary /absolute/path/to/everett --destination "{self.destination.resolve()}"', remedy)
        self.assertLess(remedy.index('install.py'), remedy.index('codex plugin marketplace add'))

    def test_wrong_json_types_report_remedies_not_tracebacks(self):
        cases = {
            'mcpServers list': ('.mcp.json', {'mcpServers': ['everett_local']}, 'mcp_manifest'),
            'non-string config': (None, lambda d: d['mcpServers']['everett_local']['args'].__setitem__(2, 7),
                                  'mcp_manifest'),
            'non-string launcher': (None, lambda d: d['mcpServers']['everett_local']['args'].__setitem__(0, ['x']),
                                    'mcp_manifest'),
            'non-list args': (None, lambda d: d['mcpServers']['everett_local'].__setitem__('args', 'launch.py'),
                              'mcp_manifest'),
            'marketplace source string': ('../../.agents/plugins/marketplace.json',
                                          {'plugins': [{'source': './plugins/everett-direct'}]}, 'marketplace'),
            'marketplace plugins dict': ('../../.agents/plugins/marketplace.json', {'plugins': {}}, 'marketplace'),
            'top-level list': ('.mcp.json', [], 'mcp_manifest'),
        }
        for label, (relative, change, name) in cases.items():
            with self.subTest(label):
                plugin = Path(self.install()['plugin'])
                if relative is None:
                    self.mcp(plugin, change)
                else:
                    (plugin / relative).write_text(json.dumps(change))
                run = self.run_installed(plugin)
                self.assertEqual(run.returncode, 1, run.stdout)
                self.assertNotIn('Traceback', run.stderr)
                rows = {row['name']: row for row in json.loads(run.stdout)['checks']}
                self.assertFalse(rows[name]['ok'])
                self.assertIn('remedy', rows[name])

    def test_mcp_manifest_requires_this_plugins_launcher(self):
        plugin = self.plugin_dir()
        foreign = self.root / 'other/scripts/launch.py'
        foreign.parent.mkdir(parents=True)
        foreign.write_text((plugin / 'scripts/launch.py').read_text())
        self.mcp(plugin, lambda d: d['mcpServers']['everett_local']['args'].__setitem__(0, str(foreign)))
        row = self.row(plugin, 'mcp_manifest')
        self.assertFalse(row['ok'])
        self.assertIn('launch.py', row['detail'])

    def test_mcp_manifest_requires_executable_command(self):
        plugin = self.plugin_dir()
        program = self.root / 'python-not-executable'
        program.write_text('')
        program.chmod(0o600)
        self.mcp(plugin, lambda d: d['mcpServers']['everett_local'].__setitem__('command', str(program)))
        row = self.row(plugin, 'mcp_manifest')
        self.assertFalse(row['ok'])
        self.assertIn('executable', row['detail'])

    def test_mcp_manifest_requires_python_command(self):
        """An executable that is not Python (like /bin/true) must fail."""
        plugin = self.plugin_dir()
        program = self.root / 'not-python'
        program.write_text('#!/bin/sh\nexit 0\n')
        program.chmod(0o700)
        self.mcp(plugin, lambda d: d['mcpServers']['everett_local'].__setitem__('command', str(program)))
        row = self.row(plugin, 'mcp_manifest')
        self.assertFalse(row['ok'])
        self.assertIn('Python', row['detail'])

    def test_runtime_runs_the_manifest_command(self):
        """runtime goes through .mcp.json's command, not an in-process import."""
        plugin = self.plugin_dir()
        program = self.root / 'not-python'
        program.write_text('#!/bin/sh\nexit 0\n')
        program.chmod(0o700)
        self.mcp(plugin, lambda d: d['mcpServers']['everett_local'].__setitem__('command', str(program)))
        row = self.row(plugin, 'runtime')
        self.assertFalse(row['ok'])

    def test_binary_failing_version_check_reports_remedy_not_traceback(self):
        plugin = self.plugin_dir()
        self.binary.write_text('#!/bin/sh\nexit 1\n')
        run = self.run_installed(plugin)
        self.assertEqual(run.returncode, 1)
        self.assertNotIn('Traceback', run.stderr)
        runtime = next(row for row in json.loads(run.stdout)['checks'] if row['name'] == 'runtime')
        self.assertFalse(runtime['ok'])
        self.assertIn('remedy', runtime)

    def test_installed_copy_with_no_arguments_reports_ready_and_exit_codes(self):
        plugin = self.plugin_dir()
        run = self.run_installed(plugin)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertTrue(json.loads(run.stdout)['ready'])
        self.assertEqual(json.loads(run.stdout)['plugin'], str(plugin.resolve()))
        (plugin / 'plugin.json').write_text('{}')
        run = self.run_installed(plugin)
        self.assertEqual(run.returncode, 1)
        self.assertFalse(json.loads(run.stdout)['ready'])

    def test_source_check_accepts_plugin_flag(self):
        plugin = self.plugin_dir()
        run = subprocess.run([sys.executable, str(ROOT / 'check.py'), '--plugin', str(plugin)],
                             capture_output=True, text=True, timeout=15)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


class InstallOutputTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='everett-plugin-install-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary = self.root / 'everett'
        self.binary.write_text('#!/bin/sh\necho "everett 2.2.1"\n')
        self.binary.chmod(0o700)
        self.env = patch.dict(os.environ, {}, clear=False)
        self.env.start()
        self.addCleanup(self.env.stop)
        os.environ.pop('EVERETT_BIN', None)

    def test_next_prints_runnable_commands_for_custom_destination(self):
        destination = self.root / 'custom place'
        installed = installer.install(destination, self.binary)
        check = destination.resolve() / 'plugins/everett-direct/check.py'
        self.assertEqual(installed['commands'], [
            f"codex plugin marketplace add '{destination.resolve()}' --json",
            'codex plugin add everett-direct@everett-direct-local --json',
            f"python3 '{check}'",
        ])
        nxt = installed['next']
        self.assertLess(nxt.index('codex plugin marketplace add'), nxt.index('quit and reopen'))
        self.assertLess(nxt.index('codex plugin add'), nxt.index('quit and reopen'))
        self.assertIn(f"python3 '{check}'", nxt)
        # The printed check command runs as written (check.py is copied 0644).
        run = subprocess.run(['python3', str(check)], capture_output=True, text=True, timeout=15,
                             env={**os.environ, 'CODEX_HOME': str(self.root / 'empty codex')})
        self.assertNotIn('Traceback', run.stderr)
        self.assertIn('"checks"', run.stdout)


if __name__ == '__main__':
    unittest.main()
