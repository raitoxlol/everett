from __future__ import annotations

from ..external import records
from ..session import Session


def scan(harness: str = '') -> list[Session]:
    sessions = []
    for record in records():
        if harness and record.harness != harness:
            continue
        sessions.append(Session(record.harness, record.id, record.cwd, '', '', record.updated,
                                title=record.title, source='external'))
    return sessions
