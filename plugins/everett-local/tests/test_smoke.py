import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class SmokeTests(unittest.TestCase):
    def test_listing_must_contain_sessions(self):
        tools = ['everett_whoami', 'everett_ls', 'everett_send', 'everett_inbox']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / 'manifest.json'
            replies = root / 'replies.jsonl'
            manifest.write_text(json.dumps({'mcpServers': {'everett_local': {
                'command': 'cat', 'args': [str(replies)]
            }}}))
            results = [{}, {'tools': [{'name': name} for name in tools]},
                       {'structuredContent': {'session_id': 'caller'}}, {'structuredContent': {}}]
            replies.write_text(''.join(json.dumps({'id': i, 'result': result}) + '\n'
                                       for i, result in enumerate(results, 1)))
            run = subprocess.run([sys.executable, str(Path(__file__).with_name('smoke.py')),
                                  '--manifest', str(manifest)], capture_output=True, text=True)
            self.assertNotEqual(run.returncode, 0)
            self.assertIn('everett_ls returned no session list', run.stderr)
