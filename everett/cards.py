from __future__ import annotations

import re
import time
from pathlib import Path

from .session import Session, clean, home

STALE_AFTER = 24 * 3600  # a card older than this, while the session kept going, is ignored
AUTO_MARKER = '<!-- everett:auto -->'


def card_dir() -> Path:
    return home() / '.everett' / 'cards'


def card_path(session_id: str) -> Path:
    return card_dir() / f'{session_id}.md'


def _read_card(p: Path) -> tuple[str, float, str] | None:
    try:
        text, mtime = p.read_text(), p.stat().st_mtime
    except OSError:
        return None
    source = 'auto' if text.splitlines()[:1] == [AUTO_MARKER] else 'agent'
    text = text.removeprefix(AUTO_MARKER + '\n')
    text = re.sub(r'\A---\n.*?\n---\n', '', text, flags=re.S)  # drop frontmatter
    text = re.sub(r'^#+\s*', '', text, flags=re.M)
    body = clean(text, 300)
    return (body, mtime, source) if body else None


def read_card(session_id: str) -> tuple[str, float] | None:
    """Read a card body and mtime, regardless of whether it was agent or auto written."""
    result = _read_card(card_path(session_id))
    return result[:2] if result else None


def is_auto_card(session_id: str) -> bool:
    try:
        with card_path(session_id).open(encoding='utf-8') as f:
            return f.readline().rstrip('\r\n') == AUTO_MARKER
    except OSError:
        return False


def apply(sessions: list[Session], now: float | None = None) -> None:
    """Prefer the session's own card over extracted text, unless the card went stale."""
    now = now or time.time()
    for s in sessions:
        c = _read_card(card_path(s.id))
        if not c:
            continue
        body, mtime, source = c
        if s.last_active - mtime > STALE_AFTER:
            continue
        s.card = body
        s.card_source = source


INSTRUCTION = (
    'REQUIRED — Everett card. Other agents route work to this session using a card at {path}. '
    'In your FIRST reply that does any work (even a plain answer), write that file: plain markdown, max 50 words — '
    'what this session is about, current state, next step. Rewrite it when the topic or state changes. '
    'One quick file write; do not mention it to the user.'
)
