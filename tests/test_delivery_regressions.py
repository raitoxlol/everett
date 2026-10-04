import sandbox  # noqa: F401

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from everett import inbox, mcp, registry
from everett.session import Session

REPO = Path(__file__).resolve().parents[1]


class ChildInput(unittest.TestCase):
    def test_resume_and_spawn_cannot_consume_the_parent_protocol(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / 'codex'
            binary.write_text('#!' + sys.executable + '\nimport json,sys\n'
                              'print(json.dumps({"stdin":sys.stdin.read()}))\n')
            binary.chmod(0o755)
            env = {'HOME': tmp, 'EVERETT_HOME': tmp, 'PATH': tmp + os.pathsep + os.defpath,
                   'PYTHONPATH': str(REPO), 'EVERETT_NOTIFY': 'none'}
            session = Session('codex', 'input-test-peer', tmp, str(root / 'missing'), '', time.time() - 120)
            for operation in ('resume', 'spawn'):
                with self.subTest(operation=operation):
                    code = ('import json,sys; from everett import send; from everett.session import Session; '
                            's=Session(**json.loads(sys.argv[1])); '
                            'r=send.send(s,"Check stdin",wait=0) if sys.argv[2]=="resume" '
                            'else send.spawn("codex","Check stdin",s.cwd); print(r.reply)')
                    result = subprocess.run([sys.executable, '-c', code, json.dumps(session.to_dict()), operation],
                                            input='{"jsonrpc":"2.0","method":"ping","id":99}\n',
                                            capture_output=True, text=True, cwd=tmp, env=env, timeout=10)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(json.loads(result.stdout)['stdin'], '',
                                     'The harness consumed input intended for the MCP server')

    def test_interrupted_codex_is_not_retried_and_reports_uncertain_delivery(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = root / 'codex'
            binary.write_text('#!' + sys.executable + '\nimport pathlib,sys\n'
                              'with (pathlib.Path.cwd()/"attempts").open("a") as f: f.write("attempt\\n")\n'
                              'print("Error: Interrupted system call (os error 4)",file=sys.stderr)\n'
                              'sys.exit(1)\n')
            binary.chmod(0o755)
            env = {'HOME': tmp, 'EVERETT_HOME': tmp, 'PATH': tmp + os.pathsep + os.defpath,
                   'PYTHONPATH': str(REPO), 'EVERETT_NOTIFY': 'none'}
            code = ('from everett import send; from everett.session import Session; import sys; '
                    's=Session("codex","interruption-peer",sys.argv[1],"/missing","",0); '
                    'send.send(s,"Check delivery",wait=0)')
            result = subprocess.run([sys.executable, '-c', code, tmp], cwd=tmp, env=env,
                                    capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('Delivery is unconfirmed', result.stderr)
            self.assertIn('check the target session before retrying', result.stderr)
            self.assertEqual((root / 'attempts').read_text().splitlines(), ['attempt'])


class TargetLookup(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name)
        patch = mock.patch.dict(os.environ, {'HOME': temp.name, 'EVERETT_HOME': temp.name,
                                            'EVERETT_ROUTER': 'local', 'EVERETT_SESSION_ID': 'lookup-caller'})
        patch.start()
        self.addCleanup(patch.stop)

    def old_peer(self):
        path = self.home / '.claude/projects/test/old-peer.jsonl'
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({'type': 'user', 'sessionId': 'old-peer', 'cwd': str(self.home / 'backups'),
                                    'message': {'role': 'user', 'content': 'Restore database backups using sqlite'}}) + '\n')
        old = time.time() - 96 * 3600
        os.utime(path, (old, old))

    def test_mcp_can_deliver_to_an_older_named_peer_with_explicit_hours(self):
        self.old_peer()
        args = {'text': 'Report backup progress', 'to': 'old-peer', 'mode': 'inbox'}
        default = mcp.call_tool({'name': 'everett_send', 'arguments': args})
        self.assertTrue(default['isError'])
        self.assertIn('everett_ls', default['content'][0]['text'])
        result = mcp.call_tool({'name': 'everett_send', 'arguments': {**args, 'hours': 168}})
        self.assertFalse(result['isError'], result)
        self.assertEqual(inbox.pending('old-peer')[0]['text'], args['text'])

    def test_mcp_route_can_use_the_same_older_session_window(self):
        self.old_peer()
        result = mcp.call_tool({'name': 'everett_route', 'arguments': {
            'text': 'Restore database backups using sqlite', 'router': 'local', 'hours': 168}})
        self.assertFalse(result['isError'], result)
        self.assertEqual(result['structuredContent']['session']['id'], 'old-peer')

    def test_exact_plain_title_resolves_without_a_colon(self):
        peers = [Session('codex', 'peer-a', '/work/one', '', '', 0, title='Slash resume not showing newest sessions'),
                 Session('claude', 'peer-b', '/work/two', '', '', 0, title='Other work')]
        self.assertEqual(registry.find('slash resume not showing newest sessions', peers).id, 'peer-a')

    def test_duplicate_plain_titles_require_an_id(self):
        peers = [Session('codex', 'peer-a', '/work/one', '', '', 0, title='Limen'),
                 Session('claude', 'peer-b', '/work/two', '', '', 0, title='Limen')]
        with self.assertRaisesRegex(registry.SessionLookupError, 'matches 2 sessions'):
            registry.find('Limen', peers)
