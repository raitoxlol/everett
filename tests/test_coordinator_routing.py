"""Coordinator vs worker routing: a session that only talks about a project (a coordinator
asking/listing other sessions) must not outrank a session that actually did the work on it.

Reproduces the real bug: `everett route "what's the status of kairos"` picking a coordinator
session in ~ over the worker session in ~/Kairos, for both the local BM25 router and Jev.
"""
import sandbox  # noqa: F401  (must be first: isolates HOME)

import time
import unittest

from everett.route import is_coordinator, local_route, relevant_sessions, route
from everett.session import Session

NOW = time.time()

COORDINATOR = Session(
    'claude', 'coord-1', '/Users/alice', '/tmp/coord-1.jsonl', '', NOW - 300,
    title='Session sweep',
    card='Coordinator session: using Everett to list sessions and ask the Kairos session about its status.',
    first_user='use everett to check on my other sessions',
    last_user='everett_send: asking the kairos session for a status update',
)

WORKER = Session(
    'codex', 'work-1', '/Users/alice/Kairos', '/tmp/work-1.jsonl', '', NOW - 3600,
    title='Kairos ingest pipeline',
    card='Implementing the Kairos ingest pipeline; wiring up the scheduler next.',
    first_user='build the kairos ingest pipeline',
    last_user='wired up the scheduler for kairos ingest',
)

CALLER = Session(
    'claude', 'caller-1', '/Users/alice', '/tmp/caller-1.jsonl', '', NOW - 10,
    title='Routing question', card='Asking everett to route a request about kairos.',
)


class CoordinatorDetection(unittest.TestCase):
    def test_coordinator_card_detected(self):
        self.assertTrue(is_coordinator(COORDINATOR))

    def test_worker_card_not_coordinator(self):
        self.assertFalse(is_coordinator(WORKER))


class RelevantSessions(unittest.TestCase):
    def test_coordinator_excluded_when_project_not_named_by_it(self):
        pool = relevant_sessions("what's the status of kairos", [COORDINATOR, WORKER])
        self.assertNotIn(COORDINATOR, pool)
        self.assertIn(WORKER, pool)

    def test_caller_session_always_excluded(self):
        pool = relevant_sessions("what's the status of kairos", [WORKER, CALLER], caller_id='caller-1')
        self.assertNotIn(CALLER, pool)

    def test_never_stranded_when_only_coordinators_exist(self):
        # No worker at all: fall back to the (still ranked) coordinator rather than returning nothing.
        pool = relevant_sessions("what's the status of kairos", [COORDINATOR])
        self.assertIn(COORDINATOR, pool)


class LocalRouterPicksTheWorker(unittest.TestCase):
    def test_worker_session_wins_over_coordinator(self):
        r = local_route("what's the status of kairos", [COORDINATOR, WORKER], now=NOW)
        if r['decision'] == 'SESSION':
            self.assertEqual(r['session']['id'], 'work-1')
        else:
            # If still unsure, it must not silently prefer the coordinator either.
            self.assertEqual(r['decision'], 'ASK')
            self.assertIn('Kairos', r.get('suggested', '') or '')

    def test_caller_session_excluded_from_local_route(self):
        r = local_route("what's the status of kairos", [COORDINATOR, WORKER, CALLER],
                        now=NOW, caller_id='caller-1')
        if r['decision'] == 'SESSION':
            self.assertNotEqual(r['session']['id'], 'caller-1')


class JevRouterPicksTheWorker(unittest.TestCase):
    """A fake Jev backend: it only ever sees the criteria route() hands it, so this also proves
    the coordinator was filtered out (or ranked worse) before Jev is even called."""

    def test_worker_only_candidate_when_coordinator_is_not_named(self):
        seen = {}

        def fake_jev(text, crit, key):
            seen['crit'] = crit
            # Pick whichever option's description is about the actual Kairos work.
            for option, description in crit.items():
                if 'ingest pipeline' in description:
                    return {'choice': option, 'confidence': 0.9}
            return {'choice': 'none', 'confidence': 0.0}

        r = route("what's the status of kairos", [COORDINATOR, WORKER], api_key='k', jev=fake_jev)
        self.assertEqual(r['decision'], 'SESSION')
        self.assertEqual(r['session']['id'], 'work-1')
        # The coordinator must not have been offered to Jev as a plain candidate.
        self.assertFalse(any('coord-1' in v for k, v in seen['crit'].items() if k not in ('new', 'none')))

    def test_caller_session_excluded_from_jev_criteria(self):
        seen = {}

        def fake_jev(text, crit, key):
            seen['crit'] = crit
            return {'choice': 'none', 'confidence': 0.0}

        route("what's the status of kairos", [COORDINATOR, WORKER, CALLER], api_key='k',
              jev=fake_jev, caller_id='caller-1')
        self.assertNotIn('caller-1', ' '.join(seen['crit'].values()))


if __name__ == '__main__':
    unittest.main()
