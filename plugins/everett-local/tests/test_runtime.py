from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(os.environ.get('EVERETT_VERIFY_BINARY'), 'set EVERETT_VERIFY_BINARY to verify an actual runtime')
class RuntimeTests(unittest.TestCase):
    def configure_account(self, home, env):
        identity = {'owner': 'fixture-owner', 'deviceId': 'fixture-device', 'role': 'member'}
        token = 'evt_' + 'a' * 64

        class Issuer(BaseHTTPRequestHandler):
            def do_POST(self):
                if self.path != '/v2/relay/whoami' or self.headers.get('Authorization') != f'Bearer {token}':
                    self.send_error(401)
                    return
                self.rfile.read(int(self.headers.get('Content-Length', '0')))
                body = json.dumps({'result': identity}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        issuer = ThreadingHTTPServer(('127.0.0.1', 0), Issuer)
        thread = threading.Thread(target=issuer.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(issuer.server_close)
        self.addCleanup(thread.join)
        self.addCleanup(issuer.shutdown)
        origin = f'http://127.0.0.1:{issuer.server_port}'
        env.update(EVERETT_AUTH_PROVIDER='cloudflare', EVERETT_AUTH_BASE_URL=origin)
        credential = home / '.everett/cloudflare-auth.json'
        credential.parent.mkdir(mode=0o700, exist_ok=True)
        with open(credential, 'x', opener=lambda name, flags: os.open(name, flags, 0o600)) as credits:
            json.dump({'issuer': origin, 'owner': identity['owner'], 'agent_id': identity['deviceId'],
                       'device_name': 'Fixture', 'token': token,
                       'expires_at': int(time.time() * 1000) + 120_000}, credits)

    def test_installed_portable_command_with_isolated_session(self):
        binary = Path(os.environ['EVERETT_VERIFY_BINARY']).resolve(strict=True)
        with tempfile.TemporaryDirectory(prefix='everett-plugin-runtime-') as directory:
            home = Path(directory)
            session = 'e29a19a4-8220-4f87-a508-a86dcd46f93e'
            transcript = home / '.claude/projects/plugin-check' / f'{session}.jsonl'
            transcript.parent.mkdir(parents=True)
            transcript.write_text(json.dumps({'type': 'user', 'sessionId': session,
                'cwd': str(home), 'message': {'role': 'user', 'content': 'Verify local plugin tools.'}}) + '\n')
            env = {'HOME': str(home), 'EVERETT_HOME': str(home), 'PATH': os.environ.get('PATH', ''),
                   'EVERETT_SESSION_ID': session, 'EVERETT_HARNESS_NAME': 'claude',
                   'EVERETT_NOTIFY': 'none', 'EVERETT_ROUTER': 'local', 'CI': '1'}
            installed = subprocess.run([sys.executable, str(ROOT / 'install.py'), '--binary', str(binary),
                '--destination', str(home / 'plugin with spaces')], env=env, capture_output=True, text=True, timeout=15)
            self.assertEqual(installed.returncode, 0, installed.stderr)
            plugin = Path(json.loads(installed.stdout)['plugin'])
            self.configure_account(home, env)
            smoke = subprocess.run([sys.executable, str(ROOT / 'tests/smoke.py'), '--manifest', str(plugin / '.mcp.json')],
                env=env, cwd=home, capture_output=True, text=True, timeout=45)
            self.assertEqual(smoke.returncode, 0, smoke.stderr)
            result = json.loads(smoke.stdout)
            self.assertEqual(result['session_id'], session)
            self.assertTrue(result['local_listing_call'])
            self.assertEqual(result['messages_sent'], 0)
