import sandbox  # noqa: F401  (must be first: isolates HOME)

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from everett import core
from everett.cli import main

REPO = Path(__file__).resolve().parents[1]


class TempHome(unittest.TestCase):
    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.home = Path(self._dir.name)
        self.env = {'HOME': str(self.home), 'EVERETT_HOME': str(self.home)}
        patcher = mock.patch.dict(os.environ, self.env)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self._dir.cleanup)
        self.project = self.home / 'src' / 'atlas'
        (self.project / '.git').mkdir(parents=True)
        (self.project / 'pkg').mkdir()


class SecretFilter(unittest.TestCase):
    SECRETS = [
        'use key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv',
        'token ghp_abcdefghijklmnopqrstuvwxyz0123',
        'AKIAIOSFODNN7EXAMPLE is the aws key',
        'password = hunter2hunter2',
        'API_KEY: 9f8e7d6c5b4a',
        '-----BEGIN OPENSSH PRIVATE KEY----- b3BlbnNzaC1r',
        'db at postgres://admin:s3cretpw@db.internal/app',
        'header Authorization: Bearer abcdefghijklmnop1234',
        'jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N',
        'random Xk9fQ2LmP8vR3tW7yZ1aB4cD6eH0jN5s',
    ]
    CLEAN = [
        'Tests run with python3 -m unittest discover -s tests',
        'The deploy target is fly.io, region nrt; never push on Fridays',
        'Use the token budget of 4000 for summaries',
        'Path is /Users/someone/Projects/everett/everett/hooks/common.py',
        'Session ids look like 01a0d358-0f69-77f3-950a-8bfbe2f5a13a',
    ]

    def test_detects_secrets(self):
        for text in self.SECRETS:
            self.assertTrue(core.find_secret(text), text)

    def test_allows_ordinary_facts(self):
        for text in self.CLEAN:
            self.assertEqual(core.find_secret(text), '', text)


class Learn(TempHome):
    def test_appends_entry_with_metadata(self):
        with mock.patch.dict(os.environ, {'EVERETT_SESSION_ID': 's-1', 'CLAUDECODE': '1'}):
            entry = core.learn('Atlas uses pnpm, not npm', scope='project', cwd=str(self.project / 'pkg'))
        self.assertEqual((entry['project'], entry['scope'], entry['session'], entry['harness']),
                         ('atlas', 'project', 's-1', 'claude'))
        [stored] = core.read_inbox()
        self.assertEqual(stored['text'], 'Atlas uses pnpm, not npm')
        self.assertEqual(stored['cwd'], str(self.project / 'pkg'))

    def test_defaults_and_rejections(self):
        self.assertEqual(core.learn('global fact', cwd=str(self.home))['scope'], 'global')
        self.assertEqual(core.learn('p fact', project='My App')['project'], 'my-app')
        with self.assertRaises(core.CoreError):
            core.learn('x' * 501)
        with self.assertRaises(core.CoreError):
            core.learn('   ')
        with self.assertRaises(core.CoreError) as cm:
            core.learn('the password = hunter2hunter2')
        self.assertIn('credential', str(cm.exception))
        with self.assertRaises(core.CoreError):
            core.learn('needs a project', scope='project', cwd=str(self.home))
        self.assertEqual(len(core.read_inbox()), 2)

    def test_cli_learn_rejects_secret_with_exit_2(self):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(main(['learn', 'key sk-proj-abcdefghijklmnopqrstu']), 2)
        self.assertIn('Secrets never go into the shared core', err.getvalue())
        self.assertEqual(core.read_inbox(), [])


class Merge(TempHome):
    def test_deterministic_merge_dedupes_and_archives(self):
        core.learn('Everett tests use a temp HOME', cwd=str(self.home))
        core.learn('Atlas deploys from main', project='atlas')
        core.learn('everett tests use a temp home.', cwd=str(self.home))  # same fact, newer
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(main(['trunk', 'merge', '--llm', 'none']), 0)
        text = core.global_path().read_text()
        self.assertEqual(text.count('temp'), 1)
        self.assertIn('everett tests use a temp home.', text)
        self.assertIn('Atlas deploys from main', core.project_path('atlas').read_text())
        self.assertFalse(core.inbox_path().exists())
        [snap] = list(core.history_dir().iterdir())
        self.assertEqual(len((snap / 'inbox.jsonl').read_text().splitlines()), 3)

        core.learn('Second round fact', cwd=str(self.home))
        before = core.global_path().read_text()
        with contextlib.redirect_stdout(io.StringIO()):
            main(['trunk', 'merge', '--llm', 'none'])
        snaps = sorted(core.history_dir().iterdir())
        self.assertEqual(len(snaps), 2)
        self.assertEqual((snaps[-1] / 'core.md').read_text(), before)
        self.assertIn('Atlas deploys', (snaps[-1] / 'projects' / 'atlas.md').read_text())

    def test_dry_run_writes_nothing(self):
        core.learn('A fact', cwd=str(self.home))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main(['trunk', 'merge', '--llm', 'none', '--dry-run'])
        self.assertIn('A fact', out.getvalue())
        self.assertFalse(core.global_path().exists())
        self.assertEqual(len(core.read_inbox()), 1)

    def test_word_cap_drops_oldest(self):
        for i in range(80):
            core.learn(f'fact number {i} about the build system and its many quirks', cwd=str(self.home))
        with contextlib.redirect_stdout(io.StringIO()):
            main(['trunk', 'merge', '--llm', 'none'])
        text = core.global_path().read_text()
        self.assertLessEqual(core.words(text), core.CORE_WORDS)
        self.assertIn('fact number 79 ', text)
        self.assertNotIn('fact number 0 ', text)

    def test_llm_merge_uses_headless_harness(self):
        core.learn('Old: deploy on Fridays', cwd=str(self.home))
        reply = json.dumps({'global': '# Everett core\n\n- Never deploy on Fridays (was: deploy on Fridays)\n',
                            'projects': {'Atlas': '# Project core: atlas\n\n- Uses pnpm\n'}})
        runner = mock.Mock(return_value=SimpleNamespace(returncode=0, stdout=reply, stderr=''))
        result = core.merge('claude', runner=runner)
        command = runner.call_args.args[0]
        self.assertEqual(command[:2], ['claude', '--print'])
        self.assertIn('Old: deploy on Fridays', command[-1])
        self.assertEqual(runner.call_args.kwargs['env']['EVERETT_SEND'], '1')
        self.assertIn('Never deploy', core.global_path().read_text())
        self.assertIn('pnpm', core.project_path('atlas').read_text())
        self.assertEqual(result['merged'], 1)

    def test_bad_llm_output_changes_nothing(self):
        core.learn('A fact', cwd=str(self.home))
        for stdout in ('I merged it for you!', json.dumps({'global': 'token = abcdefghijklmnop'})):
            runner = mock.Mock(return_value=SimpleNamespace(returncode=0, stdout=stdout, stderr=''))
            with self.assertRaises(core.CoreError):
                core.merge('codex', runner=runner)
        self.assertFalse(core.global_path().exists())
        self.assertEqual(len(core.read_inbox()), 1)

    def test_llm_output_is_capped(self):
        core.learn('x', cwd=str(self.home))
        long = '# T\n' + ''.join(f'- item {i} ' + 'word ' * 20 + '\n' for i in range(40))
        runner = mock.Mock(return_value=SimpleNamespace(returncode=0, stdout=json.dumps({'global': long}), stderr=''))
        core.merge('claude', runner=runner)
        self.assertLessEqual(core.words(core.global_path().read_text()), core.CORE_WORDS)

    def test_vault_mirror(self):
        core.learn('Mirror me', cwd=str(self.home))
        with mock.patch.dict(os.environ, {'EVERETT_VAULT': str(self.home / 'vault')}):
            result = core.merge('none')
        mirror = self.home / 'vault' / 'Everett' / 'Core.md'
        self.assertEqual(result['mirror'], str(mirror))
        self.assertIn('Mirror me', mirror.read_text())

    def test_empty_inbox(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(main(['trunk', 'merge', '--llm', 'none']), 0)
        self.assertIn('nothing to merge', out.getvalue())


class Pull(TempHome):
    def _hook(self, script, payload, extra=None):
        start = time.perf_counter()
        result = subprocess.run([sys.executable, f'everett/hooks/{script}'], input=payload, capture_output=True,
                                text=True, cwd=REPO, env={**self.env, 'PATH': '/usr/bin:/bin', **(extra or {})})
        return result, time.perf_counter() - start

    def _fill(self):
        core.global_path().parent.mkdir(parents=True, exist_ok=True)
        core.global_path().write_text('# Everett core\n\n' + ''.join(f'- global fact {i} ' + 'g ' * 10 + '\n' for i in range(40)))
        core.project_path('atlas').parent.mkdir(parents=True, exist_ok=True)
        core.project_path('atlas').write_text('# Project core: atlas\n\n- Atlas uses pnpm\n')

    def test_claude_and_codex_hooks_inject_bounded_core(self):
        self._fill()
        for script in ('claude_session_start.py', 'codex_session_start.py'):
            payload = json.dumps({'session_id': 'abc', 'cwd': str(self.project / 'pkg')})
            result, elapsed = self._hook(script, payload)
            self.assertEqual(result.returncode, 0)
            ctx = json.loads(result.stdout)['hookSpecificOutput']['additionalContext']
            self.assertIn('.everett/cards/abc.md', ctx)
            self.assertIn('Atlas uses pnpm', ctx)
            self.assertIn('global fact 0', ctx)
            self.assertIn('everett learn', ctx)
            shared = ctx.split('Everett shared core', 1)[1]
            self.assertLessEqual(core.words(shared), core.CONTEXT_WORDS)
            self.assertLess(elapsed, 0.3 if os.environ.get('CI') is None else 1.0, script)

    def test_context_without_core_is_just_the_learn_line(self):
        self.assertEqual(core.context(str(self.home)), core.LEARN_LINE)

    def test_broken_core_keeps_card_and_stays_silent(self):
        core.global_path().mkdir(parents=True)  # a directory where the file should be
        result, _ = self._hook('claude_session_start.py', json.dumps({'session_id': 'abc', 'cwd': str(self.home)}))
        self.assertEqual((result.returncode, result.stderr), (0, ''))
        self.assertIn('.everett/cards/abc.md', result.stdout)
        result, _ = self._hook('claude_session_start.py', 'garbage')
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, '', ''))

    def test_headless_sends_stay_silent(self):
        self._fill()
        result, _ = self._hook('codex_session_start.py', json.dumps({'session_id': 'abc'}), {'EVERETT_SEND': '1'})
        self.assertEqual(result.stdout, '')

    def test_omp_helper_includes_core(self):
        self._fill()
        result = subprocess.run([sys.executable, str(REPO / 'everett/hooks/omp_card_context.py'), 'o-1'],
                                capture_output=True, text=True, cwd=self.project,
                                env={**self.env, 'PATH': '/usr/bin:/bin'})
        self.assertIn('Atlas uses pnpm', result.stdout)
        self.assertIn('o-1.md', result.stdout)


class CoreCLI(TempHome):
    def test_show_edit_path_history_and_trunk_alias(self):
        core.learn('Shown fact', cwd=str(self.home))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main(['core'])
        self.assertIn('1 learning(s) waiting', out.getvalue())
        with contextlib.redirect_stdout(io.StringIO()):
            main(['trunk', 'merge', '--llm', 'none'])
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main(['core', 'show'])
            main(['core', 'edit-path'])
            main(['core', 'history'])
        text = out.getvalue()
        self.assertIn('Shown fact', text)
        self.assertIn(str(core.global_path()), text)
        self.assertIn(str(core.history_dir()), text)
        with mock.patch('everett.cli.registry.scan', return_value=[]), \
                mock.patch.dict(os.environ, {'EVERETT_VAULT': str(self.home / 'v')}), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(['trunk']), 0)
            self.assertEqual(main(['trunk', 'view']), 0)
        self.assertTrue((self.home / 'v' / 'Everett' / 'Trunk.md').exists())


if __name__ == '__main__':
    unittest.main()
