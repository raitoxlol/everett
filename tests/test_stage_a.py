import sandbox  # noqa: F401  (must be first: isolates HOME)

import contextlib
import io
import json
import os
import sqlite3
import tempfile
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from everett import registry
from everett.adapters import hermes, pi
from everett.cli import main
from everett.send import (MAX_HOPS, SendError, command_for, hop_env, is_busy, refuse_self, spawn,
                          spawn_command)
from everett.session import Session

SCHEMA = '''
CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, started_at REAL NOT NULL, ended_at REAL,
  cwd TEXT, title TEXT, profile_name TEXT, parent_session_id TEXT, hidden INTEGER DEFAULT 0,
  archived INTEGER DEFAULT 0, last_activity_at REAL);
CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL,
  content TEXT, timestamp REAL NOT NULL);
'''


class TempHome(unittest.TestCase):
    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.home = Path(self._dir.name)
        patcher = mock.patch.dict(os.environ, {'HOME': str(self.home), 'EVERETT_HOME': str(self.home)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self._dir.cleanup)


def make_db(path: Path, rows, messages):
    path.parent.mkdir(parents=True, exist_ok=True)
    con = sqlite3.connect(path)
    con.executescript(SCHEMA)
    con.executemany('INSERT INTO sessions (id, source, started_at, cwd, title, profile_name, parent_session_id, '
                    'hidden, last_activity_at) VALUES (?,?,?,?,?,?,?,?,?)', rows)
    con.executemany('INSERT INTO messages (session_id, role, content, timestamp) VALUES (?,?,?,?)', messages)
    con.commit()
    con.close()


class HermesAdapter(TempHome):
    def setUp(self):
        super().setUp()
        now = time.time()
        self.db = self.home / '.hermes/profiles/max/state.db'
        make_db(self.db, [
            ('h-cli', 'cli', now - 600, '/work/api', 'API retries', 'max', None, 0, now - 60),
            ('h-tg', 'telegram', now - 900, '', '', 'max', None, 0, now - 120),
            ('h-cron', 'cron', now - 900, '', 'nightly', 'max', None, 0, now - 30),
            ('h-hidden', 'cli', now - 900, '', 'x', 'max', None, 1, now - 30),
            ('h-child', 'cli', now - 900, '', 'x', 'max', 'h-cli', 0, now - 30),
            ('h-old', 'cli', now - 10 * 86400, '', 'old', 'max', None, 0, now - 10 * 86400),
        ], [
            ('h-cli', 'user', 'add retries to the upload client', now - 590),
            ('h-cli', 'assistant', 'ok', now - 580),
            ('h-cli', 'user', 'cap them at five', now - 70),
            ('h-tg', 'user', 'what is on my calendar', now - 130),
        ])
        make_db(self.home / '.hermes/state.db', [('root-1', 'cli', now - 60, '/w', 'Root', None, None, 0, now - 50)],
                [('root-1', 'user', 'hello root', now - 55)])

    def test_reads_sessions_read_only(self):
        before = self.db.read_bytes()
        found = {s.id: s for s in hermes.scan(72)}
        self.assertEqual(set(found), {'h-cli', 'h-tg', 'h-cron', 'root-1'})
        s = found['h-cli']
        self.assertEqual((s.harness, s.profile, s.cwd, s.title), ('hermes', 'max', '/work/api', 'API retries'))
        self.assertEqual((s.first_user, s.last_user), ('add retries to the upload client', 'cap them at five'))
        self.assertTrue(found['h-cron'].auto)
        self.assertFalse(found['h-tg'].auto)
        self.assertEqual(found['root-1'].profile, 'default')
        self.assertEqual(self.db.read_bytes(), before)

    def test_connection_is_read_only(self):
        con = hermes._connect(self.db)
        with self.assertRaises(sqlite3.OperationalError):
            con.execute("INSERT INTO messages (session_id, role, content, timestamp) VALUES ('x','user','y',0)")
        con.close()

    def test_broken_db_is_skipped(self):
        bad = self.home / '.hermes/profiles/bad/state.db'
        bad.parent.mkdir(parents=True)
        bad.write_text('not sqlite')
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertIn('h-cli', {s.id for s in hermes.scan(72)})

    def test_resume_commands_and_chat_sessions_refused(self):
        found = {s.id: s for s in hermes.scan(72)}
        self.assertEqual(command_for(found['h-cli'], 'go'),
                         ['hermes', '-p', 'max', 'chat', '--resume', 'h-cli', '-Q', '-q', 'go'])
        self.assertEqual(command_for(found['root-1'], 'go')[:3], ['hermes', '-p', 'default'])
        with self.assertRaises(SendError):
            command_for(found['h-tg'], 'go')

    def test_busy_uses_session_activity_not_db_mtime(self):
        s = Session('hermes', 'h', '', str(self.db), '', time.time() - 3600, source='cli')
        os.utime(self.db)  # other sessions keep writing the shared db
        self.assertFalse(is_busy(s, '', time.time()))


class PiAdapter(TempHome):
    def test_pi_sessions_share_omp_format(self):
        rows = [{'type': 'session', 'version': 3, 'id': 'pi-1', 'timestamp': '2026-09-24T00:00:00Z', 'cwd': '/work/site'},
                {'type': 'message', 'message': {'role': 'user', 'content': [{'type': 'text', 'text': 'redo the hero'}]}},
                {'type': 'message', 'message': {'role': 'assistant', 'content': [{'type': 'text', 'text': 'done'}]}}]
        path = self.home / '.pi/agent/sessions/--work-site--/2026-09-24_pi-1.jsonl'
        path.parent.mkdir(parents=True)
        path.write_text('\n'.join(map(json.dumps, rows)) + '\n')
        [s] = pi.scan(72)
        self.assertEqual((s.harness, s.id, s.cwd, s.first_user), ('pi', 'pi-1', '/work/site', 'redo the hero'))
        self.assertEqual(command_for(s, 'go'), ['pi', '--session', str(path), '-p', 'go'])


class DirectSend(unittest.TestCase):
    def setUp(self):
        self.sessions = [
            Session('claude', 'abc-111', '/work/atlas', '', '', 0, card='Atlas: release notes'),
            Session('codex', 'abd-222', '/work/app', '', '', 0, title='Login bug'),
            Session('omp', 'xyz-333', '/work/app', '', '', 0),
        ]

    def test_exact_prefix_name_and_folder(self):
        self.assertEqual(registry.find('abc', self.sessions).id, 'abc-111')
        self.assertEqual(registry.find('xyz-333', self.sessions).id, 'xyz-333')
        self.assertEqual(registry.find('Atlas', self.sessions).id, 'abc-111')

    def test_ambiguous_prefix_lists_candidates(self):
        with self.assertRaises(registry.SessionLookupError) as cm:
            registry.find('ab', self.sessions)
        self.assertIn('abc-111', str(cm.exception))
        self.assertIn('abd-222', str(cm.exception))
        with self.assertRaises(registry.SessionLookupError):
            registry.find('app', self.sessions)  # two sessions in that folder
        with self.assertRaises(registry.SessionLookupError):
            registry.find('nothing', self.sessions)

    def test_cli_to_skips_routing(self):
        out = io.StringIO()
        with mock.patch('everett.cli.registry.scan', return_value=self.sessions), \
                mock.patch('everett.cli.route') as router, \
                mock.patch('everett.cli.send', return_value=SimpleNamespace(command=['x'], reply='done')) as sender, \
                contextlib.redirect_stdout(out):
            self.assertEqual(main(['send', '--to', 'abc', 'ship it', '--json']), 0)
        router.assert_not_called()
        self.assertEqual(sender.call_args.args[0].id, 'abc-111')
        result = json.loads(out.getvalue())
        self.assertEqual((result['router'], result['reply']), ('direct', 'done'))

    def test_cli_to_ambiguous_exits_2(self):
        err = io.StringIO()
        with mock.patch('everett.cli.registry.scan', return_value=self.sessions), contextlib.redirect_stderr(err):
            self.assertEqual(main(['send', '--to', 'ab', 'x']), 2)
        self.assertIn('abd-222', err.getvalue())


class Safety(unittest.TestCase):
    def test_hop_guard(self):
        with mock.patch.dict(os.environ, {'EVERETT_HOPS': '1'}):
            self.assertEqual(hop_env()['EVERETT_HOPS'], '2')
            self.assertEqual(hop_env()['EVERETT_SEND'], '1')
        with mock.patch.dict(os.environ, {'EVERETT_HOPS': str(MAX_HOPS)}):
            with self.assertRaises(SendError) as cm:
                hop_env()
            self.assertEqual(cm.exception.code, 7)
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                self.assertEqual(main(['send', 'anything']), 7)

    def test_self_send_refused(self):
        s = Session('claude', 'me-123', '', '', '', 0)
        with self.assertRaises(SendError):
            refuse_self(s, caller='me-123')
        refuse_self(s, caller='other')
        with mock.patch.dict(os.environ, {'EVERETT_SESSION_ID': 'me-123'}):
            with self.assertRaises(SendError):
                refuse_self(s)


class Spawn(TempHome):
    def test_spawn_commands(self):
        self.assertEqual(spawn_command('claude', 'hi', 'u-1'), ['claude', '--session-id', 'u-1', '--print', 'hi'])
        self.assertEqual(spawn_command('codex', 'hi', out_file='/t/o'),
                         ['codex', 'exec', '--skip-git-repo-check', '-o', '/t/o', 'hi'])
        self.assertEqual(spawn_command('omp', 'hi'), ['omp', '-p', 'hi'])
        with self.assertRaises(SendError):
            spawn_command('vim', 'hi')

    def test_claude_spawn_preassigns_id_and_records_it(self):
        run = mock.Mock(return_value=SimpleNamespace(returncode=0, stdout='OK\n', stderr=''))
        with mock.patch('everett.send.subprocess.run', run):
            result = spawn('claude', 'reply OK', str(self.home))
        self.assertEqual(result.reply, 'OK')
        self.assertEqual(run.call_args.args[0][:2], ['claude', '--session-id'])
        self.assertEqual(run.call_args.args[0][2], result.session_id)
        self.assertEqual(run.call_args.kwargs['cwd'], str(self.home))
        self.assertIn(result.session_id, registry.spawned_ids())
        hidden = Session('claude', result.session_id, '', '', '', 0, auto=True)
        self.assertFalse(registry.is_auto(hidden, registry.spawned_ids()))

    def test_codex_spawn_reads_last_message_and_session_id(self):
        def fake_run(cmd, **kw):
            Path(cmd[cmd.index('-o') + 1]).write_text('the answer')
            return SimpleNamespace(returncode=0, stdout='noise',
                                   stderr='session id: 01a0d358-0f69-77f3-950a-8bfbe2f5a13a\n')
        with mock.patch('everett.send.subprocess.run', side_effect=fake_run):
            result = spawn('codex', 'q', str(self.home))
        self.assertEqual((result.reply, result.session_id), ('the answer', '01a0d358-0f69-77f3-950a-8bfbe2f5a13a'))

    def test_spawn_errors(self):
        with self.assertRaises(SendError):
            spawn('claude', 'x', str(self.home / 'missing'))
        failed = SimpleNamespace(returncode=3, stdout='', stderr='boom')
        with mock.patch('everett.send.subprocess.run', return_value=failed), self.assertRaises(SendError) as cm:
            spawn('claude', 'x', str(self.home))
        self.assertEqual(cm.exception.code, 6)

    def test_cli_spawn_only_with_flag_on_confident_new(self):
        routed = {'input': 'x', 'decision': 'NEW', 'choice': 'new', 'confidence': 0.9, 'router': 'local'}
        fake = SimpleNamespace(command=['claude'], reply='hi', session_id='new-1', harness='codex', cwd=str(self.home))
        with mock.patch('everett.cli.registry.scan', return_value=[]), \
                mock.patch('everett.cli.route', return_value=routed), \
                mock.patch('everett.cli.spawn', return_value=fake) as spawner:
            with contextlib.redirect_stdout(io.StringIO()):
                main(['send', 'x'])
            spawner.assert_not_called()
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(main(['send', 'x', '--spawn', '--harness', 'codex', '--dir', str(self.home), '--json']), 0)
            self.assertEqual(spawner.call_args.args[:3], ('codex', 'x', str(self.home)))
            self.assertEqual(json.loads(out.getvalue())['session_id'], 'new-1')
            spawner.reset_mock()
            with mock.patch('everett.cli.route', return_value={**routed, 'confidence': 0.3}), \
                    contextlib.redirect_stdout(io.StringIO()):
                main(['send', 'x', '--spawn'])
            spawner.assert_not_called()

    def test_spawn_dir_defaults_to_best_match(self):
        sessions = [Session('claude', 'a', '/work/atlas', '', '', time.time(), card='atlas release')]
        from everett.route import best_dir
        self.assertEqual(best_dir('atlas docs', sessions), '/work/atlas')
        self.assertEqual(best_dir('penguins', sessions), '')


if __name__ == '__main__':
    unittest.main()
