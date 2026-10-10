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
from unittest import mock

from everett import registry
from everett.adapters import devin
from everett.route import resume_command
from everett.send import SendError, command_for, is_busy, spawn_command
from everett.session import Session

SCHEMA = '''
CREATE TABLE sessions (id TEXT PRIMARY KEY, working_directory TEXT, title TEXT, model TEXT,
  created_at REAL, last_activity_at REAL, hidden INTEGER DEFAULT 0);
CREATE TABLE message_nodes (node_id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,
  created_at REAL, chat_message TEXT);
'''

SCHEMA_NO_NODES = '''
CREATE TABLE sessions (id TEXT PRIMARY KEY, working_directory TEXT, title TEXT, model TEXT,
  created_at REAL, last_activity_at REAL, hidden INTEGER DEFAULT 0);
'''


class TempHome(unittest.TestCase):
    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.home = Path(self._dir.name)
        patcher = mock.patch.dict(os.environ, {'HOME': str(self.home), 'EVERETT_HOME': str(self.home)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self._dir.cleanup)
        self.data = self.home / '.local/share/devin/cli'  # a candidate dir on every platform
        patcher2 = mock.patch.dict(os.environ, {'DEVIN_HOME': str(self.data)})
        patcher2.start()
        self.addCleanup(patcher2.stop)


def node(session_id: str, node_id: int, ts: float, obj: dict):
    return (session_id, node_id, ts, json.dumps(obj))


def user_msg(session_id: str, node_id: int, ts: float, text: str):
    return node(session_id, node_id, ts,
                {'role': 'user', 'content': text, 'metadata': {'is_user_input': True}})


def make_db(path: Path, rows, nodes, schema=SCHEMA):
    path.parent.mkdir(parents=True, exist_ok=True)
    con = sqlite3.connect(path)
    con.executescript(schema)
    con.executemany('INSERT INTO sessions (id, working_directory, title, model, created_at, '
                    'last_activity_at, hidden) VALUES (?,?,?,?,?,?,?)', rows)
    if nodes:
        con.executemany('INSERT INTO message_nodes (session_id, node_id, created_at, chat_message) '
                        'VALUES (?,?,?,?)', nodes)
    con.commit()
    con.close()


def transcript(path: Path, sid: str, steps, **extra):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({'session_id': sid, 'agent': {'model_name': 'swe-2-max'},
                                'steps': steps, **extra}), encoding='utf-8')


class DevinDb(TempHome):
    def setUp(self):
        super().setUp()
        now = time.time()
        self.db = self.data / 'sessions.db'
        make_db(self.db, [
            ('d-1', '/work/api', 'Ship the adapter', 'swe-2-max', now - 600, now - 60, 0),
            ('d-hidden', '/work/x', 'secret', 'swe-2-max', now - 900, now - 30, 1),
            ('d-old', '/work/x', 'old', 'swe-2-max', now - 10 * 86400, now - 10 * 86400, 0),
            ('d-quiet', '', '', 'swe-2-max', now - 100, now - 90, 0),
        ], [
            user_msg('d-1', 1, now - 590, 'add a devin adapter to everett'),
            node('d-1', 2, now - 580, {'role': 'assistant', 'content': 'on it'}),
            user_msg('d-1', 3, now - 70, 'and keep it read-only'),
            node('d-1', 4, now - 60, {'metadata': {'request_id': 'r1'}, 'tool_calls': []}),
            user_msg('d-quiet', 10, now - 95, '<system_reminder>injected</system_reminder>'),
            user_msg('d-quiet', 11, now - 92, 'real ask'),
        ])

    def test_lists_sessions_read_only(self):
        before = self.db.read_bytes()
        found = {s.id: s for s in devin.scan(72)}
        self.assertEqual(set(found), {'d-1', 'd-quiet'})
        s = found['d-1']
        self.assertEqual((s.harness, s.cwd, s.title, s.source), ('devin', '/work/api', 'Ship the adapter', 'cli'))
        self.assertEqual((s.first_user, s.last_user), ('add a devin adapter to everett', 'and keep it read-only'))
        self.assertEqual(found['d-quiet'].first_user, 'real ask')  # injected prompt skipped
        self.assertEqual(self.db.read_bytes(), before)

    def test_connection_is_read_only(self):
        con = devin._connect(self.db)
        with self.assertRaises(sqlite3.OperationalError):
            con.execute("INSERT INTO sessions (id) VALUES ('x')")
        con.close()

    def test_hours_window_filters(self):
        self.assertEqual({s.id for s in devin.scan(1)}, {'d-1', 'd-quiet'})
        self.assertEqual(devin.scan(0), [])

    def test_resume_spawn_and_busy(self):
        s = devin.scan(72)[0]
        self.assertEqual(command_for(s, 'go'), ['devin', '--resume', s.id, '--print', 'go'])
        self.assertIn('devin --resume', resume_command(s, 'go'))
        self.assertEqual(spawn_command('devin', 'new'), ['devin', '-p', 'new'])
        db_backed = Session('devin', 'd', '', str(self.db), '', time.time() - 3600)
        os.utime(self.db)  # other sessions keep writing the shared db
        self.assertFalse(is_busy(db_backed, '', time.time()))

    def test_iso_and_ms_timestamps(self):
        now = time.time()
        alt = self.data / 'alt/sessions.db'
        make_db(alt, [('iso-1', '/w', 'iso times', 'm', '2026-10-01T00:00:00Z',
                       '2026-10-04T00:00:00+00:00', 0)], [])
        with mock.patch('everett.adapters.devin.time.time', return_value=1791158400):
            sessions, _ = devin.read_db(alt, 72)
        self.assertEqual(sessions[0].title, 'iso times')
        self.assertTrue(sessions[0].started.startswith('2026-10-01'))
        self.assertAlmostEqual(devin._epoch(now * 1000), now, places=3)
        self.assertEqual(devin._epoch('not a time'), 0.0)

    def test_large_db_keeps_recent_sessions(self):
        now = time.time()
        rows = [(f'bulk-{i}', '/w', 'bulk', 'm', now - 200 * i - 10000, now - 200 * i - 10000, 0)
                for i in range(500)]
        rows.append(('d-newest', '/work/fresh', 'newest', 'm', now - 10000, now - 10, 0))
        make_db(self.data / 'big' / 'sessions.db', rows, [])
        sessions, _ = devin.read_db(self.data / 'big' / 'sessions.db', 72)
        self.assertEqual(len(sessions), 400)
        self.assertIn('d-newest', {s.id for s in sessions})  # LIMIT must not drop the recent rows

    def test_broken_db_warns_and_transcripts_still_scan(self):
        self.db.write_text('not sqlite')
        transcript(self.data / 'transcripts/t-9.json', 't-9', [
            {'source': 'user', 'content': 'fix the flaky test', 'metadata': {'created_at': time.time()}}])
        with contextlib.redirect_stderr(io.StringIO()) as err:
            found = {s.id: s for s in devin.scan(72)}
        self.assertIn('skip', err.getvalue())
        self.assertIn('t-9', found)


class DevinTranscripts(TempHome):
    def setUp(self):
        super().setUp()
        now = time.time()
        transcript(self.data / 'transcripts/t-1.json', 't-1', [
            {'source': 'user', 'content': 'redo the hero', 'metadata': {'created_at': now - 500}},
            {'source': 'agent', 'content': 'done', 'metadata': {'created_at': now - 400}},
            {'source': 'user', 'content': [{'type': 'text', 'text': 'make it wider'}],
             'metadata': {'created_at': now - 300}},
        ], title='Landing page')
        transcript(self.data / 'transcripts/t-old.json', 't-old', [
            {'source': 'user', 'content': 'old', 'metadata': {'created_at': now - 10 * 86400}}])
        (self.data / 'transcripts/bad.json').write_text('{oops', encoding='utf-8')

    def test_transcripts_list_without_db(self):
        found = {s.id: s for s in devin.scan(72)}
        self.assertEqual(set(found), {'t-1'})
        s = found['t-1']
        self.assertEqual((s.harness, s.title, s.first_user, s.last_user),
                         ('devin', 'Landing page', 'redo the hero', 'make it wider'))
        self.assertTrue(s.path.endswith('t-1.json'))
        self.assertTrue(s.started)

    def test_db_without_nodes_enriches_transcripts(self):
        now = time.time()
        make_db(self.data / 'sessions.db',
                [('t-1', '/work/site', '', 'swe-2-max', now - 500, now - 200, 0),
                 ('d-only', '/work/api', 'db only', 'm', now - 100, now - 50, 0)], [],
                schema=SCHEMA_NO_NODES)
        found = {s.id: s for s in devin.scan(72)}
        s = found['t-1']  # db metadata wins, transcript fills the messages
        self.assertEqual(s.cwd, '/work/site')
        self.assertEqual((s.first_user, s.last_user), ('redo the hero', 'make it wider'))
        self.assertIn('d-only', found)


class DevinDirs(TempHome):
    def test_devin_home_override_and_default(self):
        self.assertEqual(devin.data_dir(), self.data)
        os.environ.pop('DEVIN_HOME')  # setUp's patch restores it for the next test
        self.assertIn('devin/cli', str(devin.data_dir()))
        self.assertEqual(devin.data_dir(), self.home / devin.DEFAULT_REL)

    def test_scan_merges_candidate_dirs(self):
        now = time.time()
        other = self.home / 'other-devin-home'
        with mock.patch.dict(os.environ, {'DEVIN_HOME': str(other)}):
            transcript(other / 'transcripts/t-a.json', 't-a', [
                {'source': 'user', 'content': 'override store', 'metadata': {'created_at': now}}])
            transcript(self.data / 'transcripts/t-b.json', 't-b', [
                {'source': 'user', 'content': 'default store', 'metadata': {'created_at': now}}])
            self.assertEqual({s.id for s in devin.scan(72)}, {'t-a', 't-b'})

    def test_registry_includes_devin(self):
        self.assertIn('devin', registry.ADAPTERS)
        now = time.time()
        transcript(self.data / 'transcripts/t-reg.json', 't-reg', [
            {'source': 'user', 'content': 'registry check', 'metadata': {'created_at': now}}])
        found = registry.scan(72, limit=None, harness='devin')
        self.assertEqual([s.id for s in found], ['t-reg'])
        self.assertEqual(found[0].harness, 'devin')

    def test_send_validation_and_lookup(self):
        with self.assertRaises(SendError):
            command_for(Session('devin', '', '', '', '', 0), 'go')
        now = time.time()
        transcript(self.data / 'transcripts/t-find.json', 't-find', [
            {'source': 'user', 'content': 'find me', 'metadata': {'created_at': now}}], title='Findable')
        self.assertEqual(registry.find('t-find', registry.scan(72, limit=None, harness='devin')).id, 't-find')


if __name__ == '__main__':
    unittest.main()
