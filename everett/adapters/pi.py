"""Pi coding-agent sessions: ~/.pi/agent/sessions/<cwd-slug>/*.jsonl.

Pi and OMP share one JSONL format (a `session` header line, then `message` lines), so this
reuses the OMP parser.
"""
from __future__ import annotations

from ..session import Session, home
from .omp import scan_root


def scan(since_hours: float = 72) -> list[Session]:
    return scan_root(home() / '.pi' / 'agent' / 'sessions', 'pi', since_hours)
