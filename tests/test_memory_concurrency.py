import sandbox  # noqa: F401

import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from everett import core


class SharedMemoryConcurrency(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name)
        env = mock.patch.dict(os.environ, {'HOME': tmp.name, 'EVERETT_HOME': tmp.name})
        env.start()
        self.addCleanup(env.stop)

    def test_learning_during_merge_stays_pending_and_is_not_archived(self):
        core.learn('Initial convention')

        def runner(*args, **kwargs):
            core.learn('Late convention')
            return SimpleNamespace(returncode=0, stdout=json.dumps({'global': '# Core\n\n- Initial convention\n',
                                                                   'projects': {}}), stderr='')

        result = core.merge('claude', runner=runner)
        self.assertEqual(result['merged'], 1)
        self.assertEqual([i['text'] for i in core.read_inbox()], ['Late convention'])
        archived = Path(result['history']) / 'inbox.jsonl'
        self.assertEqual([json.loads(line)['text'] for line in archived.read_text().splitlines()], ['Initial convention'])
        core.merge('none')
        self.assertIn('Late convention', core.global_path().read_text())
        self.assertIn('Initial convention', core.global_path().read_text())
        self.assertEqual(core.read_inbox(), [])

    def test_second_merge_refuses_promptly_while_learning_remains_available(self):
        core.learn('Initial convention')
        started, release = threading.Event(), threading.Event()

        def runner(*args, **kwargs):
            started.set()
            if not release.wait(5):
                raise RuntimeError('test runner did not release the merge')
            return SimpleNamespace(returncode=0, stdout=json.dumps({'global': '- Initial convention', 'projects': {}}),
                                   stderr='')

        with ThreadPoolExecutor(max_workers=1) as pool:
            first = pool.submit(core.merge, 'claude', False, runner)
            try:
                self.assertTrue(started.wait(5))
                core.learn('Concurrent convention')
                with self.assertRaisesRegex(core.CoreError, 'merge is already running'):
                    core.merge('none')
            finally:
                release.set()
            self.assertEqual(first.result(timeout=5)['merged'], 1)
        self.assertEqual([i['text'] for i in core.read_inbox()], ['Concurrent convention'])

    def test_parallel_cli_learnings_produce_complete_unique_json_lines(self):
        repo = str(Path(__file__).resolve().parents[1])

        def learn(number):
            return subprocess.run([sys.executable, '-m', 'everett', 'learn', f'Convention number {number}'],
                                  cwd=self.home, env={**os.environ, 'PYTHONPATH': repo},
                                  text=True, capture_output=True, timeout=10)

        with ThreadPoolExecutor(max_workers=8) as pool:
            results = list(pool.map(learn, range(8)))
        self.assertTrue(all(r.returncode == 0 for r in results), [r.stderr for r in results])
        self.assertEqual({i['text'] for i in core.read_inbox()}, {f'Convention number {n}' for n in range(8)})
        self.assertEqual(len(core.inbox_path().read_text().splitlines()), 8)
        self.assertEqual(core.merge('none')['merged'], 8)

    def test_invalid_model_projects_do_not_change_core_or_consume_facts(self):
        core.learn('Original convention')
        core.merge('none')
        before = core.global_path().read_bytes()
        core.learn('Pending convention')
        result = SimpleNamespace(returncode=0, stdout='{"global":"new","projects":["invalid"]}', stderr='')
        with self.assertRaises(core.CoreError):
            core.merge('claude', runner=lambda *a, **kw: result)
        self.assertEqual(core.global_path().read_bytes(), before)
        self.assertEqual([i['text'] for i in core.read_inbox()], ['Pending convention'])

    def test_failed_model_preserves_both_initial_and_late_facts(self):
        core.learn('Initial convention')

        def runner(*args, **kwargs):
            core.learn('Late convention')
            raise subprocess.TimeoutExpired('claude', 1)

        with self.assertRaises(core.CoreError):
            core.merge('claude', runner=runner)
        self.assertEqual([i['text'] for i in core.read_inbox()], ['Initial convention', 'Late convention'])
        self.assertFalse(core.global_path().exists())

    def test_dry_run_creates_no_core_or_archive_files(self):
        core.learn('Initial convention')
        before = {p: p.read_bytes() for p in core.core_dir().rglob('*') if p.is_file()}
        self.assertEqual(core.merge('none', dry_run=True)['merged'], 1)
        after = {p: p.read_bytes() for p in core.core_dir().rglob('*') if p.is_file()}
        self.assertEqual(after, before)
