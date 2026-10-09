import sandbox  # noqa: F401

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from dataclasses import asdict
from pathlib import Path
from unittest import mock

from everett import external, inbox, registry, route, send
from everett.adapters import external as adapter

REPO = Path(__file__).resolve().parents[1]


class ExternalSessions(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        self.bin = self.home / 'bin'
        self.bin.mkdir()
        self.env = {'HOME': str(self.home), 'EVERETT_HOME': str(self.home),
                    'EVERETT_NOTIFY': 'none', 'EVERETT_ROUTER': 'local',
                    'PYTHONPATH': str(REPO), 'PATH': str(self.bin),
                    'EVERETT_SESSION_ID': 'fixture-owner', 'EVERETT_HARNESS_NAME': ''}
        patch = mock.patch.dict(os.environ, self.env, clear=True)
        patch.start()
        self.addCleanup(patch.stop)
        for harness in ('grok', 'omp', 'claude', 'codex'):
            binary = self.bin / harness
            binary.write_text(f'#!/bin/sh\necho invoked > "{self.home / "provider-invoked"}"\nexit 91\n')
            binary.chmod(0o755)

    def record(self, **changes):
        return {'id': 'ext-grok', 'harness': 'grok-bot', 'title': 'Database backup reports',
                'cwd': '/work/backups', 'updated': 0, **changes}

    def plant(self, record, filename=None):
        external.directory().mkdir(parents=True, exist_ok=True)
        file = external.directory() / (filename or f'{record["id"]}.json')
        file.write_text(json.dumps(record))
        return file

    def module(self, *args):
        return subprocess.run([sys.executable, '-m', 'everett.external', *args],
                              cwd=self.home, env=self.env, capture_output=True, text=True, timeout=15)

    def mcp(self, calls, caller='fixture-owner', harness=''):
        requests = [{'jsonrpc': '2.0', 'id': 0, 'method': 'initialize', 'params': {
            'protocolVersion': '2024-11-05', 'capabilities': {},
            'clientInfo': {'name': 'external-fixture', 'version': '1'}}}]
        requests.extend({'jsonrpc': '2.0', 'id': i, 'method': 'tools/call',
                         'params': {'name': name, 'arguments': args}}
                        for i, (name, args) in enumerate(calls, 1))
        result = subprocess.run([sys.executable, '-m', 'everett', 'mcp'],
                                input=''.join(json.dumps(request) + '\n' for request in requests),
                                cwd=self.home, env={**self.env, 'EVERETT_SESSION_ID': caller,
                                                   'EVERETT_HARNESS_NAME': harness},
                                capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        responses = {response['id']: response for response in map(json.loads, result.stdout.splitlines())}
        return [responses[i]['result'] for i in range(1, len(calls) + 1)]

    def data(self, result):
        self.assertFalse(result.get('isError'), result)
        return result['structuredContent']

    def test_registration_roundtrip_and_explicit_lifecycle(self):
        record = external.register('ext-grok', 'grok-bot', 'Backup reports', '/work/backups')
        self.assertEqual(external.records(), [record])
        self.assertEqual(external.load(external.path(record.id)), record)
        self.assertEqual(set(asdict(record)), {'id', 'harness', 'title', 'cwd', 'updated'})
        self.assertEqual(external.path(record.id).stat().st_mode & 0o077, 0)
        with self.assertRaises(FileExistsError):
            external.register(record.id, 'grok-bot', 'Unexpected replacement')
        with self.assertRaisesRegex(ValueError, 'changing its harness'):
            external.register(record.id, 'openai-dot', replace=True)
        updated = external.register(record.id, 'grok-bot', 'New scope', replace=True)
        self.assertEqual(external.records(), [updated])
        self.assertGreaterEqual(updated.updated, record.updated)
        message = inbox.post(record.id, 'Pending task')
        external.remove(record.id)
        self.assertEqual(external.records(), [])
        self.assertEqual(inbox.pending(record.id)[0]['id'], message['id'])
        external.register(record.id, 'grok-bot')
        self.assertEqual(inbox.pending(record.id)[0]['id'], message['id'])

    def test_registration_module_add_list_replace_remove(self):
        result = self.module('add', '--id', 'ext-dot', '--harness', 'openai-dot', '--title', 'Owner dot')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['id'], 'ext-dot')
        self.assertEqual(json.loads(self.module('list').stdout)[0]['harness'], 'openai-dot')
        self.assertEqual(self.module('add', '--id', 'ext-dot', '--harness', 'openai-dot').returncode, 2)
        result = self.module('add', '--id', 'ext-dot', '--harness', 'openai-dot', '--replace',
                             '--title', 'Changed scope')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['title'], 'Changed scope')
        self.assertEqual(self.module('remove', '--id', 'ext-dot').returncode, 0)
        self.assertEqual(json.loads(self.module('list').stdout), [])

    def test_only_safe_ids_and_supported_harnesses_are_registered(self):
        for identifier in ('', '../escape', 'dots.with.periods', 'a/b', 'grok bot', 'グロク',
                           'é', 'a' * 101, 'human', 'HUMAN', 'live', '.', '..'):
            with self.subTest(identifier=identifier), self.assertRaises(ValueError):
                external.register(identifier, 'grok-bot')
        for identifier in ('ext-grok', 'grok-wright', '_', 'A' * 100):
            self.assertTrue(external.valid_id(identifier))
        with self.assertRaises(ValueError):
            external.register('ext-grok', 'grok')
        self.assertFalse(external.directory().exists())

    def test_scanning_skips_invalid_records_and_symlinks(self):
        self.plant(self.record())
        bad_records = [self.record(id='../escape'), self.record(harness='claude'),
                       self.record(title=None), self.record(cwd=[]), self.record(updated=True),
                       self.record(updated=-1), self.record(updated=float('nan')),
                       self.record(updated=float('inf')), self.record(updated=10 ** 400)]
        for index, record in enumerate(bad_records):
            self.plant(record, f'invalid-{index}.json')
        self.plant(self.record(), 'wrong-id.json')
        (external.directory() / 'broken.json').write_text('{')
        target = self.home / 'target.json'
        target.write_text(json.dumps(self.record(id='ext-linked')))
        link = external.directory() / 'ext-linked.json'
        link.symlink_to(target)
        with self.assertRaisesRegex(ValueError, 'regular file'):
            external.load(link)
        self.assertEqual([record.id for record in external.records()], ['ext-grok'])
        for record in bad_records:
            with self.subTest(record=record), self.assertRaises(ValueError):
                external.Registration.from_dict(record)

    def test_registered_sessions_persist_outside_the_transcript_window(self):
        self.plant(self.record())
        self.plant(self.record(id='ext-dot', harness='openai-dot', updated=time.time()))
        sessions = registry.scan(since_hours=0, limit=None)
        self.assertEqual({session.id for session in sessions}, {'ext-grok', 'ext-dot'})
        self.assertEqual([session.id for session in registry.scan(0, harness='grok-bot')], ['ext-grok'])
        self.assertEqual(len(registry.scan(0, limit=0)), 2)
        for session in sessions:
            self.assertEqual(session.source, 'external')
            self.assertEqual(session.path, '')
            self.assertEqual(session.started, '')
            self.assertEqual(session.first_user, '')
            self.assertFalse(session.running)
            self.assertFalse(registry.session_running(session, ps_out=session.id, now=time.time()))

    def test_external_routes_queue_and_never_resume_or_spawn(self):
        for harness in external.HARNESSES:
            self.plant(self.record(harness=harness))
            session = adapter.scan()[0]
            result = route.route('Database backup reports', [session], router='local')
            self.assertEqual(result['decision'], 'SESSION')
            self.assertEqual(result['session']['id'], session.id)
            self.assertIn('--mode inbox', result['command'])
            self.assertNotIn('--resume', result['command'])
            for mode in ('auto', 'inbox'):
                self.assertEqual(send.delivery_mode(session, mode, ps_out=''), 'inbox')
            with self.assertRaisesRegex(send.SendError, 'inbox-only'):
                send.delivery_mode(session, 'resume', ps_out=session.id)
            with self.assertRaisesRegex(send.SendError, 'inbox-only'):
                send.send(session, 'Task', wait=0)
            with self.assertRaisesRegex(send.SendError, 'cannot spawn'):
                send.spawn(harness, 'Task', str(self.home))
        self.assertFalse((self.home / 'provider-invoked').exists())

    def test_stdio_mcp_lists_queues_polls_and_replies_without_provider_cli(self):
        for harness in external.HARNESSES:
            with self.subTest(harness=harness):
                identifier = 'ext-' + harness
                self.plant(self.record(id=identifier, harness=harness))
                listing, queued, refused = self.mcp([
                    ('everett_ls', {'hours': 0, 'harness': harness}),
                    ('everett_send', {'to': identifier, 'text': 'Report backups', 'mode': 'auto'}),
                    ('everett_send', {'to': identifier, 'text': 'Do not resume', 'mode': 'resume'}),
                ])
                self.assertEqual(self.data(listing)['sessions'][0]['id'], identifier)
                queue = self.data(queued)
                self.assertEqual(queue['mode'], 'inbox')
                self.assertTrue(queue['queued'])
                self.assertFalse(queue['hooked'])
                self.assertIn('No provider wake-up or consumption is confirmed', queue['note'])
                self.assertTrue(refused['isError'], refused)
                self.assertIn('inbox-only', refused['content'][0]['text'])
                mid = queue['message_id']
                peek, polled, answered, empty = self.mcp([
                    ('everett_inbox', {'peek': True}),
                    ('everett_inbox', {}),
                    ('everett_send', {'text': 'Backups complete', 'reply_to': mid}),
                    ('everett_inbox', {}),
                ], caller=identifier, harness=harness)
                self.assertEqual(self.data(peek)['messages'][0]['id'], mid)
                self.assertEqual(self.data(polled)['messages'][0]['text'], 'Report backups')
                self.assertEqual(self.data(answered)['to'], 'fixture-owner')
                self.assertEqual(self.data(empty)['messages'], [])
                reply = self.data(self.mcp([('everett_inbox', {})])[0])['messages'][0]
                self.assertEqual(reply['from'], identifier)
                self.assertEqual(reply['from_harness'], harness)
                self.assertEqual(reply['reply_to'], mid)
                self.assertEqual(reply['text'], 'Backups complete')
        self.assertFalse((self.home / 'provider-invoked').exists())


if __name__ == '__main__':
    unittest.main()
