import sandbox  # noqa: F401  (must be first: isolates HOME)

import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from everett import events, inbox, mcp
from everett.cli import main
from everett.hooks.common import stop_event
from everett.session import Session

REPO = Path(__file__).resolve().parents[1]


class Base(unittest.TestCase):
    def setUp(self):
        self.home = Path(tempfile.mkdtemp(prefix='everett-events-'))
        patch = mock.patch.dict(os.environ, {'HOME': str(self.home), 'EVERETT_HOME': str(self.home),
                                             'EVERETT_NOTIFY': 'none'})
        patch.start()
        self.addCleanup(patch.stop)
        self.addCleanup(shutil.rmtree, self.home, True)
        for name in ('EVERETT_SESSION_ID', 'CLAUDE_CODE_SESSION_ID', 'EVERETT_SEND', 'EVERETT_NOTIFY_COMMAND'):
            os.environ.pop(name, None)
        self.project = self.home / 'kairos'
        self.project.mkdir()
        self.calls = []

    def fake(self, argv, env):  # the fake notifier: records, never runs anything
        self.calls.append((argv, env))

    def record(self, kind, text, session='s1', **kw):
        return events.record(kind, text, session=session, cwd=str(self.project), harness='claude',
                             notifier=self.fake, **kw)


class Classify(unittest.TestCase):
    def test_rules(self):
        cases = [
            ('All tests pass. Committed as abc123.', 'done'),
            ('Done. Next: ship it.', 'done'),
            ('I added the flag.\n\nShould I also update the README?', 'needs-input'),
            ('Which option do you prefer: A or B?', 'needs-input'),
            ('The migration is ready. I need you to run it against prod.', 'needs-input'),
            ('Let me know if you want the retry cap raised.', 'needs-input'),
            ("I'm blocked: the Max prompt is missing.", 'blocked'),
            ('Waiting on the staging credentials from Wright.', 'blocked'),
            ("I can't proceed until the API key is rotated.", 'blocked'),
            ('The deploy is no longer blocked; it finished.', 'done'),
            ('The job is not blocked anymore. Shipped.', 'done'),
            ('```\nwhat is this?\nblocked\n```\nFixed the parser.', 'done'),
            ('Is it blocked? No: it shipped fine.', 'done'),
        ]
        for text, kind in cases:
            self.assertEqual(events.classify(text)[0], kind, text)
        self.assertIsNone(events.classify(''))
        self.assertIsNone(events.classify('```\ncode only\n```'))
        self.assertEqual(events.classify("Everything else works. I'm blocked on the Max prompt.")[1],
                         "I'm blocked on the Max prompt.")


class Record(Base):
    def test_event_log_state_and_describe(self):
        e = self.record('blocked', 'waiting on Max prompt')
        self.assertEqual((e['kind'], e['project'], e['session']), ('blocked', 'kairos', 's1'))
        self.assertEqual(events.read()[0]['id'], e['id'])
        st = events.state('s1')
        self.assertEqual(st['since'], e['ts'])
        later = self.record('blocked', 'still waiting on Max prompt')
        self.assertEqual(events.state('s1')['since'], e['ts'])  # the same state keeps its start
        self.assertTrue(events.describe(events.state('s1'), now=e['ts'] + 32 * 3600).startswith('blocked 32h: '))
        self.record('done', 'shipped')
        self.assertEqual(events.state('s1')['kind'], 'done')
        self.assertTrue(later)

    def test_validation(self):
        for kind, text in (('nope', 'x'), ('info', '  '), ('info', 'token = sk-ant-abcdefghijklmnopqrstu')):
            with self.assertRaises(events.EventError):
                self.record(kind, text)

    def test_auto_debounce(self):
        self.assertTrue(self.record('done', 'finished A', source='auto'))
        self.assertIsNone(self.record('done', 'finished B', source='auto'))  # same state within 10 min
        self.assertIsNone(self.record('done', 'finished A', source='auto', now=time.time() + 3600))  # same text
        self.assertTrue(self.record('done', 'finished C', source='auto', now=time.time() + 3600))
        self.assertTrue(self.record('needs-input', 'Ship it?', source='auto'))  # a change of state always counts
        self.assertEqual(len(events.read()), 3)

    def test_human_notifier_only_for_blocked_and_needs_input(self):
        self.record('done', 'ok')
        self.record('info', 'fyi')
        self.assertEqual(self.calls, [])
        with mock.patch.dict(os.environ, {'EVERETT_NOTIFY': 'both', 'EVERETT_NOTIFY_COMMAND': 'max-notify "$1"'}), \
                mock.patch('everett.events.sys.platform', 'darwin'):
            self.record('needs-input', 'Should I "deploy"?')
        (cmd, env), (osa, _) = self.calls
        self.assertEqual(cmd[:3], ['/bin/sh', '-c', 'max-notify "$1"'])
        self.assertIn('needs-input [kairos] Should I "deploy"?', cmd[4])
        self.assertEqual(env['EVERETT_EVENT_KIND'], 'needs-input')
        self.assertEqual(osa[0], 'osascript')
        self.assertIn('\\"deploy\\"', osa[2])

    def test_notify_modes(self):
        e = {'kind': 'blocked', 'text': 't', 'session': 's', 'project': 'p'}
        with mock.patch('everett.events.sys.platform', 'darwin'):
            with mock.patch.dict(os.environ, {'EVERETT_NOTIFY': 'none'}):
                self.assertEqual(events.notify(e, runner=self.fake), [])
            os.environ.pop('EVERETT_NOTIFY')
            self.assertEqual([a[0] for a in events.notify(e, runner=self.fake)], ['osascript'])  # the default
            with mock.patch.dict(os.environ, {'EVERETT_NOTIFY_COMMAND': 'x'}):
                self.assertEqual(len(events.notify(e, runner=self.fake)), 2)
                with mock.patch.dict(os.environ, {'EVERETT_NOTIFY': 'command'}):
                    self.assertEqual([a[0] for a in events.notify(e, runner=self.fake)], ['/bin/sh'])
            cfg = self.home / '.everett' / 'config.toml'
            cfg.parent.mkdir(parents=True, exist_ok=True)
            cfg.write_text('notify = "none"\n')
            self.assertEqual(events.notify(e, runner=self.fake), [])

    def test_real_notify_command_runs_detached(self):
        out = self.home / 'notified.txt'
        with mock.patch.dict(os.environ, {'EVERETT_NOTIFY': 'command',
                                          'EVERETT_NOTIFY_COMMAND': f'printf "%s|$EVERETT_EVENT_KIND" "$1" > {out}'}):
            events.record('blocked', 'need creds', session='s9', cwd=str(self.project))
        for _ in range(50):
            if out.exists() and out.read_text():
                break
            time.sleep(0.05)
        self.assertEqual(out.read_text(), 'blocked [kairos] need creds|blocked')

    def test_escalation_once_after_threshold(self):
        start = time.time()
        self.record('blocked', 'waiting on Max prompt', now=start)
        self.calls.clear()
        with mock.patch.dict(os.environ, {'EVERETT_ESCALATE_MINUTES': '30', 'EVERETT_NOTIFY': 'command',
                                          'EVERETT_NOTIFY_COMMAND': 'x'}):
            self.assertEqual(events.check_escalations(now=start + 10 * 60, runner=self.fake), [])
            done = events.check_escalations(now=start + 31 * 60, runner=self.fake)
            self.assertEqual(len(done), 1)
            self.assertEqual(len(self.calls), 1)
            self.assertIn('blocked for 31 min', self.calls[0][0][4])
            self.assertEqual(events.check_escalations(now=start + 90 * 60, runner=self.fake), [])
        self.assertTrue(events.state('s1')['escalated'])
        self.record('done', 'unblocked, shipped')
        self.assertFalse(events.state('s1')['escalated'])

    def test_escalation_reason_reaches_command(self):
        start = time.time()
        with mock.patch.dict(os.environ, {'EVERETT_NOTIFY': 'command', 'EVERETT_NOTIFY_COMMAND': 'x'}):
            self.record('needs-input', 'Ship it?', now=start)
            self.calls.clear()
            events.check_escalations(now=start + 3600, runner=self.fake)
        argv, env = self.calls[0]
        self.assertTrue(argv[4].startswith('needs-input for 60 min: needs-input [kairos]'))
        self.assertEqual(env['EVERETT_EVENT_REASON'], 'needs-input for 60 min')

    def test_maybe_escalate_is_throttled(self):
        self.record('blocked', 'x', now=time.time() - 7200)
        with mock.patch('everett.events.check_escalations') as check:
            events.maybe_escalate()
            events.maybe_escalate()
        self.assertEqual(check.call_count, 1)


class Subscriptions(Base):
    def test_session_and_project_subscribers_get_inbox_events(self):
        events.subscribe('watcher', 's1')
        events.subscribe('pm', 'project:kairos')
        events.subscribe('other', 'project:web')
        events.subscribe('s1', '*')  # never gets its own events
        e = self.record('done', 'shipped retries')
        self.assertEqual(inbox.pending('watcher')[0]['kind'], 'event')
        self.assertIn('DONE from claude s1 in kairos: shipped retries', inbox.pending('watcher')[0]['text'])
        self.assertEqual(len(inbox.pending('pm')), 1)
        self.assertEqual(inbox.pending('other'), [])
        self.assertEqual(inbox.pending('s1'), [])
        self.assertIn('EVENT', inbox.take('pm'))
        self.assertEqual(events.subscribe('watcher', 's1', remove=True), [])
        self.assertNotIn('watcher', events.subscriptions())
        self.assertTrue(e)
        with self.assertRaises(events.EventError):
            events.subscribe('', 's1')

    def test_resolve_target(self):
        s = Session('claude', 'abcdef-123', str(self.project), '', '', time.time())
        with mock.patch('everett.registry.scan', return_value=[s]):
            self.assertEqual(events.resolve_target('abcdef'), 'abcdef-123')
            self.assertEqual(events.resolve_target('Web App'), 'project:web-app')
        self.assertEqual(events.resolve_target('project:Kairos'), 'project:kairos')
        self.assertEqual(events.resolve_target('*'), '*')


class StopHook(Base):
    def test_auto_detect_from_hook_input_and_transcript(self):
        stop_event(json.dumps({'session_id': 'c1', 'cwd': str(self.project),
                               'last_assistant_message': 'Tests pass. Should I push the branch?'}), 'claude')
        self.assertEqual(events.state('c1')['kind'], 'needs-input')
        self.assertEqual(events.state('c1')['source'], 'auto')
        transcript = self.home / 't.jsonl'
        transcript.write_text(json.dumps({'type': 'assistant', 'message': {'role': 'assistant', 'content': [
            {'type': 'text', 'text': "I'm blocked: waiting on the Max prompt."}]}}) + '\n')
        stop_event(json.dumps({'session_id': 'c1', 'cwd': str(self.project), 'transcript_path': str(transcript)}),
                   'claude')
        self.assertEqual(events.state('c1')['kind'], 'blocked')
        self.assertEqual(inbox.live('c1')['state'], 'idle') if inbox.live('c1') else None

    def test_silent_cases(self):
        for raw in ('junk', '[]', json.dumps({'session_id': '../x', 'last_assistant_message': 'done'}),
                    json.dumps({'session_id': 'c2', 'stop_hook_active': True, 'last_assistant_message': 'Ok?'})):
            stop_event(raw, 'claude')
        with mock.patch.dict(os.environ, {'EVERETT_SEND': '1'}):
            stop_event(json.dumps({'session_id': 'c2', 'last_assistant_message': 'Ok?'}), 'claude')
        self.assertEqual(events.read(), [])

    def test_stop_script_records_event(self):
        env = {**os.environ, 'PATH': '/usr/bin:/bin', 'EVERETT_NOTIFY': 'none'}
        payload = json.dumps({'session_id': 'x1', 'cwd': str(self.project), 'hook_event_name': 'Stop',
                              'stop_hook_active': False, 'last_assistant_message': 'Deployed to staging.',
                              'transcript_path': None, 'turn_id': 't', 'model': 'm', 'permission_mode': 'default'})
        r = subprocess.run([sys.executable, str(REPO / 'everett/hooks/codex_stop.py')], input=payload,
                           capture_output=True, text=True, env=env, timeout=10)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, '', ''))
        self.assertEqual(events.state('x1')['kind'], 'done')

    def test_grok_stop_uses_camel_case(self):
        from everett.hooks.common import grok_stop_hook
        grok_stop_hook(json.dumps({'sessionId': 'g1', 'cwd': str(self.project),
                                   'lastAssistantMessage': 'Need you to approve the plan.'}))
        self.assertEqual(events.state('g1')['kind'], 'needs-input')


class Surfaces(Base):
    def test_cli_event_events_subscribe_and_ls_state(self):
        out = io.StringIO()
        with mock.patch.dict(os.environ, {'EVERETT_SESSION_ID': 'sess-1'}), contextlib.redirect_stdout(out):
            self.assertEqual(main(['event', 'blocked', 'waiting on Max prompt', '--project', 'kairos']), 0)
        self.assertIn('blocked [kairos] waiting on Max prompt', out.getvalue())
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(main(['events', '--since', '1h']), 0)
        self.assertIn('waiting on Max prompt', out.getvalue())
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main(['events', '--json'])
        self.assertEqual(json.loads(out.getvalue())[0]['kind'], 'blocked')
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(main(['events', '--since', 'soon']), 2)
            self.assertEqual(main(['subscribe', 'kairos']), 2)  # no session to subscribe
        out = io.StringIO()
        with mock.patch('everett.registry.scan', return_value=[]), contextlib.redirect_stdout(out):
            self.assertEqual(main(['subscribe', 'kairos', '--as', 'me-1']), 0)
        self.assertEqual(events.subscriptions(), {'me-1': ['project:kairos']})
        s = Session('claude', 'sess-1', str(self.project), '', '', time.time(), card='Kairos: retries')
        events.apply([s])
        self.assertTrue(s.state.startswith('blocked 0m: waiting on Max prompt'))
        out = io.StringIO()
        with mock.patch('everett.cli.registry.scan', return_value=[s]), contextlib.redirect_stdout(out):
            main(['ls'])
        self.assertIn('⚠ blocked 0m: waiting on Max prompt · Kairos: retries', out.getvalue())

    def test_mcp_tools(self):
        with mock.patch.dict(os.environ, {'EVERETT_SESSION_ID': 'mcp-1'}):
            r = mcp.call_tool({'name': 'everett_event', 'arguments': {'kind': 'needs-input', 'message': 'Ship?'}})
            self.assertFalse(r['isError'])
            self.assertEqual(r['structuredContent']['event']['session'], 'mcp-1')
            with mock.patch('everett.registry.scan', return_value=[]):
                r = mcp.call_tool({'name': 'everett_subscribe', 'arguments': {'target': 'project:web'}})
            self.assertEqual(r['structuredContent']['following'], ['project:web'])
            r = mcp.call_tool({'name': 'everett_event', 'arguments': {'kind': 'bogus', 'message': 'x'}})
            self.assertTrue(r['isError'])
        s = Session('claude', 'mcp-1', str(self.project), '', '', time.time())
        events.apply([s])
        self.assertEqual(mcp._brief(s)['state_kind'], 'needs-input')


if __name__ == '__main__':
    unittest.main()
