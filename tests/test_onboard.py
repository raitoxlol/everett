import sandbox  # noqa: F401  (must be first: isolates HOME)

import argparse
import io
import json
import os
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from everett import cards, install, onboard
from everett.cli import main
from everett.session import home


class IsolatedHome(unittest.TestCase):
    """Each test gets its own fake HOME (sandbox.py only isolates the whole suite from the real one;
    onboarding writes real files, so tests must not share state with each other)."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self._patch = mock.patch.dict(os.environ, {'HOME': self._tmp.name, 'EVERETT_HOME': self._tmp.name})
        self._patch.start()
        self.addCleanup(self._patch.stop)

    def write_claude_session(self, sid: str, cwd: str, text: str, days_old: float = 0) -> Path:
        root = home() / '.claude' / 'projects' / cwd.strip('/').replace('/', '-')
        root.mkdir(parents=True, exist_ok=True)
        path = root / f'{sid}.jsonl'
        rows = [
            {'type': 'user', 'sessionId': sid, 'cwd': cwd, 'timestamp': '2026-09-20T00:00:00Z',
             'message': {'role': 'user', 'content': text}},
            {'type': 'assistant', 'sessionId': sid,
             'message': {'role': 'assistant',
                         'content': [{'type': 'text', 'text': f'Working on it. Next: ship {sid}.'}]}},
        ]
        path.write_text('\n'.join(json.dumps(r) for r in rows) + '\n', encoding='utf-8')
        mtime = time.time() - days_old * 86400
        os.utime(path, (mtime, mtime))
        return path


def _args(**overrides):
    ns = argparse.Namespace(yes=False, span_days=3, no_backfill=False, no_mcp=False)
    for k, v in overrides.items():
        setattr(ns, k, v)
    return ns


class NonInteractivePath(IsolatedHome):
    def setUp(self):
        super().setUp()
        self.write_claude_session('c-fresh', '/work/app', 'fix the login bug')

    def test_yes_end_to_end(self):
        out = io.StringIO()
        with redirect_stdout(out):
            rc = main(['onboard', '--yes'])
        self.assertEqual(rc, 0)
        text = out.getvalue()
        self.assertIn('Everett onboarding complete.', text)
        settings = json.loads(install.settings_path('claude').read_text())
        self.assertIn('SessionStart', settings['hooks'])
        self.assertIn('Stop', settings['hooks'])
        self.assertTrue(install.mcp_installed('claude'))
        self.assertIn('1 card(s) written', text)
        card = cards.card_path('c-fresh').read_text()
        self.assertTrue(card.startswith('<!-- everett:auto -->'))

    def test_no_backfill_flag_skips_generation(self):
        out = io.StringIO()
        with redirect_stdout(out):
            main(['onboard', '--yes', '--no-backfill'])
        self.assertFalse(cards.card_path('c-fresh').exists())

    def test_no_mcp_flag_skips_registration(self):
        main(['onboard', '--yes', '--no-mcp'])
        self.assertFalse(install.mcp_installed('claude'))
        self.assertTrue(install.installed('claude')['SessionStart'])

    def test_bad_span_days_rejected(self):
        rc = onboard.run(_args(yes=True, span_days=31))
        self.assertEqual(rc, 2)
        rc = onboard.run(_args(yes=True, span_days=0))
        self.assertEqual(rc, 2)

    def test_second_run_is_idempotent(self):
        main(['onboard', '--yes'])
        out = io.StringIO()
        with redirect_stdout(out):
            rc = main(['onboard', '--yes'])
        self.assertEqual(rc, 0)
        self.assertIn('already installed', out.getvalue())
        self.assertIn('0 card(s) written', out.getvalue())


class BackfillSpanAndSafety(IsolatedHome):
    def test_respects_span(self):
        self.write_claude_session('c-inside', '/work/a', 'inside span', days_old=1)
        self.write_claude_session('c-outside', '/work/b', 'outside span', days_old=10)
        result = onboard.run_apply(onboard.OnboardConfig(hooks={}, mcp={}, backfill_enabled=True, span_days=3))
        self.assertEqual(result.backfill_written, 1)
        self.assertTrue(cards.card_path('c-inside').exists())
        self.assertFalse(cards.card_path('c-outside').exists())

    def test_never_overwrites_agent_card(self):
        self.write_claude_session('c-agent', '/work/agent', 'has a real card')
        cards.card_path('c-agent').parent.mkdir(parents=True, exist_ok=True)
        cards.card_path('c-agent').write_text('Kairos: agent-written, do not touch.', encoding='utf-8')
        written, skipped = onboard.generate_backfill_cards(onboard.missing_card_sessions(3))
        self.assertEqual(written, 0)
        self.assertEqual(skipped, 0)  # the agent-carded session isn't in the "missing" list at all
        self.assertEqual(cards.card_path('c-agent').read_text(), 'Kairos: agent-written, do not touch.')

    def test_missing_card_sessions_excludes_cards_already_written(self):
        self.write_claude_session('c-x', '/work/x', 'needs a card')
        before = onboard.missing_card_sessions(3)
        self.assertEqual({s.id for s in before}, {'c-x'})
        onboard.generate_backfill_cards(before)
        after = onboard.missing_card_sessions(3)
        self.assertEqual(after, [])

    def test_progress_callback_invoked_per_session(self):
        self.write_claude_session('c-p1', '/work/p1', 'one')
        self.write_claude_session('c-p2', '/work/p2', 'two')
        seen = []
        onboard.generate_backfill_cards(onboard.missing_card_sessions(3), progress=lambda i, t: seen.append((i, t)))
        self.assertEqual(seen, [(1, 2), (2, 2)])


class QuittingWritesNothing(IsolatedHome):
    def setUp(self):
        super().setUp()
        self.write_claude_session('c-quit', '/work/app', 'should not be touched')

    def test_tui_quit_leaves_no_changes(self):
        out = io.StringIO()
        with redirect_stdout(out), \
             mock.patch('everett.onboard.run_tui', side_effect=onboard.QuitOnboarding), \
             mock.patch('sys.stdin.isatty', return_value=True), \
             mock.patch('sys.stdout.isatty', return_value=True):
            rc = onboard.run(_args())
        self.assertEqual(rc, 0)
        self.assertIn('Cancelled', out.getvalue())
        self.assertFalse(install.settings_path('claude').exists())
        self.assertFalse(install.mcp_installed('claude'))
        self.assertFalse(cards.card_path('c-quit').exists())

    def test_plain_prompt_quit_leaves_no_changes(self):
        with mock.patch('sys.stdin.isatty', return_value=False), mock.patch('builtins.input', return_value='q'):
            out = io.StringIO()
            with redirect_stdout(out):
                rc = onboard.run(_args())
        self.assertEqual(rc, 0)
        self.assertFalse(install.settings_path('claude').exists())
        self.assertFalse(cards.card_path('c-quit').exists())

    def test_plain_prompt_no_to_apply_writes_nothing(self):
        answers = iter(['', '', '', '', '', '', '', '', 'n'])  # hooks x4, mcp x1, backfill on, span default, decline apply
        with mock.patch('builtins.input', side_effect=lambda *_: next(answers)):
            out = io.StringIO()
            with redirect_stdout(out):
                rc = onboard.run_plain(_args())
        self.assertEqual(rc, 0)
        self.assertIn('Nothing was changed.', out.getvalue())
        self.assertFalse(install.settings_path('claude').exists())
        self.assertFalse(cards.card_path('c-quit').exists())


class PlainPromptFallback(IsolatedHome):
    def setUp(self):
        super().setUp()
        self.write_claude_session('c-plain', '/work/app', 'plain path session')

    def test_used_when_not_a_tty(self):
        with mock.patch('sys.stdin.isatty', return_value=False):
            answers = iter(['y', 'y', 'y', 'y', 'y', 'y', 'y', '', 'y'])
            with mock.patch('builtins.input', side_effect=lambda *_: next(answers)):
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = onboard.run(_args())
        self.assertEqual(rc, 0)
        self.assertIn('Everett onboarding complete.', out.getvalue())
        self.assertTrue(install.settings_path('claude').exists())
        self.assertTrue(cards.card_path('c-plain').exists())

    def test_falls_back_when_curses_raises(self):
        with mock.patch('sys.stdin.isatty', return_value=True), \
             mock.patch('sys.stdout.isatty', return_value=True), \
             mock.patch('everett.onboard.run_tui', side_effect=RuntimeError('curses unavailable')):
            with mock.patch('builtins.input', return_value='n'):
                out = io.StringIO()
                with redirect_stdout(out):
                    rc = onboard.run(_args())
        self.assertEqual(rc, 0)
        self.assertFalse(install.settings_path('claude').exists())


class Detection(IsolatedHome):
    def test_detect_only_lists_existing_stores(self):
        self.assertEqual(onboard.detect_harnesses(), {})
        self.write_claude_session('c-det', '/work/app', 'hello')
        self.assertEqual(onboard.detect_harnesses(), {'claude': 1})

    def test_default_config_selects_everything_detected(self):
        self.write_claude_session('c-def', '/work/app', 'hello')
        cfg = onboard.default_config()
        self.assertEqual(cfg.hooks['claude'], {'SessionStart': True, 'Stop': True, 'UserPromptSubmit': True,
                                               'PostToolUse': True})
        self.assertTrue(cfg.mcp['claude'])
        self.assertNotIn('codex', cfg.hooks)
        self.assertNotIn('omp', cfg.mcp)


class LayoutHelpers(unittest.TestCase):
    """Pure functions behind the curses frame -- no curses import required to test them."""

    def test_step_indicator(self):
        self.assertEqual(onboard.step_indicator(2, 6), 'step 2 of 6')
        self.assertEqual(onboard.step_indicator(1, 1), 'step 1 of 1')

    def test_progress_rail(self):
        self.assertEqual(onboard.progress_rail(2, 6), '●●○○○○')
        self.assertEqual(onboard.progress_rail(0, 3), '○○○')
        self.assertEqual(onboard.progress_rail(3, 3), '●●●')

    def test_progress_rail_clamps(self):
        self.assertEqual(onboard.progress_rail(-1, 3), '○○○')
        self.assertEqual(onboard.progress_rail(9, 3), '●●●')

    def test_use_ascii_glyphs(self):
        self.assertTrue(onboard.use_ascii_glyphs('ascii'))
        self.assertTrue(onboard.use_ascii_glyphs(None))
        self.assertTrue(onboard.use_ascii_glyphs('ANSI_X3.4-1968'))
        self.assertFalse(onboard.use_ascii_glyphs('utf-8'))
        self.assertFalse(onboard.use_ascii_glyphs('UTF-8'))

    def test_glyph_unicode(self):
        self.assertEqual(onboard.glyph(True, ascii_mode=False), '☑')
        self.assertEqual(onboard.glyph(False, ascii_mode=False), '☐')

    def test_glyph_ascii_fallback(self):
        self.assertEqual(onboard.glyph(True, ascii_mode=True), '[x]')
        self.assertEqual(onboard.glyph(False, ascii_mode=True), '[ ]')

    def test_stepper_text(self):
        self.assertEqual(onboard.stepper_text(3), '‹ 3 days ›')
        self.assertEqual(onboard.stepper_text(1), '‹ 1 day ›')
        self.assertEqual(onboard.stepper_text(30, unit='day'), '‹ 30 days ›')

    def test_animation_disabled_respects_env(self):
        with mock.patch.dict(os.environ, {'NO_COLOR': '', 'EVERETT_NO_ANIM': ''}, clear=False):
            os.environ.pop('NO_COLOR', None)
            os.environ.pop('EVERETT_NO_ANIM', None)
            self.assertFalse(onboard.animation_disabled())
        with mock.patch.dict(os.environ, {'NO_COLOR': '1'}):
            self.assertTrue(onboard.animation_disabled())
        with mock.patch.dict(os.environ, {'EVERETT_NO_ANIM': '1'}):
            self.assertTrue(onboard.animation_disabled())


if __name__ == '__main__':
    unittest.main()
