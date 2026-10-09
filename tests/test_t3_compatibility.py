import sandbox  # noqa: F401

import json
import sqlite3
from datetime import datetime, timedelta, timezone
from pathlib import Path

from everett import core, inbox, registry
from everett.adapters import t3code
from everett.route import resume_command
from everett.send import SendError, command_for, delivery_mode, send_inbox
from test_grok_t3code import T3_SCHEMA, TempHome
from test_stage_c import Server


def stamp(hours=0):
    return (datetime.now(timezone.utc) - timedelta(hours=hours)).isoformat()


class T3V2(TempHome):
    def setUp(self):
        super().setUp()
        self.db = self.home / '.t3/userdata/statev2.sqlite'
        self.db.parent.mkdir(parents=True)
        self.con = sqlite3.connect(self.db)
        self.con.executescript((Path(__file__).parent / 'fixtures/t3_v2.sql').read_text())
        self.con.execute('INSERT INTO projection_projects VALUES (?,?,?,?,?,?,?,?)',
                         ('p', 'Project', '/work/project', None, '[]', stamp(), stamp(), None))
        self.addCleanup(self.con.close)

    def add(self, native='native-1', thread='app-1', driver='codex', **changes):
        updated = changes.get('updated', stamp())
        provider_id = 'provider-' + thread
        thread_payload = {'worktreePath': changes.get('worktree', '/work/tree')}
        provider_payload = {'driver': driver, 'providerInstanceId': 'custom-instance',
                            'nativeThreadRef': {'driver': driver, 'nativeId': native, 'strength': 'strong'}
                            if native is not None else None}
        self.con.execute('''INSERT INTO orchestration_v2_projection_threads
            (thread_id,project_id,title,default_provider,provider_instance_id,runtime_mode,
             interaction_mode,active_provider_thread_id,created_at,updated_at,archived_at,deleted_at,payload_json)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)''',
            (thread, 'p', 'T3 task ' + thread, 'custom-instance', 'custom-instance', 'full-access',
             'default', provider_id, updated, updated, changes.get('archived'), changes.get('deleted'),
             json.dumps(thread_payload)))
        self.con.execute('''INSERT INTO orchestration_v2_projection_provider_threads
            (provider_thread_id,thread_id,provider,driver,provider_instance_id,status,updated_at,payload_json)
            VALUES (?,?,?,?,?,?,?,?)''',
            (provider_id, thread, 'custom-instance', driver, 'custom-instance', 'idle', updated,
             json.dumps(provider_payload)))
        for index, text in enumerate(('first real request', 'latest real request')):
            self.con.execute('''INSERT INTO orchestration_v2_projection_messages
                (message_id,thread_id,role,streaming,created_at,updated_at,payload_json)
                VALUES (?,?,?,0,?,?,?)''',
                (thread + str(index), thread, 'user', updated, updated, json.dumps({'text': text})))
        self.con.commit()

    def test_missing_transcript_uses_native_ref_driver_and_messages(self):
        self.add()
        before = self.db.read_bytes()
        sessions = registry.scan(72, harness='codex')
        self.assertEqual(len(sessions), 1)
        session = sessions[0]
        self.assertEqual((session.id, session.harness, session.source), ('native-1', 'codex', 't3code'))
        self.assertEqual((session.cwd, session.first_user, session.last_user),
                         ('/work/tree', 'first real request', 'latest real request'))
        self.assertEqual(self.db.read_bytes(), before)
        with t3code._connect(self.db) as con:
            with self.assertRaises(sqlite3.OperationalError):
                con.execute('DELETE FROM orchestration_v2_projection_threads')
        self.assertEqual(registry.scan(72, harness='claude'), [])

    def test_provider_transcript_remains_authoritative_and_deduplicated(self):
        self.add()
        path = self.home / '.codex/sessions/2026/10/09/rollout-native-1.jsonl'
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({'type': 'session_meta', 'payload': {
            'id': 'native-1', 'cwd': '/work/provider', 'timestamp': stamp()}}) + '\n' +
            json.dumps({'type': 'response_item', 'payload': {'type': 'message', 'role': 'user',
                        'content': [{'type': 'input_text', 'text': 'provider request'}]}}) + '\n')
        sessions = registry.scan(72)
        self.assertEqual(len(sessions), 1)
        self.assertEqual((sessions[0].cwd, sessions[0].first_user), ('/work/provider', 'provider request'))
        self.assertEqual(sessions[0].source, 't3code')

    def test_old_deleted_archived_unsupported_and_missing_native_are_not_synthesized(self):
        self.add(thread='old', updated=stamp(100))
        self.add(thread='deleted', deleted=stamp())
        self.add(thread='archived', archived=stamp())
        self.add(thread='unsupported', driver='cursor')
        self.add(thread='empty', native=None)
        self.add(thread='valid', native='good', worktree=None)
        sessions = registry.scan(72, limit=1)
        self.assertEqual([s.id for s in sessions], ['good'])
        self.assertEqual(sessions[0].cwd, '/work/project')

    def test_malformed_native_ref_is_ignored_without_losing_other_threads(self):
        self.add(thread='bad')
        self.add(thread='valid', native='good')
        self.con.execute('UPDATE orchestration_v2_projection_provider_threads SET payload_json=? WHERE thread_id=?',
                         (json.dumps({'nativeThreadRef': {'driver': 'claudeAgent', 'nativeId': 'wrong'}}), 'bad'))
        self.con.commit()
        self.assertEqual([s.id for s in registry.scan(72)], ['good'])

    def test_current_store_is_preferred_to_stale_legacy_store(self):
        self.add()
        legacy = self.db.with_name('state.sqlite')
        with sqlite3.connect(legacy) as con:
            con.executescript(T3_SCHEMA)
            con.execute('INSERT INTO projection_threads VALUES (?,?,?,?,?,NULL)',
                         ('old-copy', 'p', 'Old copy', stamp(), stamp()))
            con.execute('INSERT INTO provider_session_runtime VALUES (?,?,?,?,?,?,?)',
                         ('old-copy', 'codex', 'codex', 'stopped', stamp(), '{"threadId":"stale"}', '{}'))
        self.assertEqual(t3code.db_path(), self.db)
        self.assertEqual([s.id for s in registry.scan(72)], ['native-1'])

    def test_t3_polling_metadata_and_route_never_resume_provider(self):
        self.add()
        session = registry.scan(72)[0]
        self.assertEqual(delivery_mode(session, 'auto', ps_out=''), 'inbox')
        with self.assertRaises(SendError):
            command_for(session, 'go')
        command = resume_command(session, 'go')
        self.assertIn('everett_inbox', command)
        self.assertNotIn('codex resume', command)
        queued = send_inbox(session, 'go', caller='sender')
        self.assertFalse(queued['hooked'])
        self.assertEqual((queued['pickup'], queued['poll_session_id']), ('poll', 'native-1'))
        self.assertEqual(inbox.pending('native-1')[0]['text'], 'go')

    def test_synthetic_mcp_poll_reply_and_shared_core_journey(self):
        self.add()
        server = Server(self.home, {'EVERETT_SESSION_ID': 'sender'})
        receiver = Server(self.home, {'EVERETT_SESSION_ID': 'native-1'})
        self.addCleanup(lambda: server.proc.poll() is None and server.proc.kill())
        self.addCleanup(lambda: receiver.proc.poll() is None and receiver.proc.kill())
        for client in (server, receiver):
            client.request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                          'clientInfo': {'name': 'synthetic-t3', 'version': '1'}})
        queued = server.call('everett_send', to='native-1', text='Review the T3 task')['structuredContent']
        self.assertEqual(queued['pickup'], 'poll')
        self.assertFalse(queued['hooked'])
        with_self = receiver.call('everett_send', to='native-1', text='loop', session_id='native-1')
        self.assertTrue(with_self['isError'])
        received = receiver.call('everett_inbox', session_id='native-1')['structuredContent']
        self.assertEqual(received['messages'][0]['id'], queued['message_id'])
        self.assertEqual(receiver.call('everett_inbox', session_id='native-1')['structuredContent']['messages'], [])
        answered = receiver.call('everett_send', reply_to=queued['message_id'], text='Reviewed',
                                 session_id='native-1')
        self.assertFalse(answered['isError'])
        response = server.call('everett_inbox', session_id='sender')['structuredContent']['messages'][0]
        self.assertEqual((response['text'], response['reply_to']), ('Reviewed', queued['message_id']))
        learned = receiver.call('everett_learn', fact='T3 messages require explicit inbox polling', project='/work/tree')
        self.assertFalse(learned['isError'])
        self.assertEqual(receiver.call('everett_core', project='/work/tree')['structuredContent']['pending_learnings'], 1)
        self.assertEqual(core.merge(llm='none')['merged'], 1)
        shared = server.call('everett_core', project='/work/tree')['structuredContent']
        self.assertEqual(shared['pending_learnings'], 0)
        self.assertIn('T3 messages require explicit inbox polling', shared['core'])
        self.assertEqual(server.close(), '')
        self.assertEqual(receiver.close(), '')


class T3Legacy(TempHome):
    def test_missing_provider_with_real_cursor_can_be_listed_and_filtered(self):
        db = self.home / '.t3/userdata/state.sqlite'
        db.parent.mkdir(parents=True)
        with sqlite3.connect(db) as con:
            con.executescript(T3_SCHEMA)
            con.execute('INSERT INTO projection_threads VALUES (?,?,?,?,?,NULL)',
                        ('legacy-app', 'p', 'Legacy task', stamp(), stamp()))
            con.execute('INSERT INTO provider_session_runtime VALUES (?,?,?,?,?,?,?)',
                        ('legacy-app', 'claudeAgent', 'custom-instance', 'stopped', stamp(),
                         '{"resume":"legacy-native"}', '{}'))
        sessions = registry.scan(72, harness='claude')
        self.assertEqual([(s.id, s.source) for s in sessions], [('legacy-native', 't3code')])
        self.assertEqual(registry.scan(72, harness='codex'), [])
