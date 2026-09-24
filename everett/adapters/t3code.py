"""T3 Code threads, read-only from ~/.t3/userdata/state.sqlite (SQLite `mode=ro`, never written).

T3 Code is a GUI that drives Codex, Claude Code (through the Agent SDK) and Grok. It does not keep
its own transcripts: each thread's `provider_session_runtime.resume_cursor_json` names the
underlying harness session, which that harness writes to its usual store:

- codex:       {"threadId": "<codex session id>"}
- claudeAgent: {"resume": "<claude session id>", "resumeSessionAt": "<message uuid>", ...}
- grok:        {"sessionId": "<grok session id>"}

So T3 threads are not listed on their own. The matching harness sessions are annotated with
source "t3code", and the thread title fills in when the session has no title of its own.
(`~/Library/Application Support/t3code` holds only the desktop shell's browser data.)
"""
from __future__ import annotations

import json
import sqlite3
from pathlib import Path

from ..session import Session, clean, home, warn

SOURCE = 't3code'
CURSOR_KEYS = {'codex': ('codex', 'threadId'), 'claudeAgent': ('claude', 'resume'), 'grok': ('grok', 'sessionId')}


def db_path() -> Path:
    return home() / '.t3' / 'userdata' / 'state.sqlite'


def _connect(path: Path) -> sqlite3.Connection:
    return sqlite3.connect(f'{path.resolve().as_uri()}?mode=ro', uri=True, timeout=1)


def threads(path: Path | None = None) -> dict[tuple[str, str], dict]:
    """{(harness, session id): {'thread_id', 'title'}} for every T3 thread with a provider session."""
    path = path or db_path()
    if not path.is_file():
        return {}
    try:
        con = _connect(path)
    except sqlite3.Error as exc:
        warn(f'skip {path}: {exc}')
        return {}
    try:
        rows = con.execute(
            'SELECT r.thread_id, r.provider_name, r.resume_cursor_json, t.title '
            'FROM provider_session_runtime r LEFT JOIN projection_threads t ON t.thread_id = r.thread_id'
        ).fetchall()
    except sqlite3.Error as exc:
        warn(f'skip {path}: {exc}')
        return {}
    finally:
        con.close()
    out: dict[tuple[str, str], dict] = {}
    for thread_id, provider, cursor, title in rows:
        harness, key = CURSOR_KEYS.get(provider, ('', ''))
        if not harness:
            continue
        try:
            session_id = (json.loads(cursor or '{}') or {}).get(key)
        except (ValueError, AttributeError):
            continue
        if isinstance(session_id, str) and session_id:
            out[(harness, session_id)] = {'thread_id': thread_id, 'title': title or ''}
    return out


def annotate(sessions: list[Session], found: dict[tuple[str, str], dict] | None = None) -> None:
    """Mark sessions T3 Code drives; its thread title is the fallback headline."""
    found = threads() if found is None else found
    if not found:
        return
    for s in sessions:
        thread = found.get((s.harness, s.id))
        if thread is None:
            continue
        s.source = SOURCE
        if not s.title and thread['title']:
            s.title = clean(thread['title'], 120)
