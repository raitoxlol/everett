"""Devin CLI sessions, read-only from the Devin CLI data directory.

The Devin CLI keeps a `sessions.db` SQLite store plus per-session ATIF transcripts
(`transcripts/<id>.json`, export-only snapshots). The data directory is `$DEVIN_HOME`
when set, else the platform default:

- Linux:   ~/.local/share/devin/cli
- macOS:   ~/Library/Application Support/devin/cli
- Windows: %APPDATA%/devin/cli

The database is opened with SQLite's read-only URI (mode=ro); Everett never writes to it.
"""
from __future__ import annotations

import json
import os
import sqlite3
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

from ..session import Session, clean, home, is_injected, warn

DEFAULT_REL = (
    'Library/Application Support/devin/cli' if sys.platform == 'darwin'
    else 'AppData/Roaming/devin/cli' if sys.platform == 'win32'
    else '.local/share/devin/cli')


def data_dir() -> Path:
    """Primary data dir: $DEVIN_HOME when set, else the platform default."""
    override = os.environ.get('DEVIN_HOME')
    if override:
        return Path(override).expanduser()
    return home() / DEFAULT_REL


def data_dirs() -> list[Path]:
    """Existing candidate dirs: $DEVIN_HOME, the platform default, and the XDG path
    (CLI builds have used it on every platform, so both are always tried)."""
    out: list[Path] = []
    for cand in (os.environ.get('DEVIN_HOME'), str(home() / DEFAULT_REL),
                 str(home() / '.local/share/devin/cli')):
        if not cand:
            continue
        p = Path(cand).expanduser()
        if p not in out:
            out.append(p)
    return out


def _connect(path: Path) -> sqlite3.Connection:
    con = sqlite3.connect(f'{path.resolve().as_uri()}?mode=ro', uri=True, timeout=1)
    con.row_factory = sqlite3.Row
    return con


def _columns(con: sqlite3.Connection, table: str) -> set[str]:
    return {row[1] for row in con.execute(f'PRAGMA table_info({table})')}


def _tables(con: sqlite3.Connection) -> set[str]:
    return {row[0] for row in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}


def _epoch(value) -> float:
    """sessions.db columns and node metadata hold epoch seconds/milliseconds or ISO 8601."""
    if isinstance(value, (int, float)):
        return float(value) / 1000 if value > 1e12 else float(value)
    if isinstance(value, str):
        try:
            return datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()
        except ValueError:
            try:
                v = float(value)
            except ValueError:
                return 0.0
            return v / 1000 if v > 1e12 else v
    return 0.0


def _iso(ts: float) -> str:
    return datetime.fromtimestamp(ts, timezone.utc).isoformat() if ts else ''


def _parts_text(content) -> str:
    """ATIF message content: a plain string or a list of ContentPart objects."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return ' '.join(str(p['text']) for p in content
                        if isinstance(p, dict) and p.get('text'))
    return ''


def _json(raw):
    if isinstance(raw, (str, bytes)):
        try:
            return json.loads(raw)
        except ValueError:
            return None
    return raw if isinstance(raw, dict) else None


def _layers(obj) -> list[dict]:
    """A node/step plus its nested chat_message/message payloads, if any."""
    out = [obj] if isinstance(obj, dict) else []
    for key in ('chat_message', 'message'):
        nested = _json(obj.get(key)) if isinstance(obj, dict) else None
        if isinstance(nested, dict):
            out.append(nested)
    return out


def _is_user(obj: dict) -> bool:
    return any(layer.get('source') == 'user' or layer.get('role') == 'user' or
               (layer.get('metadata') or {}).get('is_user_input') for layer in _layers(obj))


def _node_text(obj) -> str:
    for layer in _layers(obj):
        for key in ('content', 'text', 'message'):
            text = _parts_text(layer.get(key))
            if text.strip():
                return text
    return ''


def _users(con: sqlite3.Connection, cols: set[str], session_id: str) -> tuple[str, str]:
    """First and last user texts in message_nodes; the schema shifts between CLI builds,
    so columns are probed. User nodes carry metadata.is_user_input or a user role/source."""
    sid = next((c for c in ('session_id', 'session') if c in cols), None)
    body = next((c for c in ('chat_message', 'message', 'content', 'data') if c in cols), None)
    order = next((c for c in ('node_id', 'created_at', 'id') if c in cols), None)
    if not sid or not body or not order:
        return '', ''

    def nearest(desc: bool) -> str:
        rows = con.execute(
            f'SELECT {body} AS body FROM message_nodes WHERE {sid} = ? '
            f'ORDER BY {order} {"DESC" if desc else "ASC"} LIMIT 1000', (session_id,))
        for r in rows:
            obj = _json(r['body'])
            if obj and _is_user(obj):
                text = _node_text(obj)
                if text and not is_injected(text):
                    return clean(text)
        return ''

    return nearest(False), nearest(True)


def read_db(path: Path, since_hours: float) -> tuple[list[Session], bool]:
    """Sessions from sessions.db; the bool says whether message_nodes was readable
    (when it is, transcripts stay unread so the two never double count)."""
    con = _connect(path)
    try:
        cols = _columns(con, 'sessions')
        if 'id' not in cols:
            return [], False
        have_nodes = 'message_nodes' in _tables(con)
        node_cols = _columns(con, 'message_nodes') if have_nodes else set()
        wanted = [c for c in ('id', 'working_directory', 'cwd', 'title', 'model',
                              'created_at', 'last_activity_at', 'updated_at') if c in cols]
        where = ' AND '.join(f'COALESCE({flag}, 0) = 0'
                             for flag in ('hidden', 'archived') if flag in cols) or '1=1'
        activity = [c for c in ('last_activity_at', 'updated_at', 'created_at') if c in cols]
        order = f' ORDER BY COALESCE({", ".join(activity)}) DESC' if activity else ''
        rows = con.execute(
            f'SELECT {", ".join(wanted)} FROM sessions WHERE {where}{order} LIMIT 400').fetchall()
        keys = rows[0].keys() if rows else []
        cutoff = time.time() - since_hours * 3600
        out: list[Session] = []
        for row in rows:
            last_ts = max((_epoch(row[c]) for c in ('last_activity_at', 'updated_at', 'created_at')
                           if c in keys), default=0.0)
            if last_ts and last_ts < cutoff:
                continue
            first, last = _users(con, node_cols, row['id']) if have_nodes else ('', '')
            transcript = path.parent / 'transcripts' / f'{row["id"]}.json'
            cwd = ''
            for key in ('working_directory', 'cwd'):
                if key in keys and row[key]:
                    cwd = str(row[key])
                    break
            out.append(Session(
                'devin', str(row['id']), cwd,
                str(transcript) if transcript.is_file() else str(path),
                _iso(_epoch(row['created_at']) if 'created_at' in keys else 0.0), last_ts,
                title=clean(row['title'], 120) if 'title' in keys and row['title'] else '',
                first_user=first, last_user=last, source='cli'))
        return out, have_nodes
    finally:
        con.close()


def read_transcript(path: Path) -> Session | None:
    """One ATIF transcript file -> Session, for when sessions.db is missing or too old
    to carry message_nodes."""
    try:
        obj = json.loads(path.read_text(encoding='utf-8', errors='ignore'))
    except (OSError, ValueError):
        return None
    if not isinstance(obj, dict) or not isinstance(obj.get('steps'), list):
        return None
    users: list[str] = []
    stamps = []
    for step in obj['steps']:
        if not isinstance(step, dict):
            continue
        meta = step.get('metadata') or {}
        ts = _epoch(meta.get('created_at')) or _epoch(step.get('created_at'))
        if ts:
            stamps.append(ts)
        if _is_user(step):
            text = _node_text(step)
            if text and not is_injected(text):
                users.append(clean(text))
    agent = obj.get('agent') or {}
    extra = agent.get('extra') or {}
    cwd = ''
    for key in ('working_directory', 'cwd', 'workdir'):
        if isinstance(extra.get(key), str) and extra[key]:
            cwd = extra[key]
            break
    last_ts = max(stamps, default=0.0)
    return Session(
        'devin', str(obj.get('session_id') or path.stem), cwd, str(path),
        _iso(min(stamps) if stamps else 0.0), last_ts,
        title=clean(obj.get('title') or '', 120),
        first_user=users[0] if users else '', last_user=users[-1] if users else '',
        source='cli')


def scan(since_hours: float = 72) -> list[Session]:
    cutoff = time.time() - since_hours * 3600
    by_id: dict[str, Session] = {}
    for base in data_dirs():
        sessions: list[Session] = []
        have_nodes = False
        db = base / 'sessions.db'
        if db.is_file():
            try:
                sessions, have_nodes = read_db(db, since_hours)
            except (sqlite3.Error, OSError, ValueError) as exc:
                warn(f'skip {db}: {exc}')
        for s in sessions:
            by_id.setdefault(s.id, s)
        if not have_nodes:  # transcripts carry the listing the db cannot
            for path in sorted((base / 'transcripts').glob('*.json')):
                s = read_transcript(path)
                if not s:
                    continue
                known = by_id.get(s.id)
                if known:
                    known.first_user = known.first_user or s.first_user
                    known.last_user = known.last_user or s.last_user
                    known.cwd = known.cwd or s.cwd
                    known.title = known.title or s.title
                    known.started = known.started or s.started
                    known.last_active = known.last_active or s.last_active
                    known.path = s.path
                elif not s.last_active or s.last_active >= cutoff:
                    by_id[s.id] = s
    return sorted(by_id.values(), key=lambda s: s.last_active, reverse=True)
