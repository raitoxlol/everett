import sandbox  # noqa: F401

import os
import argparse
import io
import json
import sys
from contextlib import redirect_stdout
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from everett import doctor, install, onboard, send
from everett.cli import main
from everett.session import Session


class InstallationErrors(unittest.TestCase):
    def test_missing_harness_fails_before_waiting_for_a_session(self):
        session = Session('claude', 'busy', '', '/missing', '', 0)
        with mock.patch('everett.send.shutil.which', return_value=None), \
                mock.patch('everett.send.wait_idle') as wait:
            with self.assertRaisesRegex(send.SendError, 'not on PATH.*everett doctor'):
                send.send(session, 'review')
        wait.assert_not_called()

    def test_codex_output_is_removed_after_start_failure_or_timeout(self):
        for error in (FileNotFoundError('codex'), subprocess.TimeoutExpired('codex', 1)):
            with self.subTest(error=type(error).__name__), tempfile.TemporaryDirectory() as tmp:
                with mock.patch('everett.send.shutil.which', return_value='/bin/codex'), \
                        mock.patch('tempfile.tempdir', tmp), \
                        mock.patch('everett.send.subprocess.run', side_effect=error):
                    with self.assertRaises(send.SendError):
                        send.spawn('codex', 'review', tmp)
                self.assertEqual(list(Path(tmp).iterdir()), [])

    def test_invalid_request_does_not_allocate_a_codex_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch('tempfile.tempdir', tmp):
                with self.assertRaises(send.SendError):
                    send.spawn('codex', '', tmp)
            self.assertEqual(list(Path(tmp).iterdir()), [])


class FirstRun(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        env = mock.patch.dict(os.environ, {'HOME': tmp.name, 'EVERETT_HOME': tmp.name})
        env.start()
        self.addCleanup(env.stop)
        path = mock.patch('shutil.which', return_value=None)
        path.start()
        self.addCleanup(path.stop)

    def test_harnesses_are_detected_before_their_first_session(self):
        with mock.patch('shutil.which', side_effect=lambda h, **kw: '/bin/' + h if h in ('claude', 'grok') else None):
            cfg = onboard.default_config()
        self.assertEqual(cfg.detected, {'claude': 0, 'grok': 0})
        self.assertEqual(cfg.mcp, {'claude': True, 'grok': True})

    def test_eof_cancels_instead_of_applying_defaults(self):
        cfg = onboard.OnboardConfig(detected={'claude': 0}, hooks={'claude': {'Stop': True}})
        args = argparse.Namespace(yes=False, span_days=3, no_backfill=True, no_mcp=False)
        with mock.patch('everett.onboard.default_config', return_value=cfg), \
                mock.patch('builtins.input', side_effect=EOFError), \
                mock.patch('everett.onboard.run_apply') as apply, redirect_stdout(io.StringIO()):
            self.assertEqual(onboard.run(args), 0)
        apply.assert_not_called()

    def test_doctor_probes_stdio_and_explains_empty_home(self):
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(main(['doctor']), 0)
        text = out.getvalue()
        self.assertIn('10 tools over stdio', text)
        self.assertIn('No harness CLI', text)
        self.assertIn('everett onboard --yes', text)
        self.assertNotIn('\nhealthy\n', text)

    def test_stale_registration_is_repaired_with_backup_and_other_servers_kept(self):
        for harness in ('claude', 'codex', 'omp', 'grok'):
            with self.subTest(harness=harness):
                path = install.mcp_path(harness)
                path.parent.mkdir(parents=True, exist_ok=True)
                if harness in install.TOML_MCP:
                    original = ('model = "chosen"\n[mcp_servers.other]\ncommand = "other"\n'
                                '[mcp_servers.everett]\ncommand = "/removed/python"\n'
                                'args = ["-m", "everett", "mcp"]\nenabled = false\n'
                                '[mcp_servers.everett.env]\nKEEP = "value"\n'
                                '[mcp_servers.later]\ncommand = "later"\n')
                else:
                    original = json.dumps({'keep': 1, 'mcpServers': {
                        'other': {'command': 'other'}, 'everett': {
                            'command': '/removed/python', 'args': ['-m', 'everett', 'mcp'],
                            'disabled': True, 'env': {'KEEP': 'value'}}}})
                path.write_text(original)
                self.assertFalse(install.mcp_status(harness).ready)
                with self.assertRaisesRegex(ValueError, '--repair --apply'):
                    install.apply_mcp(harness)
                self.assertEqual(path.read_text(), original)
                install.apply_mcp(harness, repair=True)
                self.assertTrue(install.mcp_status(harness).ready)
                self.assertIn('other', path.read_text())
                self.assertIn('KEEP', path.read_text())
                self.assertEqual(next(path.parent.glob(path.name + '.everett-bak-*')).read_text(), original)

    def test_malformed_configuration_is_not_overwritten(self):
        for harness, original in (('claude', '{broken'), ('codex', '[broken')):
            path = install.mcp_path(harness)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(original)
            with self.assertRaises(ValueError):
                install.apply_mcp(harness, repair=True)
            self.assertEqual(path.read_text(), original)

    def test_repair_requires_apply(self):
        with redirect_stdout(io.StringIO()):
            self.assertEqual(main(['install-mcp', '--claude', '--repair']), 2)
        self.assertFalse(install.mcp_path('claude').exists())
