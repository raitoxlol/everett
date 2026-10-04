#!/usr/bin/env python3
"""Drive an installed Everett CLI and all MCP tools with disposable session stores."""
from __future__ import annotations

import argparse
import json
import os
import select
import subprocess
import sys
import tempfile
import time
from pathlib import Path

EXPECTED_TOOLS = {'everett_ls', 'everett_route', 'everett_send', 'everett_inbox', 'everett_event',
                  'everett_subscribe', 'everett_learn', 'everett_core', 'everett_card', 'everett_whoami'}


def check(condition, detail):
    if not condition:
        raise AssertionError(detail)


class Wire:
    def __init__(self, command, env, cwd, evidence):
        self.evidence = evidence
        self.seen = set()
        self.next_id = 0
        self.proc = subprocess.Popen(command, cwd=cwd, env=env, text=True,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def request(self, method, params=None):
        self.next_id += 1
        message = {'jsonrpc': '2.0', 'id': self.next_id, 'method': method}
        if params is not None:
            message['params'] = params
        self.proc.stdin.write(json.dumps(message) + '\n')
        self.proc.stdin.flush()
        check(select.select([self.proc.stdout], [], [], 10)[0], f'MCP {method} timed out')
        response = json.loads(self.proc.stdout.readline())
        self.evidence.append({'request': message, 'response': response})
        check(response.get('id') == self.next_id, 'MCP response id mismatch')
        check('error' not in response, f'MCP protocol error: {response}')
        return response['result']

    def call(self, name, **arguments):
        result = self.request('tools/call', {'name': name, 'arguments': arguments})
        check(not result.get('isError'), f'{name} failed: {result}')
        self.seen.add(name)
        return result['structuredContent']

    def initialize(self):
        info = self.request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                          'clientInfo': {'name': 'everett-release-check', 'version': '1'}})
        check('tools' in info['capabilities'], 'MCP does not advertise tools')
        self.proc.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        self.proc.stdin.flush()
        tools = self.request('tools/list')['tools']
        check(len(tools) == 10 and {t['name'] for t in tools} == EXPECTED_TOOLS, 'MCP tool list is incomplete')
        return info['serverInfo']

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=5)
        stderr = self.proc.stderr.read()
        self.proc.stdout.close()
        self.proc.stderr.close()
        self.evidence.append({'server_exit': self.proc.returncode, 'stderr': stderr})


def seed_session(root, sid, cwd, text):
    path = root / '.claude/projects/verification' / f'{sid}.jsonl'
    path.parent.mkdir(parents=True, exist_ok=True)
    rows = [{'type': 'user', 'sessionId': sid, 'cwd': str(cwd),
             'message': {'role': 'user', 'content': text}},
            {'type': 'assistant', 'sessionId': sid,
             'message': {'role': 'assistant', 'content': [{'type': 'text', 'text': 'Ready for review.'}]}}]
    path.write_text('\n'.join(json.dumps(r) for r in rows) + '\n', encoding='utf-8')
    old = time.time() - 120
    os.utime(path, (old, old))


def journey(args, evidence):
    cli = [str(Path(args.cli).resolve())] if args.cli else [args.python, '-m', 'everett']
    with tempfile.TemporaryDirectory(prefix='everett-release-check-') as tmp:
        root = Path(tmp)
        evidence['scratch_home'] = tmp
        bin_dir = root / 'bin'
        bin_dir.mkdir()
        stub = bin_dir / 'claude'
        stub.write_text('#!/bin/sh\n# Detection only: a model call fails this verification.\nexit 88\n')
        stub.chmod(0o755)
        env = {'HOME': tmp, 'EVERETT_HOME': tmp, 'PATH': str(bin_dir) + os.pathsep + os.defpath,
               'EVERETT_NOTIFY': 'none', 'EVERETT_ROUTER': 'local', 'CI': '1'}
        if args.source:
            env['PYTHONPATH'] = str(Path(args.source).resolve())

        def run(command, extra_env=None):
            result = subprocess.run(command, cwd=tmp, env={**env, **(extra_env or {})}, capture_output=True,
                                    text=True, timeout=15)
            evidence['cli'].append({'command': command, 'code': result.returncode,
                                    'stdout': result.stdout, 'stderr': result.stderr})
            check(result.returncode == 0, f'CLI failed: {command}: {result.stderr}')
            return result.stdout

        evidence['version'] = run([*cli, '--version']).strip()
        check(run([args.python, '-m', 'everett', '--version']).strip() == evidence['version'],
              'Console and module entry points disagree')
        if not args.source:
            installed = json.loads(run([args.python, '-c',
                'import everett, importlib.metadata, json; '
                'print(json.dumps({"version": importlib.metadata.version("everett-sessions"), '
                '"module_version": everett.__version__, "module_file": everett.__file__}))']))
            check(evidence['version'] == 'everett ' + installed['version'] ==
                  'everett ' + installed['module_version'], 'Packaged metadata and runtime versions disagree')
            evidence['installed'] = installed
        check('10 tools over stdio' in run([*cli, 'doctor']), 'doctor did not probe MCP')
        run([*cli, 'onboard', '--yes', '--no-backfill'])
        check((root / '.claude.json').is_file(), 'Onboarding missed the CLI without a session store')
        check('setup needs attention' not in run([*cli, 'doctor']), 'Fresh onboarding left setup incomplete')

        sender_dir, worker_dir = root / 'docs', root / 'uploads'
        sender_dir.mkdir()
        worker_dir.mkdir()
        seed_session(root, 'verify-sender', sender_dir, 'Document email templates')
        task = 'Fix upload retries for storage client'
        seed_session(root, 'verify-worker', worker_dir, task)
        check(len(json.loads(run([*cli, 'ls', '--json']))) == 2, 'CLI did not find both sessions')
        routed = json.loads(run([*cli, 'route', task, '--router', 'local', '--json']))
        check(routed['decision'] == 'SESSION' and routed['session']['id'] == 'verify-worker', 'CLI route failed')

        wire = Wire([args.python, '-m', 'everett', 'mcp'],
                    {**env, 'EVERETT_SESSION_ID': 'verify-sender', 'EVERETT_HARNESS_NAME': 'claude'}, tmp, evidence['mcp'])
        try:
            evidence['server'] = wire.initialize()
            check(wire.call('everett_whoami')['session_id'] == 'verify-sender', 'Caller identity missing')
            for sid, what in [('verify-sender', 'Email templates'), ('verify-worker', 'Upload retries')]:
                wire.call('everett_card', session_id=sid, what=what, state='Reviewing', next='Run checks')
                check((root / '.everett/cards' / f'{sid}.md').is_file(), 'Card was not written')
            sessions = wire.call('everett_ls')['sessions']
            check({s['id'] for s in sessions} == {'verify-sender', 'verify-worker'}, 'MCP session listing failed')
            routed = wire.call('everett_route', text=task, router='local', session_id='verify-sender')
            check(routed['session']['id'] == 'verify-worker', 'MCP route chose the wrong session')

            sent = wire.call('everett_send', text='Review the upload tests', to='verify-worker', mode='inbox')
            received = wire.call('everett_inbox', session_id='verify-worker')['messages']
            check(len(received) == 1 and received[0]['id'] == sent['message_id'], 'Message did not reach the worker')
            check(wire.call('everett_inbox', session_id='verify-worker')['messages'] == [], 'Message was not consumed')
            run([*cli, 'reply', sent['message_id'], 'Upload tests pass'], {'EVERETT_SESSION_ID': 'verify-worker'})
            replies = wire.call('everett_inbox')['messages']
            check(replies[0]['text'] == 'Upload tests pass' and replies[0]['reply_to'] == sent['message_id'],
                  'Reply did not return to the original sender')
            cli_sent = json.loads(run([*cli, 'send', 'Check the retry cap', '--to', 'verify-worker',
                                       '--mode', 'inbox', '--json'], {'EVERETT_SESSION_ID': 'verify-sender'}))
            wire.call('everett_send', reply_to=cli_sent['message_id'], text='Cap is five', session_id='verify-worker')
            cli_inbox = json.loads(run([*cli, 'inbox', '--session', 'verify-sender', '--json']))
            check(cli_inbox['messages'][0]['text'] == 'Cap is five', 'CLI did not read the MCP reply')

            wire.call('everett_subscribe', target='verify-worker')
            wire.call('everett_event', kind='done', message='Upload tests passed', session_id='verify-worker')
            updates = wire.call('everett_inbox')['messages']
            check(any(m['kind'] == 'event' and 'Upload tests passed' in m['text'] for m in updates),
                  'Status subscription did not deliver an event')
            worker = next(s for s in wire.call('everett_ls')['sessions'] if s['id'] == 'verify-worker')
            check(worker['state_kind'] == 'done', 'Session status was not updated')
            check(json.loads(run([*cli, 'events', '--json']))[-1]['kind'] == 'done', 'CLI did not see the status')

            fact = 'Use bounded retries for uploads'
            wire.call('everett_learn', fact=fact, scope='global')
            run([*cli, 'learn', 'Retry caps are five', '--scope', 'project', '--project', 'verification'])
            check(wire.call('everett_core', project='verification')['pending_learnings'] == 2, 'Learnings were not queued')
            run([*cli, 'trunk', 'merge', '--llm', 'none'])
            shared = wire.call('everett_core', project='verification')
            check(fact in shared['core'] and 'Retry caps are five' in shared['core'], 'Merged core lost facts')
            check(shared['pending_learnings'] == 0, 'Merged facts remained pending')
            check((root / '.everett/core/core.md').is_file(), 'Global core was not written')
            check((root / '.everett/core/projects/verification.md').is_file(), 'Project core was not written')

            # A child that reads stdin must see EOF while the MCP pipe remains open.
            codex = bin_dir / 'codex'
            codex.write_text('#!' + str(Path(args.python).resolve()) + '\nimport json,sys\n'
                             'print(json.dumps({"stdin":sys.stdin.read(),"thread_id":"verify-spawned"}))\n')
            codex.chmod(0o755)
            peer = root / '.codex/sessions/2026/01/01/verify-codex.jsonl'
            peer.parent.mkdir(parents=True)
            codex_task = 'Repair sqlite backup restoration'
            peer.write_text(json.dumps({'type': 'session_meta', 'payload': {
                'id': 'verify-codex', 'cwd': str(worker_dir), 'source': 'cli'}}) + '\n' +
                json.dumps({'type': 'response_item', 'payload': {'type': 'message', 'role': 'user',
                    'content': [{'type': 'input_text', 'text': codex_task}]}}) + '\n')
            old = time.time() - 96 * 3600
            os.utime(peer, (old, old))
            (root / '.codex/session_index.jsonl').write_text(json.dumps({
                'id': 'verify-codex', 'thread_name': 'Backup restoration'}) + '\n')
            check(not wire.call('everett_ls', harness='codex')['sessions'], 'Default window included an old peer')
            older = wire.call('everett_route', text=codex_task, router='local', hours=168)
            check(older['session']['id'] == 'verify-codex', 'MCP route ignored the larger window')
            resumed = wire.call('everett_send', text='Check stdin', to='Backup restoration', hours=168, mode='resume')
            check(json.loads(resumed['reply'])['stdin'] == '', 'Resumed child consumed MCP stdin')
            spawned = wire.call('everett_send', text='Calibrate neutrino spectrometer', spawn=True,
                                harness='codex', dir=str(worker_dir))
            check(spawned.get('spawned') and json.loads(spawned['reply'])['stdin'] == '',
                  'Spawned child consumed MCP stdin')
            wire.request('ping')
            evidence['delivery_checks'] = {'resume_stdin': 'isolated', 'spawn_stdin': 'isolated',
                                           'older_session_hours': 168, 'exact_title': 'verified',
                                           'ping_after_delivery': 'passed'}
            check(wire.seen == EXPECTED_TOOLS, 'Not every advertised MCP tool was exercised')
            evidence['tools_exercised'] = sorted(wire.seen)
        finally:
            wire.close()
        check(wire.proc.returncode == 0, 'MCP server did not exit cleanly')
    evidence['scratch_cleaned'] = not root.exists()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--python', default=sys.executable, help='Python from the install being tested')
    parser.add_argument('--cli', help='Absolute path to its everett console script')
    parser.add_argument('--source', help='Explicit checkout to test instead of an installed distribution')
    parser.add_argument('--evidence', default='.audit/verification.json', help='JSON proof retained after cleanup')
    args = parser.parse_args()
    evidence = {'cli': [], 'mcp': [], 'started': time.time(), 'status': 'failed'}
    try:
        journey(args, evidence)
        evidence['status'] = 'passed'
    except Exception as exc:
        evidence['error'] = f'{type(exc).__name__}: {exc}'
        print(evidence['error'], file=sys.stderr)
    finally:
        evidence['seconds'] = round(time.time() - evidence['started'], 3)
        evidence['scratch_cleaned'] = not Path(evidence.get('scratch_home', '/')).exists()
        target = Path(args.evidence).resolve()
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(evidence, indent=2) + '\n', encoding='utf-8')
        print(f'{evidence["status"]}: {target}')
    return 0 if evidence['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
