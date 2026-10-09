import sandbox  # noqa: F401

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

from everett import doctor, install, onboard
from everett.cli import main


class DevinMCP(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        env = mock.patch.dict(os.environ, {
            'HOME': tmp.name, 'EVERETT_HOME': tmp.name,
            'DEVIN_HOME': str(self.root / 'session-data'),
            'APPDATA': str(self.root / 'appdata'),
        })
        env.start()
        self.addCleanup(env.stop)
        launch = mock.patch.object(install, 'mcp_launch', return_value=(
            sys.executable, ['-m', 'everett', 'mcp'], {}))
        launch.start()
        self.addCleanup(launch.stop)
        which = mock.patch('shutil.which', return_value=None)
        which.start()
        self.addCleanup(which.stop)
        self.path = install.mcp_path('devin')

    def write_config(self, data):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(json.dumps(data), encoding='utf-8')

    def test_config_path_is_independent_of_session_data(self):
        with mock.patch.object(sys, 'platform', 'linux'):
            self.assertEqual(install.mcp_path('devin'), self.root / '.config/devin/mcp_config.json')
        with mock.patch.object(sys, 'platform', 'darwin'):
            self.assertEqual(install.mcp_path('devin'), self.root / '.config/devin/mcp_config.json')
        with mock.patch.object(sys, 'platform', 'win32'):
            self.assertEqual(install.mcp_path('devin'), self.root / 'appdata/devin/mcp_config.json')
            with mock.patch.dict(os.environ, {'APPDATA': ''}):
                self.assertEqual(install.mcp_path('devin'), self.root / 'AppData/Roaming/devin/mcp_config.json')

    def test_explicit_dry_run_and_hooks_exclusion(self):
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(main(['install-mcp', '--devin']), 0)
        self.assertIn(str(self.path), out.getvalue())
        self.assertIn('v3000.3+', out.getvalue())
        self.assertIn('"mcpServers"', out.getvalue())
        self.assertIn('## devin', out.getvalue())
        self.assertNotIn('## claude', out.getvalue())
        self.assertFalse(self.path.exists())
        with redirect_stderr(io.StringIO()):
            self.assertEqual(main(['install-mcp', '--devin', '--repair']), 2)
        self.assertFalse(self.path.exists())
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
            main(['install-hooks', '--devin'])
        self.assertEqual(error.exception.code, 2)

    def test_default_dry_run_detects_config_directory(self):
        self.path.parent.mkdir(parents=True)
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(main(['install-mcp']), 0)
        self.assertIn('## devin', out.getvalue())
        with redirect_stdout(io.StringIO()):
            self.assertEqual(main(['install-hooks']), 0)
        self.assertFalse(self.path.exists())

    def test_apply_preserves_siblings_and_is_idempotent(self):
        original = {'mcpServers': {'other': {'url': 'https://example.invalid/mcp'}}, 'custom': 1}
        self.write_config(original)
        before = self.path.read_bytes()
        with redirect_stdout(io.StringIO()):
            self.assertEqual(main(['install-mcp', '--devin', '--apply']), 0)
        data = json.loads(self.path.read_text())
        self.assertEqual(data['custom'], 1)
        self.assertEqual(data['mcpServers']['other'], original['mcpServers']['other'])
        self.assertEqual(data['mcpServers']['everett']['command'], sys.executable)
        self.assertEqual(data['mcpServers']['everett']['args'], ['-m', 'everett', 'mcp'])
        self.assertNotIn('type', data['mcpServers']['everett'])
        backups = list(self.path.parent.glob('mcp_config.json.everett-bak-*'))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), before)
        registered = self.path.read_bytes()
        install.apply_mcp('devin')
        install.apply_mcp('devin', repair=True)
        self.assertEqual(self.path.read_bytes(), registered)
        self.assertEqual(list(self.path.parent.glob('mcp_config.json.everett-bak-*')), backups)
        self.assertFalse((self.root / 'session-data').exists())
        self.assertFalse((self.root / '.config/devin/config.json').exists())

    def test_repair_preserves_env_and_refreshes_remote_disabled_entry(self):
        self.write_config({'mcpServers': {
            'everett': {'url': 'https://example.invalid/mcp', 'transport': 'http',
                        'disabled': True, 'enabled': False,
                        'env': {'KEEP': 'yes', 'PYTHONPATH': '/missing/checkout'}},
            'other': {'command': 'untouched'},
        }})
        before = self.path.read_bytes()
        self.assertEqual(install.mcp_status('devin').state, 'stale')
        with self.assertRaisesRegex(ValueError, '--devin --repair --apply'):
            install.apply_mcp('devin')
        self.assertEqual(self.path.read_bytes(), before)
        install.apply_mcp('devin', repair=True)
        entry = json.loads(self.path.read_text())['mcpServers']['everett']
        self.assertEqual(entry['env'], {'KEEP': 'yes'})
        self.assertFalse(entry['disabled'])
        self.assertTrue(entry['enabled'])
        self.assertNotIn('url', entry)
        self.assertNotIn('transport', entry)
        self.assertTrue(install.mcp_status('devin').ready)
        self.assertEqual(json.loads(self.path.read_text())['mcpServers']['other'], {'command': 'untouched'})
        self.assertEqual(len(list(self.path.parent.glob('mcp_config.json.everett-bak-*'))), 1)

    def test_http_transport_is_not_reported_ready(self):
        self.write_config({'mcpServers': {'everett': {
            'command': sys.executable, 'args': ['-m', 'everett', 'mcp'], 'transport': 'http',
        }}})
        self.assertEqual(install.mcp_status('devin').state, 'stale')
        install.apply_mcp('devin', repair=True)
        self.assertTrue(install.mcp_status('devin').ready)

    def test_missing_and_invalid_configs(self):
        self.assertEqual(install.mcp_status('devin').state, 'missing')
        install.apply_mcp('devin')
        self.assertTrue(install.mcp_status('devin').ready)
        for text in ('{broken', '[]', '{"mcpServers": []}', '{"mcpServers": {"everett": "bad"}}'):
            with self.subTest(text=text):
                self.path.write_text(text)
                self.assertEqual(install.mcp_status('devin').state, 'invalid')
                with self.assertRaises(ValueError):
                    install.apply_mcp('devin', repair=True)
                self.assertEqual(self.path.read_text(), text)
                self.assertEqual(list(self.path.parent.glob('mcp_config.json.everett-bak-*')), [])

    def test_doctor_reports_missing_ready_stale_without_hooks(self):
        (self.root / 'session-data').mkdir()
        with mock.patch.object(doctor, 'probe_mcp', return_value=(True, 'mock stdio probe')):
            out = io.StringIO()
            with redirect_stdout(out):
                doctor.run()
            self.assertIn('everett install-mcp --devin --apply', out.getvalue())
            self.assertNotIn('install-hooks --devin', out.getvalue())
            install.apply_mcp('devin')
            out = io.StringIO()
            with redirect_stdout(out):
                doctor.run()
            self.assertIn('Everett stdio MCP registered', out.getvalue())
            self.assertIn(str(self.path), out.getvalue())
            data = json.loads(self.path.read_text())
            data['mcpServers']['everett']['disabled'] = True
            self.write_config(data)
            out = io.StringIO()
            with redirect_stdout(out):
                doctor.run()
            self.assertIn('everett install-mcp --devin --repair --apply', out.getvalue())

    def test_onboarding_config_only_discovery_has_mcp_but_no_hooks(self):
        self.write_config({})
        cfg = onboard.default_config()
        self.assertIn('devin', cfg.detected)
        self.assertTrue(cfg.mcp['devin'])
        self.assertNotIn('devin', cfg.hooks)
        self.assertNotIn('devin', onboard.HOOK_HARNESSES)


if __name__ == '__main__':
    unittest.main()
