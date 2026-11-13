import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(os.environ.get('EVERETT_VERIFY_BINARY'), 'set EVERETT_VERIFY_BINARY to verify an actual runtime')
class RuntimeTests(unittest.TestCase):
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
            smoke = subprocess.run([sys.executable, str(ROOT / 'tests/smoke.py'), '--manifest', str(plugin / '.mcp.json')],
                env=env, cwd=home, capture_output=True, text=True, timeout=45)
            self.assertEqual(smoke.returncode, 0, smoke.stderr)
            result = json.loads(smoke.stdout)
            self.assertEqual(result['session_id'], session)
            self.assertTrue(result['local_listing_call'])
            self.assertEqual(result['messages_sent'], 0)
