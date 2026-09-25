"""Jev first: routing is the default path for everett_send / `everett send`, not `to`.

Covers: routing when `to` is omitted (with a fake Jev), the ASK path returning candidates without
sending, and `to` still bypassing routing entirely.
"""
import sandbox  # noqa: F401  (must be first: isolates HOME)

import contextlib
import io
import json
import unittest
from types import SimpleNamespace
from unittest import mock

from everett import mcp
from everett.adapters import claude, codex, omp
from everett.cli import main
from everett.route import route
from everett.send import SendResult
from everett.session import Session
from pathlib import Path

FX = Path(__file__).parent / 'fixtures'


def fake_jev(choice: str, confidence: float):
    """A fake Jev backend: `route(..., jev=fake_jev(...))` answers with a fixed choice/confidence."""
    return lambda text, crit, key: {'choice': choice, 'confidence': confidence}


class RouteCandidates(unittest.TestCase):
    """route() itself: ASK carries candidates, for both routers."""

    def setUp(self):
        self.sessions = [claude.parse(FX / 'claude.jsonl'), omp.parse(FX / 'omp.jsonl')]

    def test_local_ask_returns_candidates(self):
        r = route('implement fix', self.sessions, api_key='')
        self.assertEqual(r['decision'], 'ASK')
        self.assertEqual(r['router'], 'local')
        self.assertEqual(len(r['candidates']), 2)

    def test_jev_ask_returns_candidates(self):
        r = route('something', self.sessions, api_key='k', jev=fake_jev('s0', 0.3))
        self.assertEqual(r['decision'], 'ASK')
        self.assertEqual(r['router'], 'jev')
        self.assertEqual(len(r['candidates']), 1)
        self.assertIn('claude', r['candidates'][0])


class McpSendRouting(unittest.TestCase):
    """everett_send (MCP tool): routes by default, reports the decision, `to` skips it."""

    def setUp(self):
        self.session = Session('claude', 'c-1', '/work/app', '/tmp/c-1.jsonl', '', 0, card='API work')

    def test_routes_when_to_is_omitted_and_reports_the_decision(self):
        routed = {'decision': 'SESSION', 'choice': 's0', 'confidence': 0.87, 'router': 'jev',
                  'session': self.session.to_dict(), 'command': 'cd /work/app && claude --resume c-1 "x"'}
        with mock.patch('everett.mcp.registry.scan', return_value=[self.session]), mock.patch(
            'everett.mcp.route', return_value=routed) as route_mock, mock.patch(
            'everett.mcp.delivery_mode', return_value='resume'), mock.patch(
            'everett.mcp.command_for', return_value=['claude', '--resume', 'c-1', '--print', 'x']), mock.patch(
            'everett.mcp.send', return_value=SendResult(['claude'], 'done')) as send_mock:
            out = mcp.tool_send({'text': 'the upload retries should cap at 5'})
        route_mock.assert_called_once()
        send_mock.assert_called_once()
        self.assertEqual(out['router'], 'jev')
        self.assertEqual(out['confidence'], 0.87)
        self.assertEqual(out['decision'], 'SESSION')
        self.assertEqual(out['session']['id'], 'c-1')
        self.assertEqual(out['reply'], 'done')

    def test_ask_returns_candidates_without_sending(self):
        routed = {'decision': 'ASK', 'choice': 'none', 'confidence': 0.3, 'router': 'local',
                  'suggested': '[claude] /work/app — API work',
                  'candidates': ['[claude] /work/app — API work', '[omp] /work/infra — Infra work']}
        with mock.patch('everett.mcp.registry.scan', return_value=[self.session]), mock.patch(
            'everett.mcp.route', return_value=routed), mock.patch('everett.mcp.send') as send_mock:
            out = mcp.tool_send({'text': 'vague thing'})
        send_mock.assert_not_called()
        self.assertFalse(out['delivered'])
        self.assertEqual(out['decision'], 'ASK')
        self.assertEqual(len(out['candidates']), 2)

    def test_to_still_bypasses_routing(self):
        with mock.patch('everett.mcp.registry.scan', return_value=[self.session]), mock.patch(
            'everett.mcp.registry.find', return_value=self.session), mock.patch(
            'everett.mcp.route') as route_mock, mock.patch(
            'everett.mcp.delivery_mode', return_value='resume'), mock.patch(
            'everett.mcp.command_for', return_value=['claude', '--resume', 'c-1', '--print', 'x']), mock.patch(
            'everett.mcp.send', return_value=SendResult(['claude'], 'done')) as send_mock:
            out = mcp.tool_send({'text': 'fix the thing', 'to': 'c-1'})
        route_mock.assert_not_called()
        send_mock.assert_called_once()
        self.assertEqual(out['router'], 'direct')
        self.assertEqual(out['confidence'], 1.0)
        self.assertTrue(out['delivered'])


class CliSendRouting(unittest.TestCase):
    """`everett send`: same routing-by-default behavior, plus the printed "routed by ..." line."""

    def setUp(self):
        self.session = Session('claude', 'c-1', '/work/app', '/tmp/c-1.jsonl', '', 0, card='API work')
        self.routed = {'decision': 'SESSION', 'choice': 's0', 'confidence': 0.87, 'router': 'jev',
                       'session': self.session.to_dict(), 'command': 'cd /work/app && claude --resume c-1 "x"'}

    def test_prints_routed_by_line(self):
        output = io.StringIO()
        with mock.patch('everett.cli.registry.scan', return_value=[self.session]), mock.patch(
            'everett.cli.route', return_value=self.routed), mock.patch(
            'everett.cli.delivery_mode', return_value='resume'), mock.patch(
            'everett.cli.send', return_value=SendResult(['claude'], 'done')), contextlib.redirect_stdout(output):
            code = main(['send', 'the upload retries should cap at 5'])
        self.assertEqual(code, 0)
        text = output.getvalue()
        self.assertIn('routed by jev', text)
        self.assertIn('0.87', text)
        self.assertIn('SENT', text)

    def test_to_bypasses_routing_and_prints_no_routed_line(self):
        output = io.StringIO()
        with mock.patch('everett.cli.registry.find', return_value=self.session), mock.patch(
            'everett.cli.registry.scan', return_value=[self.session]), mock.patch(
            'everett.cli.route') as route_mock, mock.patch(
            'everett.cli.delivery_mode', return_value='resume'), mock.patch(
            'everett.cli.send', return_value=SendResult(['claude'], 'done')), contextlib.redirect_stdout(output):
            code = main(['send', '--to', 'c-1', 'fix the thing'])
        self.assertEqual(code, 0)
        route_mock.assert_not_called()
        self.assertNotIn('routed by', output.getvalue())

    def test_ask_prints_candidates_and_sends_nothing(self):
        ask = {'decision': 'ASK', 'choice': 'none', 'confidence': 0.3, 'router': 'local',
               'suggested': '[claude] /work/app — API work',
               'candidates': ['[claude] /work/app — API work']}
        output = io.StringIO()
        with mock.patch('everett.cli.registry.scan', return_value=[self.session]), mock.patch(
            'everett.cli.route', return_value=ask), mock.patch('everett.cli.send') as send_mock, \
            contextlib.redirect_stdout(output):
            code = main(['send', 'vague thing'])
        self.assertEqual(code, 0)
        send_mock.assert_not_called()
        self.assertIn('candidate:', output.getvalue())


if __name__ == '__main__':
    unittest.main()
