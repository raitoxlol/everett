"""Read-only T3 projections, joined to real native provider session IDs.

V2 schema/JSON fields follow pingdotgg/t3code at 43f8a8de17a7ac1baa7a3cf36d681856de2d8add:
Migrations/055_OrchestrationV2.ts and packages/contracts/src/orchestrationV2.ts.
Provider transcripts remain authoritative; projections fill in missing local sessions.
"""
from __future__ import annotations

import json
import sqlite3
import time
from datetime import datetime, timezone
from pathlib import Path

from ..session import Session, clean, home, warn

SOURCE = 't3code'
CURSOR_KEYS = {'codex': ('codex', 'threadId'), 'claudeAgent': ('claude', 'resume'), 'grok': ('grok', 'sessionId')}


def db_path() -> Path:
    state = home() / '.t3' / 'userdata'
    current = state / 'statev2.sqlite'
    return current if current.is_file() else state / 'state.sqlite'


def _connect(path: Path) -> sqlite3.Connection:
    return sqlite3.connect(f'{path.resolve().as_uri()}?mode=ro', uri=True, timeout=1)


def _object(value: str | None) -> dict:
    try:
        parsed = json.loads(value or '{}')
    except (ValueError, TypeError):
        return {}
    return parsed if isinstance(parsed, dict) else {}


def _epoch(value: str) -> float:
    try:
        dt = datetime.fromisoformat(value.replace('Z', '+00:00'))
        return dt.replace(tzinfo=timezone.utc).timestamp() if dt.tzinfo is None else dt.timestamp()
    except (ValueError, TypeError, AttributeError):
        return 0


def _columns(con: sqlite3.Connection, table: str) -> set[str]:
    return {r[1] for r in con.execute(f'PRAGMA table_info({table})')}


def _rows(con: sqlite3.Connection, v2: bool) -> list[sqlite3.Row]:
    table = 'orchestration_v2_projection_threads' if v2 else 'projection_threads'
    cols = _columns(con, table)
    projects = _columns(con, 'projection_projects')
    workspace = "COALESCE(j.workspace_root, '')" if 'workspace_root' in projects else "''"
    project_join = 'LEFT JOIN projection_projects j ON j.project_id=t.project_id' if projects else ''
    project_filter = 'AND j.deleted_at IS NULL' if 'deleted_at' in projects else ''
    archive_filter = 'AND t.archived_at IS NULL' if 'archived_at' in cols else ''
    if v2:
        return con.execute(f'''
            SELECT t.thread_id, t.title, t.created_at, t.updated_at,
                   t.payload_json AS thread_payload, p.payload_json AS provider_payload,
                   p.driver AS provider, {workspace} AS workspace
            FROM {table} t JOIN orchestration_v2_projection_provider_threads p
              ON p.provider_thread_id=t.active_provider_thread_id
            {project_join}
            WHERE t.deleted_at IS NULL {archive_filter} {project_filter}
            ORDER BY t.updated_at DESC, t.thread_id
        ''').fetchall()
    worktree = "COALESCE(t.worktree_path, '')" if 'worktree_path' in cols else "''"
    return con.execute(f'''
        SELECT t.thread_id, t.title, t.created_at, t.updated_at,
               r.provider_name AS provider, r.resume_cursor_json AS cursor,
               {worktree} AS worktree, {workspace} AS workspace
        FROM projection_threads t JOIN provider_session_runtime r ON r.thread_id=t.thread_id
        {project_join}
        WHERE t.deleted_at IS NULL {archive_filter} {project_filter}
        ORDER BY t.updated_at DESC, t.thread_id
    ''').fetchall()


def _prompts(con: sqlite3.Connection, thread_id: str, v2: bool) -> tuple[str, str]:
    table = 'orchestration_v2_projection_messages' if v2 else 'projection_thread_messages'
    if not _columns(con, table):
        return '', ''
    field = 'payload_json' if v2 else 'text'
    prompts = []
    for order in ('ASC', 'DESC'):
        row = con.execute(f'''SELECT {field} FROM {table}
            WHERE thread_id=? AND role='user' ORDER BY created_at {order}, message_id {order} LIMIT 1
        ''', (thread_id,)).fetchone()
        text = (_object(row[0]).get('text', '') if v2 else row[0]) if row else ''
        prompts.append(clean(text, 200) if isinstance(text, str) else '')
    return prompts[0], prompts[1]


def threads(path: Path | None = None) -> dict[tuple[str, str], dict]:
    """Active T3 metadata keyed by harness/native ID, never by provider instance ID."""
    path = path or db_path()
    if not path.is_file():
        return {}
    try:
        con = _connect(path)
    except sqlite3.Error as exc:
        warn(f'skip {path}: {exc}')
        return {}
    try:
        con.row_factory = sqlite3.Row
        v2 = bool(_columns(con, 'orchestration_v2_projection_threads'))
        out: dict[tuple[str, str], dict] = {}
        for row in _rows(con, v2):
            harness, key = CURSOR_KEYS.get(row['provider'], ('', ''))
            if not harness:
                continue
            if v2:
                provider = _object(row['provider_payload'])
                ref = provider.get('nativeThreadRef')
                if not isinstance(ref, dict) or ref.get('driver') != row['provider']:
                    continue
                session_id = ref.get('nativeId')
                worktree = _object(row['thread_payload']).get('worktreePath')
            else:
                session_id = _object(row['cursor']).get(key)
                worktree = row['worktree']
            if not isinstance(session_id, str) or not session_id.strip():
                continue
            session_id = session_id.strip()
            first, last = _prompts(con, row['thread_id'], v2)
            out.setdefault((harness, session_id), {
                'thread_id': row['thread_id'], 'title': clean(row['title'], 120),
                'cwd': worktree if isinstance(worktree, str) and worktree else row['workspace'],
                'started': row['created_at'], 'last_active': _epoch(row['updated_at']),
                'first_user': first, 'last_user': last, 'path': str(path),
            })
        return out
    except sqlite3.Error as exc:
        warn(f'skip {path}: {exc}')
        return {}
    finally:
        con.close()


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


def overlay(sessions: list[Session], since_hours: float, harness: str = '') -> None:
    found = threads()
    annotate(sessions, found)
    by_id = {(s.harness, s.id): s for s in sessions}
    cutoff = time.time() - since_hours * 3600
    for (name, sid), thread in found.items():
        if (harness and harness != name) or thread['last_active'] < cutoff:
            continue
        existing = by_id.get((name, sid))
        if existing is not None:
            existing.last_active = max(existing.last_active, thread['last_active'])
            for field in ('cwd', 'first_user', 'last_user'):
                if not getattr(existing, field):
                    setattr(existing, field, thread[field])
            continue
        session = Session(name, sid, thread['cwd'], thread['path'], thread['started'],
                          thread['last_active'], title=thread['title'], first_user=thread['first_user'],
                          last_user=thread['last_user'], source=SOURCE)
        sessions.append(session)
        by_id[(name, sid)] = session
