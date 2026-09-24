import sandbox  # noqa: F401  (must be first: isolates HOME)

import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from everett import install, mcp
from everett.cli import main

REPO = Path(__file__).resolve().parents[1]
FX = Path(__file__).parent / 'fixtures'


class Server:
    """`python3 -m everett mcp` over real pipes, with a synthetic HOME and no harness CLIs on PATH."""

    def __init__(self, home: Path, extra_env: dict | None = None):
        env = {'HOME': str(home), 'EVERETT_HOME': str(home), 'PATH': '/usr/bin:/bin',
               'PYTHONPATH': str(REPO), **(extra_env or {})}
        self.proc = subprocess.Popen([sys.executable, '-m', 'everett', 'mcp'], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
                                     cwd=str(home))
        self.next_id = 0

    def raw(self, line: str) -> dict:
        self.proc.stdin.write(line + '\n')
        self.proc.stdin.flush()
        return json.loads(self.proc.stdout.readline())

    def notify(self, method: str, params: dict | None = None) -> None:
        self.proc.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': method, **({'params': params} if params else {})}) + '\n')
        self.proc.stdin.flush()

    def request(self, method: str, params: dict | None = None) -> dict:
        self.next_id += 1
        msg = {'jsonrpc': '2.0', 'id': self.next_id, 'method': method}
        if params is not None:
            msg['params'] = params
        response = self.raw(json.dumps(msg))
        assert response['id'] == self.next_id, response
        return response

    def call(self, name: str, **arguments) -> dict:
        return self.request('tools/call', {'name': name, 'arguments': arguments})['result']

    def close(self) -> str:
        self.proc.stdin.close()
        self.proc.wait(timeout=10)
        err = self.proc.stderr.read()
        self.proc.stdout.close()
        self.proc.stderr.close()
        return err


class TempHome(unittest.TestCase):
    def setUp(self):
        self._dir = tempfile.TemporaryDirectory()
        self.home = Path(self._dir.name)
        patcher = mock.patch.dict(os.environ, {'HOME': str(self.home), 'EVERETT_HOME': str(self.home)})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self._dir.cleanup)
        store = self.home / '.claude' / 'projects' / '-work-app'
        store.mkdir(parents=True)
        shutil.copy(FX / 'claude.jsonl', store / 'c-1.jsonl')  # session c-1, cwd /work/app

    def server(self, **env) -> Server:
        s = Server(self.home, env)
        self.addCleanup(lambda: s.proc.poll() is None and s.proc.kill())
        return s

    def start(self, **env) -> Server:
        s = self.server(**env)
        init = s.request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                        'clientInfo': {'name': 'test', 'version': '0'}})
        self.assertEqual(init['result']['protocolVersion'], '2025-06-18')
        s.notify('notifications/initialized')
        return s


class Protocol(TempHome):
    def test_round_trip_over_pipes(self):
        s = self.start()
        init_info = s.request('ping')
        self.assertEqual(init_info['result'], {})
        tools = s.request('tools/list')['result']['tools']
        self.assertEqual({t['name'] for t in tools}, {'everett_ls', 'everett_route', 'everett_send', 'everett_learn',
                                                      'everett_core', 'everett_card', 'everett_whoami'})
        listing = s.call('everett_ls')
        self.assertFalse(listing['isError'])
        sessions = listing['structuredContent']['sessions']
        self.assertEqual([x['id'] for x in sessions], ['c-1'])
        self.assertEqual(json.loads(listing['content'][0]['text']), listing['structuredContent'])
        routed = s.call('everett_route', text='add tests for the login bug', router='local')['structuredContent']
        self.assertEqual((routed['decision'], routed['session']['id']), ('SESSION', 'c-1'))
        self.assertEqual(s.close(), '')

    def test_version_negotiation(self):
        s = self.server()
        old = s.request('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {}, 'clientInfo': {}})
        self.assertEqual(old['result']['protocolVersion'], '2024-11-05')
        future = s.request('initialize', {'protocolVersion': '2099-01-01', 'capabilities': {}, 'clientInfo': {}})
        self.assertEqual(future['result']['protocolVersion'], mcp.PROTOCOL_VERSIONS[0])
        self.assertEqual(old['result']['serverInfo']['name'], 'everett')
        s.close()

    def test_error_codes(self):
        s = self.start()
        self.assertEqual(s.raw('{not json')['error']['code'], mcp.PARSE_ERROR)
        self.assertEqual(s.raw('{"jsonrpc": "1.0", "id": 9, "method": "ping"}')['error']['code'], mcp.INVALID_REQUEST)
        self.assertEqual(s.request('resources/list')['error']['code'], mcp.METHOD_NOT_FOUND)
        self.assertEqual(s.request('tools/call', {'name': 'nope'})['error']['code'], mcp.INVALID_PARAMS)
        self.assertEqual(s.request('tools/call', {'name': 'everett_route', 'arguments': {}})['error']['code'],
                         mcp.INVALID_PARAMS)
        self.assertEqual(s.request('tools/call', {'name': 'everett_ls', 'arguments': {'hours': 'x'}})['error']['code'],
                         mcp.INVALID_PARAMS)
        batch = json.loads(s.proc.stdin.write('[{"jsonrpc":"2.0","id":"a","method":"ping"},'
                                              '{"jsonrpc":"2.0","method":"notifications/x"}]\n') and
                           (s.proc.stdin.flush() or s.proc.stdout.readline()))
        self.assertEqual(batch, [{'jsonrpc': '2.0', 'id': 'a', 'result': {}}])
        s.close()

    def test_tool_schemas(self):
        for tool in mcp.TOOLS:
            self.assertTrue(tool['description'].strip())
            schema = tool['inputSchema']
            self.assertEqual(schema['type'], 'object')
            self.assertFalse(schema['additionalProperties'])
            for key in schema.get('required', []):
                self.assertIn(key, schema['properties'])
        json.dumps(mcp.TOOLS)


class Safety(TempHome):
    def test_hop_guard_refuses_past_three(self):
        s = self.start(EVERETT_HOPS='3')
        result = s.call('everett_send', text='keep going', to='c-1')
        self.assertTrue(result['isError'])
        self.assertIn('Hop limit', result['content'][0]['text'])
        s.close()

    def test_self_send_refused(self):
        s = self.start(CLAUDE_CODE_SESSION_ID='c-1')
        who = s.call('everett_whoami')['structuredContent']
        self.assertEqual((who['session_id'], who['harness'], who['source']), ('c-1', 'claude', 'env CLAUDE_CODE_SESSION_ID'))
        listing = s.call('everett_ls')['structuredContent']['sessions']
        self.assertTrue(listing[0].get('you'))
        result = s.call('everett_send', text='hello me', to='c-1')
        self.assertTrue(result['isError'])
        self.assertIn('calling session itself', result['content'][0]['text'])
        explicit = self.start()
        result = explicit.call('everett_send', text='hello me', to='c-1', session_id='c-1')
        self.assertIn('calling session itself', result['content'][0]['text'])
        s.close()
        explicit.close()

    def test_new_without_spawn_sends_nothing(self):
        s = self.start()
        result = s.call('everett_send', text='compose a sonnet about penguins')['structuredContent']
        self.assertEqual((result['decision'], result['delivered']), ('NEW', False))
        self.assertIn('spawn=true', result['note'])
        s.close()

    def test_learn_secret_filter_core_and_card(self):
        s = self.start(CODEX_THREAD_ID='x-9')
        bad = s.call('everett_learn', fact='the api_key = abcdef123456')
        self.assertTrue(bad['isError'])
        self.assertIn('Secrets never go into the shared core', bad['content'][0]['text'])
        good = s.call('everett_learn', fact='Deploys go out from main only')
        self.assertFalse(good['isError'])
        self.assertIn('pending_learnings', s.call('everett_core')['structuredContent'])
        card = s.call('everett_card', what='Deploy tooling', state='writing the rollback script', next='test it')
        self.assertFalse(card['isError'])
        path = self.home / '.everett' / 'cards' / 'x-9.md'
        self.assertEqual(path.read_text(), 'What: Deploy tooling\nState: writing the rollback script\nNext: test it\n')
        s.close()
        inbox = (self.home / '.everett' / 'core' / 'inbox.jsonl').read_text().splitlines()
        self.assertEqual(len(inbox), 1)
        self.assertEqual(json.loads(inbox[0])['harness'], 'codex')

    def test_card_needs_identity(self):
        s = self.start()
        result = s.call('everett_card', what='a', state='b', next='c')
        self.assertTrue(result['isError'])
        self.assertIn('session_id', result['content'][0]['text'])
        s.close()

    def test_log_records_calls_with_clipped_bodies(self):
        s = self.start()
        s.call('everett_route', text='x' * 1000, router='local')
        s.close()
        lines = [json.loads(line) for line in (self.home / '.everett' / 'mcp.log').read_text().splitlines()]
        call = next(line for line in lines if line['event'] == 'call')
        self.assertEqual(call['tool'], 'everett_route')
        self.assertLessEqual(len(call['args']), 201)
        self.assertTrue(any(line['event'] == 'result' for line in lines))
        self.assertTrue(all(len(json.dumps(line)) < 1000 for line in lines))


class InstallMCP(TempHome):
    def test_print_writes_nothing(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(main(['install-mcp']), 0)
        text = out.getvalue()
        self.assertIn('claude mcp add --scope user', text)
        self.assertIn('[mcp_servers.everett]', text)
        self.assertIn('"mcpServers"', text)
        self.assertFalse((self.home / '.claude.json').exists())

    def test_apply_backs_up_and_is_idempotent(self):
        claude_json = self.home / '.claude.json'
        claude_json.write_text(json.dumps({'numStartups': 3, 'mcpServers': {'other': {'command': 'x'}}}))
        codex_toml = self.home / '.codex' / 'config.toml'
        codex_toml.parent.mkdir()
        codex_toml.write_text('model = "gpt"\n[mcp_servers.other]\ncommand = "x"\n')
        for _ in range(2):
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(main(['install-mcp', '--apply']), 0)
        data = json.loads(claude_json.read_text())
        self.assertEqual(data['numStartups'], 3)
        self.assertEqual(set(data['mcpServers']), {'other', 'everett'})
        self.assertEqual(data['mcpServers']['everett']['args'], ['-m', 'everett', 'mcp'])
        toml = codex_toml.read_text()
        self.assertEqual(toml.count('[mcp_servers.everett]'), 1)
        self.assertTrue(toml.startswith('model = "gpt"\n[mcp_servers.other]'))
        if sys.version_info >= (3, 11):
            import tomllib
            parsed = tomllib.loads(toml)['mcp_servers']['everett']
            self.assertEqual(parsed['args'], ['-m', 'everett', 'mcp'])
        omp = json.loads((self.home / '.omp' / 'agent' / 'mcp.json').read_text())
        self.assertIn('everett', omp['mcpServers'])
        self.assertEqual(len(list(self.home.glob('.claude.json.everett-bak-*'))), 1)
        self.assertEqual(len(list(codex_toml.parent.glob('config.toml.everett-bak-*'))), 1)
        self.assertTrue(all(install.mcp_installed(h) for h in ('claude', 'codex', 'omp')))

    def test_registered_command_actually_serves(self):
        command, args, env = install.mcp_launch()
        proc = subprocess.run([command, *args], input='{"jsonrpc":"2.0","id":1,"method":"ping"}\n',
                              capture_output=True, text=True, timeout=20,
                              env={'HOME': str(self.home), 'PATH': '/usr/bin:/bin', **env}, cwd=str(self.home))
        self.assertEqual(json.loads(proc.stdout), {'jsonrpc': '2.0', 'id': 1, 'result': {}})


if __name__ == '__main__':
    unittest.main()
