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
from dataclasses import asdict
from pathlib import Path
from unittest import mock

from everett import inbox, install
from everett.cli import main
from everett.hooks import deliver
from everett.send import SendError, delivery_mode, reply, send_inbox
from everett.session import Session

REPO = Path(__file__).resolve().parents[1]


def fresh_home():
    d = tempfile.mkdtemp(prefix='everett-live-')
    patch = mock.patch.dict(os.environ, {'HOME': d, 'EVERETT_HOME': d})
    patch.start()
    return Path(d), patch


class Base(unittest.TestCase):
    def setUp(self):
        self.home, self.env = fresh_home()
        self.addCleanup(self.env.stop)
        self.addCleanup(shutil.rmtree, self.home, True)
        for name in ('EVERETT_HOPS', 'EVERETT_SEND', 'EVERETT_SESSION_ID', 'CLAUDE_CODE_SESSION_ID'):
            os.environ.pop(name, None)

    def session(self, sid='target-1', harness='claude', **kw):
        return Session(harness=harness, id=sid, cwd=str(self.home), path=str(self.home / 'x.jsonl'),
                       started='', last_active=time.time(), **kw)


class Inbox(Base):
    def test_post_pending_take(self):
        m = inbox.post('s1', 'hello there', sender='s0', from_harness='codex', from_card='Kairos: retries')
        self.assertTrue(m['id'].startswith('m'))
        self.assertEqual([x['id'] for x in inbox.pending('s1')], [m['id']])
        text = inbox.take('s1')
        self.assertIn('[Everett]', text)
        self.assertIn('hello there', text)
        self.assertIn('codex session s0', text)
        self.assertIn('Kairos: retries', text)
        self.assertIn(f'everett_send(reply_to="{m["id"]}"', text)
        self.assertEqual(inbox.pending('s1'), [])
        self.assertEqual(inbox.take('s1'), '')

    def test_bounded_batch_leaves_rest_pending(self):
        for i in range(8):
            inbox.post('s1', f'msg {i} ' + 'x' * 3000)
        text = inbox.take('s1')
        self.assertLessEqual(len(text), inbox.MAX_INJECT + 600)
        self.assertIn('[…clipped]', text)
        self.assertIn('more waiting', text)
        self.assertTrue(inbox.pending('s1'))

    def test_expired_and_invalid(self):
        m = inbox.post('s1', 'old')
        with mock.patch('everett.inbox.time.time', return_value=time.time() + inbox.TTL + 5):
            self.assertEqual(inbox.pending('s1'), [])
        self.assertTrue(m['id'])
        with self.assertRaises(inbox.InboxError):
            inbox.post('../evil', 'x')
        with self.assertRaises(inbox.InboxError):
            inbox.post('s1', '   ')

    def test_wait_reply_consumes_only_the_reply(self):
        inbox.post('me', 'unrelated', sender='x')
        sent = inbox.post('peer', 'question', sender='me')
        inbox.post('me', 'the answer', sender='peer', kind='reply', reply_to=sent['id'])
        got = inbox.wait_reply('me', sent['id'], wait=0)
        self.assertEqual(got['text'], 'the answer')
        self.assertEqual([m['text'] for m in inbox.pending('me')], ['unrelated'])

    def test_wait_reply_times_out(self):
        clock = iter([0, 0, 1, 2, 3, 4, 5, 6]).__next__
        self.assertIsNone(inbox.wait_reply('me', 'mnope', wait=3, poll=1, clock=clock, sleep=lambda s: None))

    def test_live_record(self):
        self.assertIsNone(inbox.live('s1'))
        with mock.patch('everett.inbox.harness_pid', return_value=os.getpid()):
            inbox.touch_live('s1', 'claude')
        self.assertEqual(inbox.live('s1')['harness'], 'claude')
        with mock.patch('everett.inbox.harness_pid', return_value=os.getpid()):
            inbox.touch_live('s2', 'claude')  # same process switched sessions (/clear, /resume)
        self.assertIsNone(inbox.live('s1'))
        self.assertTrue(inbox.live('s2'))
        with mock.patch('everett.inbox.harness_pid', return_value=os.getpid()):
            inbox.touch_live('s1', 'claude')
        inbox.live_path('s1').write_text(json.dumps({'pid': 999999, 'harness': 'claude'}))
        self.assertIsNone(inbox.live('s1'))


class DeliverHook(Base):
    def hook(self, harness, payload):
        with mock.patch('everett.inbox.harness_pid', return_value=os.getpid()):
            out = deliver.output(json.dumps(payload), harness)
        return json.loads(out) if out else None

    def test_claude_user_prompt_submit_and_post_tool_use(self):
        inbox.post('c1', 'first')
        out = self.hook('claude', {'session_id': 'c1', 'hook_event_name': 'UserPromptSubmit', 'prompt': 'hi'})
        self.assertEqual(out['hookSpecificOutput']['hookEventName'], 'UserPromptSubmit')
        self.assertIn('first', out['hookSpecificOutput']['additionalContext'])
        self.assertIsNone(self.hook('claude', {'session_id': 'c1', 'hook_event_name': 'PostToolUse'}))
        inbox.post('c1', 'second')
        out = self.hook('claude', {'session_id': 'c1', 'hook_event_name': 'PostToolUse', 'tool_name': 'Bash'})
        self.assertEqual(out['hookSpecificOutput']['hookEventName'], 'PostToolUse')
        self.assertIn('second', out['hookSpecificOutput']['additionalContext'])
        self.assertTrue(inbox.live('c1'))

    def test_codex_same_contract(self):
        inbox.post('x1', 'for codex')
        out = self.hook('codex', {'session_id': 'x1', 'hook_event_name': 'PostToolUse', 'turn_id': 't',
                                  'tool_name': 'shell', 'tool_input': {}, 'tool_response': {}})
        self.assertEqual(set(out['hookSpecificOutput']), {'hookEventName', 'additionalContext'})

    def test_grok_prompt_submit_is_not_consumed(self):
        inbox.post('g1', 'for grok')
        # Grok runs ~/.claude/settings.json hooks too; its input is camelCase and discards this context.
        self.assertIsNone(self.hook('claude', {'sessionId': 'g1', 'hook_event_name': 'UserPromptSubmit'}))
        self.assertEqual(len(inbox.pending('g1')), 1)
        out = self.hook('grok', {'sessionId': 'g1', 'hook_event_name': 'PostToolUse', 'hookEventName': 'post_tool_use'})
        self.assertIn('for grok', out['hookSpecificOutput']['additionalContext'])

    def test_silent_cases(self):
        inbox.post('c1', 'queued')
        for raw in ('nope', '[]', json.dumps({'session_id': '../x', 'hook_event_name': 'PostToolUse'}),
                    json.dumps({'session_id': 'c1', 'hook_event_name': 'Stop'})):
            self.assertEqual(deliver.output(raw, 'claude'), '')
        with mock.patch.dict(os.environ, {'EVERETT_SEND': '1'}):
            self.assertEqual(deliver.output(json.dumps({'session_id': 'c1', 'hook_event_name': 'PostToolUse'}),
                                            'claude'), '')
        self.assertEqual(len(inbox.pending('c1')), 1)

    def test_hook_scripts_run_fast_and_silent(self):
        env = {**os.environ, 'PATH': '/usr/bin:/bin'}
        payload = json.dumps({'session_id': 'fast-1', 'hook_event_name': 'PostToolUse'})
        for script in ('claude_inbox.py', 'codex_inbox.py', 'grok_inbox.py'):
            started = time.time()
            result = subprocess.run([sys.executable, str(REPO / 'everett' / 'hooks' / script)], input=payload,
                                    capture_output=True, text=True, env=env, timeout=10)
            self.assertLess(time.time() - started, 2.0)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, '', ''))
        inbox.post('fast-1', 'ping')
        result = subprocess.run([sys.executable, str(REPO / 'everett' / 'hooks' / 'claude_inbox.py')],
                                input=payload, capture_output=True, text=True, env=env, timeout=10)
        self.assertIn('ping', json.loads(result.stdout)['hookSpecificOutput']['additionalContext'])
        result = subprocess.run([sys.executable, str(REPO / 'everett' / 'hooks' / 'claude_inbox.py')],
                                input='garbage', capture_output=True, text=True, env=env, timeout=10)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, '', ''))


class SendModes(Base):
    def test_delivery_mode(self):
        s = self.session()
        self.assertEqual(delivery_mode(s, 'auto', ps_out=''), 'resume')
        self.assertEqual(delivery_mode(s, 'auto', ps_out='/usr/bin/claude --resume target-1'), 'inbox')
        self.assertEqual(delivery_mode(s, 'inbox', ps_out=''), 'inbox')
        with mock.patch('everett.inbox.harness_pid', return_value=os.getpid()):
            inbox.touch_live('target-1', 'claude')
        self.assertEqual(delivery_mode(s, 'auto', ps_out=''), 'inbox')
        self.assertEqual(delivery_mode(s, 'resume', ps_out=''), 'resume')
        self.assertEqual(delivery_mode(self.session('t3', source='t3code'), 'auto', ps_out=''), 'inbox')
        with self.assertRaises(SendError):
            delivery_mode(s, 'bogus')

    def test_round_trip_with_reply(self):
        with mock.patch.dict(os.environ, {'EVERETT_SESSION_ID': 'sender-1'}):
            result = send_inbox(self.session(), 'please review')
        self.assertEqual(result['reply_inbox'], 'sender-1')
        msg = inbox.pending('target-1')[0]
        self.assertEqual((msg['from'], msg['hops']), ('sender-1', 1))
        with mock.patch.dict(os.environ, {'EVERETT_SESSION_ID': 'target-1'}):
            out = reply(msg['id'], 'looks good')
        self.assertEqual(out['to'], 'sender-1')
        got = inbox.pending('sender-1')[0]
        self.assertEqual((got['kind'], got['reply_to'], got['text'], got['hops']), ('reply', msg['id'], 'looks good', 2))
        self.assertIn(f'REPLY to your message {msg["id"]}', inbox.take('sender-1'))

    def test_wait_returns_reply(self):
        def answer(sender, message_id, wait, poll=1.0):
            return {'text': 'done', 'from': 'target-1'}
        with mock.patch('everett.inbox.wait_reply', side_effect=answer):
            result = send_inbox(self.session(), 'do it', wait=5)
        self.assertEqual(result['reply'], 'done')

    def test_hop_limit_on_reply_chain(self):
        m = inbox.post('a', 'x', sender='b', hops=3)
        with self.assertRaises(SendError) as cm:
            reply(m['id'], 'again')
        self.assertEqual(cm.exception.code, 7)
        with mock.patch.dict(os.environ, {'EVERETT_HOPS': '3'}), self.assertRaises(SendError):
            send_inbox(self.session(), 'loop')
        with self.assertRaises(SendError):
            reply('mmissing', 'x')

    def test_cli_send_inbox_reply_and_inbox(self):
        s = self.session()
        routed = {'decision': 'SESSION', 'choice': 'to', 'confidence': 1.0, 'session': asdict(s)}
        out = io.StringIO()
        with mock.patch('everett.cli.registry.scan', return_value=[s]), mock.patch(
                'everett.cli.route', return_value=routed), mock.patch('everett.cli.send') as resume, \
                contextlib.redirect_stdout(out):
            code = main(['send', 'check the build', '--mode', 'inbox', '--json'])
        self.assertEqual(code, 0)
        data = json.loads(out.getvalue())
        self.assertEqual((data['mode'], data['reply_inbox']), ('inbox', inbox.HUMAN))
        resume.assert_not_called()
        msg_id = data['message_id']
        out = io.StringIO()
        with mock.patch.dict(os.environ, {'EVERETT_SESSION_ID': 'target-1'}), contextlib.redirect_stdout(out):
            self.assertEqual(main(['reply', msg_id, 'build is green']), 0)
        self.assertIn('REPLIED', out.getvalue())
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(main(['inbox']), 0)
        self.assertIn('build is green', out.getvalue())
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main(['inbox'])
        self.assertIn('empty', out.getvalue())

    def test_cli_auto_mode_keeps_resume_for_idle_sessions(self):
        s = self.session()
        routed = {'decision': 'SESSION', 'choice': 's0', 'confidence': 0.9, 'session': asdict(s)}
        with mock.patch('everett.cli.registry.scan', return_value=[s]), mock.patch(
                'everett.cli.route', return_value=routed), mock.patch('everett.send.registry._ps', return_value=''), \
                mock.patch('everett.cli.send') as resume, contextlib.redirect_stdout(io.StringIO()):
            resume.return_value.reply, resume.return_value.command = 'ok', ['claude']
            self.assertEqual(main(['send', 'hello']), 0)
        resume.assert_called_once()
        self.assertEqual(inbox.pending('target-1'), [])


class Install(Base):
    def test_new_hooks_merge_idempotently_with_backup(self):
        settings = self.home / '.claude' / 'settings.json'
        settings.parent.mkdir(parents=True)
        settings.write_text(json.dumps({'hooks': {'PostToolUse': [{'matcher': 'Bash', 'hooks': [
            {'type': 'command', 'command': 'other-tool'}]}]}}))
        report = install.apply('claude')
        self.assertIn('UserPromptSubmit', report)
        self.assertIn('backup', report)
        data = json.loads(settings.read_text())
        self.assertEqual(len(data['hooks']['PostToolUse']), 2)
        self.assertIn('claude_inbox.py', data['hooks']['PostToolUse'][1]['hooks'][0]['command'])
        self.assertIn('already installed', install.apply('claude'))
        self.assertEqual(len(json.loads(settings.read_text())['hooks']['PostToolUse']), 2)
        self.assertIn('codex_inbox.py', install.snippet('codex'))
        self.assertIn('grok_inbox.py', install.snippet('grok'))


class GrokMCP(Base):
    def test_install_mcp_grok_toml_idempotent_with_backup(self):
        cfg = self.home / '.grok' / 'config.toml'
        cfg.parent.mkdir(parents=True)
        cfg.write_text('model = "grok-4"\n\n[mcp_servers.other]\ncommand = "x"\n')
        self.assertFalse(install.mcp_installed('grok'))
        self.assertIn('grok mcp add', install.mcp_snippet('grok'))
        report = install.apply_mcp('grok')
        self.assertIn('backup', report)
        text = cfg.read_text()
        self.assertIn('[mcp_servers.other]', text)
        self.assertEqual(text.count('[mcp_servers.everett]'), 1)
        self.assertTrue(install.mcp_installed('grok'))
        self.assertIn('already registered', install.apply_mcp('grok'))
        self.assertEqual(cfg.read_text().count('[mcp_servers.everett]'), 1)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            main(['install-mcp'])
        self.assertIn('## grok', out.getvalue())


class OMPInbox(Base):
    SCRIPT = r'''
const hook = await import(process.env.OMP_HOOK_PATH);
const handlers = {};
const sent = [];
hook.default({ on: (event, callback) => { handlers[event] = callback; },
               sendMessage: async (msg, opts) => { sent.push({ msg, opts }); } });
const ctx = { sessionManager: { getSessionId: () => "omp-live" } };
await handlers.tool_result({}, ctx);
const before = sent.length;
const fs = await import("node:fs");
fs.mkdirSync(process.env.EVERETT_HOME + "/.everett/inbox", { recursive: true });
fs.writeFileSync(process.env.EVERETT_HOME + "/.everett/inbox/omp-live.jsonl",
  JSON.stringify({ id: "m1", from: "s9", text: "mid-task note", ts: Date.now() / 1000, hops: 1, kind: "message" }) + "\n" +
  JSON.stringify({ id: "m2", from: "s9", text: "next-turn note", ts: Date.now() / 1000, hops: 1, kind: "message" }) + "\n");
await handlers.tool_result({}, ctx);
const turn = await handlers.before_agent_start({ systemPrompt: ["base"] }, ctx);
console.log(JSON.stringify({ before, sent, turn }));
'''

    def test_omp_extension_steers_mid_task_and_injects_next_turn(self):
        node = shutil.which('node')
        if not node:
            self.skipTest('Node.js is required to exercise the OMP extension module')
        hook = REPO / 'everett' / 'hooks' / 'omp_session_start.mjs'
        env = {**os.environ, 'OMP_HOOK_PATH': hook.as_uri()}
        result = subprocess.run([node, '--input-type=module', '-e', self.SCRIPT], capture_output=True, text=True,
                                env=env, cwd=REPO, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        out = json.loads(result.stdout)
        self.assertEqual(out['before'], 0)
        self.assertEqual(len(out['sent']), 1)
        self.assertEqual(out['sent'][0]['opts'], {'deliverAs': 'steer'})
        self.assertIn('mid-task note', out['sent'][0]['msg']['content'])
        self.assertIn('next-turn note', out['sent'][0]['msg']['content'])  # both pending went in one batch
        self.assertIsNone(out.get('turn'))  # nothing left for the next turn
        self.assertEqual(inbox.pending('omp-live'), [])


if __name__ == '__main__':
    unittest.main()
