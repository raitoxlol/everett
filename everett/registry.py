from __future__ import annotations

import json
import subprocess
import time

from . import cards
from .adapters import claude, codex, grok, hermes, omp, pi, t3code
from .session import Session, home

ADAPTERS = {'claude': claude, 'codex': codex, 'omp': omp, 'pi': pi, 'hermes': hermes, 'grok': grok}
LIVE_WINDOW = 600  # seconds
CAP = 40
AUTO_PREFIXES = ('You are ', 'Automation:')  # scheduled/scripted runs


def spawned_ids() -> set[str]:
    """Sessions Everett started headless: shown like interactive ones."""
    ids = set()
    try:
        with (home() / '.everett' / 'spawned.jsonl').open(encoding='utf-8') as f:
            for line in f:
                try:
                    ids.add(json.loads(line)['id'])
                except (ValueError, KeyError, TypeError):
                    continue
    except OSError:
        pass
    return ids


def is_auto(s: Session, spawned: set[str] = frozenset()) -> bool:
    if s.id in spawned:
        return False
    return s.auto or s.first_user.startswith(AUTO_PREFIXES)


def _ps() -> str:
    try:
        return subprocess.run(['ps', '-axo', 'command'], capture_output=True, text=True, timeout=2).stdout
    except Exception:  # noqa: BLE001
        return ''


def mark_running(sessions: list[Session], ps_out: str, now: float) -> None:
    """Running = the adapter saw it open (Grok), its id is in a live process, or it was written in the last 10 min."""
    for s in sessions:
        s.running = s.running or session_running(s, ps_out, now)


def session_running(s: Session, ps_out: str | None = None, now: float | None = None) -> bool:
    """Recheck one session's running state immediately before a send."""
    if ps_out is None:
        ps_out = _ps()
    if now is None:
        now = time.time()
    return (bool(s.id) and s.id in ps_out) or (now - s.last_active) < LIVE_WINDOW


def scan(since_hours: float = 72, include_auto: bool = False,
         limit: int | None = CAP, harness: str = '') -> list[Session]:
    """Recent sessions, newest first; `harness` filters before the cap, not after."""
    sessions: list[Session] = []
    for name, mod in ADAPTERS.items():
        if not harness or name == harness:
            sessions.extend(mod.scan(since_hours))
    t3code.annotate(sessions)  # T3 Code threads are Codex/Claude/Grok sessions underneath
    for s in sessions:
        if not s.first_user:  # first real ask can sit past the head chunk in huge files
            s.first_user = s.last_user
    if not include_auto:
        spawned = spawned_ids()
        sessions = [s for s in sessions if not is_auto(s, spawned)]
    sessions.sort(key=lambda s: s.last_active, reverse=True)
    if limit is not None:
        sessions = sessions[:limit]
    mark_running(sessions, _ps(), time.time())
    cards.apply(sessions)
    return sessions


class SessionLookupError(Exception):
    pass


def _name(s: Session) -> str:
    """The short name a card or title gives a session ("Kairos: ..." -> "kairos")."""
    text = s.card or s.title or ''
    text = text.split('What:', 1)[-1].strip()
    head = text.split(':', 1)[0] if ':' in text[:40] else ''
    return head.strip().casefold()


def find(target: str, sessions: list[Session]) -> Session:
    """Resolve `send --to`: exact id, unique id prefix, or unique card/title name or project folder."""
    target = target.strip()
    if not target:
        raise SessionLookupError('--to needs a session id prefix or name.')
    exact = [s for s in sessions if s.id == target]
    if exact:
        return exact[0]
    for matcher in (lambda s: s.id.startswith(target),
                    lambda s: _name(s) == target.casefold(),
                    lambda s: (s.cwd.rstrip('/').rsplit('/', 1)[-1].casefold() == target.casefold())):
        hits = [s for s in sessions if matcher(s)]
        if len(hits) == 1:
            return hits[0]
        if len(hits) > 1:
            lines = '\n'.join(f'  {s.id}  [{s.harness}] {s.cwd} — {(s.card or s.title or s.first_user)[:70]}'
                              for s in hits[:10])
            raise SessionLookupError(f'"{target}" matches {len(hits)} sessions; use a longer id prefix:\n{lines}')
    raise SessionLookupError(f'no session matches "{target}" in the look-back window (try --hours).')
