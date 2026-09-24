import sandbox  # noqa: F401  (must be first: isolates HOME)

import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from everett import config, install, trunk
from everett.cli import main
from everett.route import RouteError, find_api_key, local_route, route
from everett.session import Session

NOW = 1_800_000_000.0


def session(sid, cwd, card='', first='', last_active=NOW, harness='claude'):
    return Session(harness, sid, cwd, f'/tmp/{sid}.jsonl', '', last_active, card=card, first_user=first)


class TempHome(unittest.TestCase):
    """Each test gets its own HOME so config/stores never leak between tests."""

    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.home = Path(self._dir.name)
        patcher = mock.patch.dict(os.environ, {'HOME': str(self.home), 'EVERETT_HOME': str(self.home)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self._dir.cleanup)

    def write(self, rel, text):
        path = self.home / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path


class LocalRouter(unittest.TestCase):
    def setUp(self):
        self.sessions = [
            session('a', '/work/kairos', card='Kairos: shipping the release notes and changelog'),
            session('b', '/work/app', card='App: fixing the login redirect bug; next add tests'),
            session('c', '/work/site', first='redesign the marketing site hero'),
        ]

    def test_picks_matching_session(self):
        r = local_route('keep going on kairos', self.sessions, now=NOW)
        self.assertEqual((r['decision'], r['session']['id']), ('SESSION', 'a'))
        self.assertGreaterEqual(r['confidence'], 0.6)
        self.assertIn('claude --resume a', r['command'])

    def test_same_output_shape_as_jev(self):
        r = local_route('login redirect tests', self.sessions, now=NOW)
        self.assertTrue({'input', 'choice', 'confidence', 'decision'} <= set(r))
        self.assertEqual(r['choice'], 's1')

    def test_unrelated_request_is_new(self):
        r = local_route('write a haiku about penguins', self.sessions, now=NOW)
        self.assertEqual(r['decision'], 'NEW')
        self.assertEqual(r['command'], "claude 'write a haiku about penguins'")

    def test_tie_asks(self):
        twins = [session('x', '/w/one', card='billing export'), session('y', '/w/two', card='billing export')]
        r = local_route('billing export', twins, now=NOW)
        self.assertEqual(r['decision'], 'ASK')
        self.assertTrue(r['suggested'])

    def test_recency_breaks_ties(self):
        twins = [session('old', '/w/one', card='billing export', last_active=NOW - 30 * 86400),
                 session('new', '/w/two', card='billing export', last_active=NOW)]
        r = local_route('billing export', twins, now=NOW)
        self.assertEqual(r.get('session', {}).get('id'), 'new')

    def test_empty_and_stopword_queries(self):
        self.assertEqual(local_route('anything', [], now=NOW)['decision'], 'NEW')
        self.assertEqual(local_route('the and of', self.sessions, now=NOW)['decision'], 'ASK')

    def test_route_flag_forces_local_even_with_key(self):
        jev = mock.Mock()
        r = route('keep going on kairos', self.sessions, router='local', api_key='k', jev=jev)
        self.assertEqual(r['router'], 'local')
        jev.assert_not_called()

    def test_unknown_router(self):
        with self.assertRaises(RouteError):
            route('x', self.sessions, router='magic', api_key='')


class Config(TempHome):
    def test_parse_sections_aliases_comments(self):
        self.write('.everett/config.toml', '# c\nvault = "~/Notes"  # mine\ndefault_harness = codex\n'
                                           '[jev]\napi_key = \'k-1\'\n')
        self.assertEqual(config.values(), {'vault': '~/Notes', 'default_harness': 'codex', 'typesafe_api_key': 'k-1'})

    def test_key_sources_in_order(self):
        self.assertEqual(find_api_key(), '')
        self.write('.hermes/.env', 'OTHER=1\nTYPESAFE_API_KEY="from-hermes"\n')
        self.assertEqual(find_api_key(), 'from-hermes')
        self.write('.everett/config.toml', 'typesafe_api_key = "from-config"\n')
        self.assertEqual(find_api_key(), 'from-config')
        with mock.patch.dict(os.environ, {'TYPESAFE_API_KEY': 'from-env'}):
            self.assertEqual(find_api_key(), 'from-env')

    def test_default_harness_for_new(self):
        self.write('.everett/config.toml', 'default_harness = "codex"\n')
        self.assertEqual(local_route('x y', [], now=NOW)['command'], "codex 'x y'")


class Trunk(TempHome):
    def test_unset_vault_is_a_clear_error(self):
        with self.assertRaises(trunk.VaultNotConfiguredError) as cm:
            trunk.output_path()
        self.assertIn('EVERETT_VAULT', str(cm.exception))
        err = io.StringIO()
        with mock.patch('everett.cli.registry.scan', return_value=[]), contextlib.redirect_stderr(err):
            self.assertEqual(main(['trunk']), 2)
        self.assertIn('no vault configured', err.getvalue())

    def test_vault_from_env_and_config(self):
        with mock.patch.dict(os.environ, {'EVERETT_VAULT': str(self.home / 'v')}):
            self.assertEqual(trunk.write([]), self.home / 'v' / 'Everett' / 'Trunk.md')
        self.write('.everett/config.toml', f'vault = "{self.home / "w"}"\nvault_dir = "Projects/Everett"\n')
        self.assertEqual(trunk.output_path(), self.home / 'w' / 'Projects' / 'Everett' / 'Trunk.md')


class InstallHooks(TempHome):
    OTHER = {'type': 'command', 'command': 'notify-me', 'timeout': 1}

    def test_print_does_not_write(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(main(['install-hooks']), 0)
        self.assertIn('claude_session_start.py', out.getvalue())
        self.assertIn('codex_stop.py', out.getvalue())
        self.assertIn('omp_session_start.mjs', out.getvalue())
        self.assertFalse((self.home / '.claude').exists())

    def test_apply_backs_up_merges_and_is_idempotent(self):
        settings = self.write('.claude/settings.json', json.dumps(
            {'model': 'x', 'hooks': {'Stop': [{'hooks': [self.OTHER]}], 'PreToolUse': [{'hooks': [self.OTHER]}]}}))
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(['install-hooks', '--claude', '--apply']), 0)
        data = json.loads(settings.read_text())
        self.assertEqual(data['model'], 'x')
        self.assertEqual(data['hooks']['PreToolUse'], [{'hooks': [self.OTHER]}])
        self.assertEqual(data['hooks']['Stop'][0], {'hooks': [self.OTHER]})
        self.assertEqual(len(data['hooks']['Stop']), 2)
        self.assertEqual(len(data['hooks']['SessionStart']), 1)
        backups = list(settings.parent.glob('settings.json.everett-bak-*'))
        self.assertEqual(len(backups), 1)
        self.assertNotIn('SessionStart', backups[0].read_text())
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main(['install-hooks', '--claude', '--apply'])
        self.assertIn('already installed', out.getvalue())
        self.assertEqual(json.loads(settings.read_text()), data)
        self.assertEqual(len(list(settings.parent.glob('settings.json.everett-bak-*'))), 1)

    def test_existing_entry_at_another_path_counts(self):
        cmd = 'python3 /somewhere/everett/everett/hooks/codex_session_start.py'
        self.write('.codex/hooks.json', json.dumps({'hooks': {'SessionStart': [{'hooks': [{'command': cmd}]}]}}))
        self.assertEqual(install.installed('codex'), {'SessionStart': True, 'Stop': False})
        with contextlib.redirect_stdout(io.StringIO()):
            main(['install-hooks', '--codex', '--apply'])
        data = json.loads((self.home / '.codex/hooks.json').read_text())
        self.assertEqual(len(data['hooks']['SessionStart']), 1)
        self.assertEqual(install.installed('codex'), {'SessionStart': True, 'Stop': True})

    def test_new_file_is_created_without_backup(self):
        with contextlib.redirect_stdout(io.StringIO()):
            main(['install-hooks', '--codex', '--omp', '--apply'])
        self.assertTrue(all(install.installed('codex').values()))
        ext = self.home / '.omp/agent/extensions/everett.ts'
        self.assertIn('omp_session_start.mjs', ext.read_text())
        self.assertEqual(list((self.home / '.codex').glob('*.everett-bak-*')), [])

    def test_malformed_settings_are_not_overwritten(self):
        path = self.write('.claude/settings.json', '{not json')
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(main(['install-hooks', '--claude', '--apply']), 1)
        self.assertEqual(path.read_text(), '{not json')


class Doctor(TempHome):
    def test_doctor_runs_on_an_empty_home(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = main(['doctor'])
        self.assertEqual(code, 0)
        text = out.getvalue()
        self.assertIn('python', text)
        self.assertIn('router: local', text)
        self.assertIn('vault: not configured', text)


if __name__ == '__main__':
    unittest.main()
