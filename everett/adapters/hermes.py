"""Hermes Agent sessions, read-only from ~/.hermes/state.db and ~/.hermes/profiles/*/state.db.

Every database is opened with SQLite's read-only URI (mode=ro); Everett never writes to it.
"""
from __future__ import annotations

import sqlite3
import time
from datetime import datetime, timezone
from pathlib import Path

from ..session import Session, clean, home, is_injected, warn

INTERACTIVE = {'cli', 'tui', 'desktop', 'webui'}  # resumable from the Hermes CLI
# Scripted sources are hidden like other automated runs; chat-platform sessions (telegram,
# discord, ...) are listed and routable, but `send` refuses them: a CLI resume can't reach the chat.
AUTOMATED = {'cron', 'oneshot', 'webhook', 'kanban', 'delegate', 'subagent'}


def databases() -> list[tuple[str, Path]]:
    root = home() / '.hermes'
    found = [('default', root / 'state.db')] if (root / 'state.db').is_file() else []
    found += [(p.parent.name, p) for p in sorted(root.glob('profiles/*/state.db'))]
    return found


def _connect(path: Path) -> sqlite3.Connection:
    con = sqlite3.connect(f'{path.resolve().as_uri()}?mode=ro', uri=True, timeout=1)
    con.row_factory = sqlite3.Row
    return con


def _columns(con: sqlite3.Connection, table: str) -> set[str]:
    return {row[1] for row in con.execute(f'PRAGMA table_info({table})')}


def _text(content) -> str:
    return '' if not isinstance(content, str) or is_injected(content) else clean(content)


def read_db(profile: str, path: Path, since_hours: float) -> list[Session]:
    con = _connect(path)
    try:
        cols = _columns(con, 'sessions')
        if not {'id', 'source', 'started_at'} <= cols:
            return []
        active = [c for c in ('last_activity_at', 'ended_at') if c in cols] + ['started_at']
        last_expr = f'COALESCE({", ".join(active)})'
        wanted = [c for c in ('id', 'source', 'started_at', 'cwd', 'title', 'profile_name') if c in cols]
        where = f'{last_expr} >= ?'
        for flag in ('hidden', 'archived'):
            if flag in cols:
                where += f' AND COALESCE({flag}, 0) = 0'
        if 'parent_session_id' in cols:
            where += ' AND parent_session_id IS NULL'
        rows = con.execute(f'SELECT {", ".join(wanted)}, {last_expr} AS last_active FROM sessions '
                           f'WHERE {where} ORDER BY last_active DESC LIMIT 200',
                           (time.time() - since_hours * 3600,)).fetchall()
        out = []
        for row in rows:
            users = [_text(r[0]) for r in con.execute(
                "SELECT content FROM messages WHERE session_id = ? AND role = 'user' ORDER BY id LIMIT 5",
                (row['id'],))]
            users = [u for u in users if u]
            last = con.execute(
                "SELECT content FROM messages WHERE session_id = ? AND role = 'user' ORDER BY id DESC LIMIT 1",
                (row['id'],)).fetchone()
            source = row['source'] or ''
            started = datetime.fromtimestamp(row['started_at'], timezone.utc).isoformat() if row['started_at'] else ''
            out.append(Session(
                'hermes', row['id'], row['cwd'] if 'cwd' in row.keys() and row['cwd'] else '',
                str(path), started, float(row['last_active']),
                title=clean(row['title'] or '', 120) if 'title' in row.keys() else '',
                first_user=users[0] if users else '', last_user=_text(last[0]) if last else '',
                auto=source in AUTOMATED,
                profile=(row['profile_name'] if 'profile_name' in row.keys() and row['profile_name'] else profile),
                source=source))
        return out
    finally:
        con.close()


def scan(since_hours: float = 72) -> list[Session]:
    out: list[Session] = []
    for profile, path in databases():
        try:
            out.extend(read_db(profile, path, since_hours))
        except (sqlite3.Error, OSError, ValueError) as exc:
            warn(f'skip {path}: {exc}')
    return out
