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
from urllib.parse import quote

from everett import doctor, install, registry
from everett.adapters import grok, t3code
from everett.cards import AUTO_MARKER, card_path
from everett.hooks.common import grok_stop_hook
from everett.route import resume_command
from everett.send import SendError, command_for, is_busy, spawn, spawn_command
from everett.session import Session


class TempHome(unittest.TestCase):
    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.home = Path(self._dir.name)
        patcher = mock.patch.dict(os.environ, {'HOME': str(self.home), 'EVERETT_HOME': str(self.home)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self._dir.cleanup)


def jsonl(path: Path, rows) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(''.join(json.dumps(r) + '\n' for r in rows), encoding='utf-8')


def grok_session(home: Path, sid: str, cwd: str, *, prompts=(), history=None, summary=None) -> Path:
    folder = home / '.grok/sessions' / quote(cwd, safe='') / sid
    folder.mkdir(parents=True, exist_ok=True)
    meta = {'info': {'id': sid, 'cwd': cwd}, 'created_at': '2026-09-24T00:00:00Z',
            'last_active_at': '2026-09-24T00:05:00Z', 'agent_name': 'grok-build-plan'}
    meta.update(summary or {})
    (folder / 'summary.json').write_text(json.dumps(meta), encoding='utf-8')
    rows = history if history is not None else [
        {'type': 'system', 'content': 'You are Grok.'},
        {'type': 'user', 'content': [{'type': 'text', 'text': '<user_info>\nOS Version: macos\n</user_info>'}]},
        {'type': 'user', 'synthetic_reason': 'system_reminder',
         'content': [{'type': 'text', 'text': '<system-reminder>\nskills\n</system-reminder>'}]},
        *[{'type': 'user', 'prompt_index': i, 'content': [{'type': 'text', 'text': f'<user_query>\n{p}\n</user_query>'}]}
          for i, p in enumerate(prompts)],
        {'type': 'assistant', 'content': 'Done. Next: run the integration test.', 'model_id': 'grok-4.7'},
    ]
    jsonl(folder / 'chat_history.jsonl', rows)
    history_file = folder.parent / 'prompt_history.jsonl'
    with history_file.open('a', encoding='utf-8') as f:
        for p in prompts:
            f.write(json.dumps({'timestamp': '2026-09-24T00:01:00Z', 'session_id': sid, 'prompt': p,
                                'is_bash': False}) + '\n')
        f.write(json.dumps({'timestamp': '2026-09-24T00:02:00Z', 'session_id': sid, 'prompt': 'ls -la',
                            'is_bash': True}) + '\n')
    return folder


class GrokAdapter(TempHome):
    def test_lists_sessions_with_cwd_headline_and_title(self):
        grok_session(self.home, '01a0-api', '/work/api', prompts=['add retries to the upload client', 'cap at five'],
                     summary={'generated_title': 'Upload retries'})
        [s] = grok.scan(72)
        self.assertEqual((s.harness, s.id, s.cwd, s.title), ('grok', '01a0-api', '/work/api', 'Upload retries'))
        self.assertEqual((s.first_user, s.last_user), ('add retries to the upload client', 'cap at five'))
        self.assertFalse(s.auto)

    def test_cwd_decoded_from_folder_and_chat_history_fallback(self):
        folder = grok_session(self.home, '01a0-web', '/work/my site', prompts=['fix the hero'])
        (folder.parent / 'prompt_history.jsonl').unlink()  # force the chat_history path
        meta = json.loads((folder / 'summary.json').read_text())
        del meta['info']
        (folder / 'summary.json').write_text(json.dumps(meta))
        [s] = grok.scan(72)
        self.assertEqual((s.id, s.cwd, s.first_user), ('01a0-web', '/work/my site', 'fix the hero'))

    def test_empty_sessions_dropped_and_headless_is_auto(self):
        grok_session(self.home, 'empty', '/w', prompts=[])
        grok_session(self.home, 'batch', '/w', prompts=['nightly report'], summary={'session_kind': 'headless'})
        found = {s.id: s for s in grok.scan(72)}
        self.assertEqual(set(found), {'batch'})
        self.assertTrue(found['batch'].auto)
        self.assertNotIn('batch', {s.id for s in registry.scan(72)})

    def test_running_from_active_sessions_pid(self):
        grok_session(self.home, 'live', '/w', prompts=['hi'])
        grok_session(self.home, 'dead', '/w', prompts=['hi'])
        old = time.time() - 3600
        for sid in ('live', 'dead'):
            for f in (self.home / '.grok/sessions' / quote('/w', safe='') / sid).iterdir():
                os.utime(f, (old, old))
        (self.home / '.grok/active_sessions.json').write_text(json.dumps([
            {'session_id': 'live', 'pid': os.getpid(), 'cwd': '/w'},
            {'session_id': 'dead', 'pid': 2 ** 22 + 12345, 'cwd': '/w'}]))
        found = {s.id: s for s in registry.scan(72)}
        self.assertTrue(found['live'].running)
        self.assertFalse(found['dead'].running)

    def test_send_and_spawn_commands(self):
        s = Session('grok', '01a0-x', '/w', '/tmp/x', '', time.time())
        self.assertEqual(command_for(s, 'go'), ['grok', '--resume', '01a0-x', '-p', 'go'])
        self.assertEqual(spawn_command('grok', 'go', 'u-1'), ['grok', '--session-id', 'u-1', '-p', 'go'])
        self.assertIn('grok --resume 01a0-x', resume_command(s, 'go'))

    def test_spawn_preassigns_session_id(self):
        run = mock.Mock(return_value=SimpleNamespace(returncode=0, stdout='OK', stderr=''))
        with mock.patch('everett.send.subprocess.run', run):
            result = spawn('grok', 'reply OK', str(self.home))
        command = run.call_args.args[0]
        self.assertEqual(command[:2], ['grok', '--session-id'])
        self.assertEqual(result.session_id, command[2])
        self.assertEqual(result.reply, 'OK')

    def test_busy_checks_files_not_directory(self):
        folder = grok_session(self.home, 'b', '/w', prompts=['hi'])
        old = time.time() - 3600
        for f in folder.iterdir():
            os.utime(f, (old, old))
        os.utime(folder, None)  # a new file in the directory; not a turn
        s = Session('grok', 'b', '/w', str(folder), '', old)
        self.assertFalse(is_busy(s, '', time.time()))
        os.utime(folder / 'updates.jsonl' if (folder / 'updates.jsonl').exists() else folder / 'chat_history.jsonl')
        self.assertTrue(is_busy(s, '', time.time()))

    def test_stop_hook_writes_auto_card(self):
        grok_session(self.home, 'card-1', '/work/api', prompts=['add retries to the upload client'])
        grok_stop_hook(json.dumps({'hookEventName': 'stop', 'hook_event_name': 'Stop', 'sessionId': 'card-1',
                                   'cwd': '/work/api'}))
        text = card_path('card-1').read_text()
        self.assertTrue(text.startswith(AUTO_MARKER))
        self.assertIn('Api: add retries to the upload client', text)
        self.assertIn('Next: run the integration test.', text)

    def test_stop_hook_ignores_bad_input(self):
        for raw in ('not json', '[]', json.dumps({'sessionId': '../x'}), json.dumps({'sessionId': 'missing'})):
            grok_stop_hook(raw)
        self.assertFalse(card_path('missing').exists())

    def test_install_hooks_writes_grok_hook_file(self):
        self.assertEqual(install.installed('grok'), {'Stop': False, 'PostToolUse': False})
        report = install.apply('grok')
        path = self.home / '.grok/hooks/everett.json'
        self.assertIn(str(path), report)
        data = json.loads(path.read_text())
        self.assertIn('grok_stop.py', data['hooks']['Stop'][0]['hooks'][0]['command'])
        self.assertEqual(install.installed('grok'), {'Stop': True, 'PostToolUse': True})
        self.assertIn('already installed', install.apply('grok'))


T3_SCHEMA = '''
CREATE TABLE projection_threads (thread_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, title TEXT NOT NULL,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);
CREATE TABLE provider_session_runtime (thread_id TEXT PRIMARY KEY, provider_name TEXT NOT NULL,
  adapter_key TEXT NOT NULL, status TEXT NOT NULL, last_seen_at TEXT NOT NULL, resume_cursor_json TEXT,
  runtime_payload_json TEXT);
'''


class T3Code(TempHome):
    def setUp(self):
        super().setUp()
        now = time.strftime('%Y-%m-%dT%H:%M:%SZ')
        claude_rows = [{'type': 'user', 'sessionId': 'cl-1', 'cwd': '/work/api', 'timestamp': now,
                        'entrypoint': 'sdk-ts', 'message': {'role': 'user', 'content': 'add retries'}}]
        jsonl(self.home / '.claude/projects/-work-api/cl-1.jsonl', claude_rows)
        codex_rows = [
            {'type': 'session_meta', 'payload': {'id': 'cx-1', 'cwd': '/work/web', 'timestamp': now,
                                                 'originator': 't3code_desktop', 'source': 'vscode'}},
            {'type': 'response_item', 'payload': {'type': 'message', 'role': 'user',
                                                  'content': [{'type': 'input_text', 'text': 'dark mode'}]}}]
        jsonl(self.home / '.codex/sessions/2026/09/24/rollout-cx-1.jsonl', codex_rows)
        grok_session(self.home, 'gk-1', '/work/infra', prompts=['terraform bucket'])
        self.db = self.home / '.t3/userdata/state.sqlite'
        self.db.parent.mkdir(parents=True)
        con = sqlite3.connect(self.db)
        con.executescript(T3_SCHEMA)
        con.executemany('INSERT INTO projection_threads VALUES (?,?,?,?,?,?)', [
            ('t-cl', 'p', 'API retry thread', now, now, None),
            ('t-cx', 'p', 'Dark mode', now, now, None),
            ('t-gk', 'p', 'Infra bucket', now, now, None),
            ('t-gone', 'p', 'Pruned', now, now, None)])
        con.executemany('INSERT INTO provider_session_runtime VALUES (?,?,?,?,?,?,?)', [
            ('t-cl', 'claudeAgent', 'claudeAgent', 'stopped', now,
             json.dumps({'threadId': 't-cl', 'resume': 'cl-1', 'resumeSessionAt': 'm-9', 'turnCount': 1}), '{}'),
            ('t-cx', 'codex', 'codex', 'stopped', now, json.dumps({'threadId': 'cx-1'}), '{}'),
            ('t-gk', 'grok', 'grok', 'stopped', now, json.dumps({'schemaVersion': 1, 'sessionId': 'gk-1'}), '{}'),
            ('t-gone', 'codex', 'codex', 'stopped', now, json.dumps({'threadId': 'no-such-session'}), '{}')])
        con.commit()
        con.close()

    def test_threads_map_to_harness_sessions_read_only(self):
        before = self.db.read_bytes()
        found = t3code.threads()
        self.assertEqual(found[('claude', 'cl-1')]['title'], 'API retry thread')
        self.assertEqual(found[('codex', 'cx-1')]['thread_id'], 't-cx')
        self.assertIn(('grok', 'gk-1'), found)
        self.assertEqual(self.db.read_bytes(), before)
        con = t3code._connect(self.db)
        with self.assertRaises(sqlite3.OperationalError):
            con.execute("DELETE FROM projection_threads")
        con.close()

    def test_sessions_annotated_not_double_listed(self):
        sessions = registry.scan(72)
        self.assertEqual(sorted(s.id for s in sessions), ['cl-1', 'cx-1', 'gk-1'])
        self.assertTrue(all(s.source == 't3code' for s in sessions))
        by_id = {s.id: s for s in sessions}
        self.assertEqual(by_id['cl-1'].title, 'API retry thread')  # no title of its own: T3's fills in
        self.assertEqual(by_id['cx-1'].harness, 'codex')

    def test_codex_originator_marks_t3_without_db(self):
        self.db.unlink()
        by_id = {s.id: s for s in registry.scan(72)}
        self.assertEqual(by_id['cx-1'].source, 't3code')
        self.assertEqual(by_id['cl-1'].source, '')

    def test_t3_sessions_are_route_only(self):
        s = next(s for s in registry.scan(72) if s.id == 'cx-1')
        with self.assertRaises(SendError) as ctx:
            command_for(s, 'go')
        self.assertEqual(ctx.exception.code, 2)
        self.assertIn('T3 Code', str(ctx.exception))

    def test_broken_db_is_ignored(self):
        self.db.write_text('not sqlite')
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(t3code.threads(), {})
            self.assertEqual(len(registry.scan(72)), 3)

    def test_doctor_shows_grok_and_t3code_stores(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            doctor.run(72)
        text = out.getvalue()
        self.assertIn('grok    ' + str(self.home / '.grok/sessions') + ' (found)', text)
        self.assertIn('t3code  ' + str(self.db) + ' (found); 4 thread(s) with a harness session id', text)
        self.assertIn('grok    missing Stop, PostToolUse; run `everett install-hooks --grok`', text)


if __name__ == '__main__':
    unittest.main()
