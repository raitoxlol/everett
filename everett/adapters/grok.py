"""Grok CLI (Grok Build) sessions: ~/.grok/sessions/<url-encoded cwd>/<session-id>/.

Each session is a directory. Everett reads, never writes:
- `summary.json`: `info.id`, `info.cwd`, `created_at`, `last_active_at`, `generated_title`
  (or a manual `title`), and `session_kind` (`headless` for `grok -p` runs).
- `chat_history.jsonl`: one message per line. Typed prompts are `{"type": "user", "prompt_index": N,
  "content": [{"type": "text", "text": "<user_query>...</user_query>"}]}`; injected context carries
  `synthetic_reason` or has no `<user_query>` wrapper. Replies are `{"type": "assistant", "content": "..."}`.
- `<url-encoded cwd>/prompt_history.jsonl`: `{"timestamp", "session_id", "prompt", "is_bash"}` per typed
  prompt. This is the main source for requests: injected reminders in chat_history can push the first
  prompt far past the head of that file.
- `~/.grok/active_sessions.json`: `[{"session_id", "pid", "cwd", "opened_at"}]` for open sessions.
"""
from __future__ import annotations

import json
import os
import re
import time
from pathlib import Path
from urllib.parse import quote, unquote

from ..cards import card_path
from ..session import Session, clean, home, is_injected, read_edges, warn

ACTIVITY_FILES = ('updates.jsonl', 'chat_history.jsonl', 'summary.json')
QUERY = re.compile(r'<user_query>\s*(.*?)\s*(?:</user_query>|$)', re.S)


def sessions_root() -> Path:
    return home() / '.grok' / 'sessions'


def activity(session_dir: Path) -> float:
    """Newest write in a session directory (appends don't touch the directory's own mtime)."""
    times = []
    for name in ACTIVITY_FILES:
        try:
            times.append((session_dir / name).stat().st_mtime)
        except OSError:
            continue
    if not times:
        times.append(session_dir.stat().st_mtime)
    return max(times)


def _content_text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return ' '.join(c.get('text', '') for c in content if isinstance(c, dict) and c.get('type') == 'text')
    return ''


def user_text(d: dict, *, raw: bool = False) -> str:
    if d.get('type') != 'user' or d.get('synthetic_reason'):
        return ''
    text = _content_text(d.get('content'))
    match = QUERY.search(text)
    if match:
        text = match.group(1)
    elif is_injected(text):  # <user_info>, <system-reminder>, ...
        return ''
    if not text.strip():
        return ''
    return text if raw else clean(text)


def assistant_text(d: dict, *, raw: bool = False) -> str:
    if d.get('type') != 'assistant':
        return ''
    text = _content_text(d.get('content'))
    return text if raw else clean(text)


def active_sessions() -> set[str]:
    """Ids of sessions a live Grok process has open."""
    try:
        rows = json.loads((home() / '.grok' / 'active_sessions.json').read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return set()
    live = set()
    for row in rows if isinstance(rows, list) else []:
        if not isinstance(row, dict) or not row.get('session_id'):
            continue
        try:
            os.kill(int(row.get('pid')), 0)
        except PermissionError:
            pass  # exists, owned by someone else
        except (OSError, TypeError, ValueError):
            continue
        live.add(row['session_id'])
    return live


def prompt_history(cwd_dir: Path) -> dict[str, list[str]]:
    """{session id: typed prompts, oldest first} from a cwd folder's prompt_history.jsonl."""
    out: dict[str, list[str]] = {}
    try:
        with (cwd_dir / 'prompt_history.jsonl').open(encoding='utf-8') as f:
            for line in f:
                try:
                    d = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(d, dict) or d.get('is_bash') or not isinstance(d.get('prompt'), str):
                    continue
                text = clean(d['prompt'])
                if text and d.get('session_id'):
                    out.setdefault(d['session_id'], []).append(text)
    except OSError:
        pass
    return out


def transcript(session_id: str, cwd: str = '') -> Path | None:
    """chat_history.jsonl of a session, looked up by id (the cwd folder first)."""
    root = sessions_root()
    if cwd:
        direct = root / quote(cwd, safe='') / session_id / 'chat_history.jsonl'
        if direct.is_file():
            return direct
    return next(iter(root.glob(f'*/{session_id}/chat_history.jsonl')), None)


def parse(session_dir: Path, live: set[str] = frozenset(),
          prompts: dict[str, list[str]] | None = None) -> Session | None:
    try:
        summary = json.loads((session_dir / 'summary.json').read_text(encoding='utf-8'))
    except (OSError, ValueError):
        summary = {}
    if not isinstance(summary, dict):
        summary = {}
    info = summary.get('info') if isinstance(summary.get('info'), dict) else {}
    sid = info.get('id') or session_dir.name
    cwd = info.get('cwd') or unquote(session_dir.parent.name)
    history = session_dir / 'chat_history.jsonl'
    head, tail = read_edges(history) if history.is_file() else ([], [])
    typed = (prompt_history(session_dir.parent) if prompts is None else prompts).get(sid, [])
    users = typed or [t for t in map(user_text, head) if t]
    last = typed or [t for t in map(user_text, tail) if t] or users
    title = summary.get('title') or summary.get('generated_title') or ''
    if not users and not title and not card_path(sid).exists():
        return None  # nothing typed yet
    return Session('grok', sid, cwd, str(session_dir), summary.get('created_at', ''), activity(session_dir),
                   title=clean(title, 120), first_user=users[0] if users else '',
                   last_user=last[-1] if last else '', running=sid in live,
                   auto=summary.get('session_kind') == 'headless')


def scan(since_hours: float = 72) -> list[Session]:
    root = sessions_root()
    cutoff = time.time() - since_hours * 3600
    live = active_sessions()
    histories: dict[Path, dict[str, list[str]]] = {}
    out = []
    for d in root.glob('*/*'):
        try:
            if not d.is_dir() or activity(d) < cutoff:
                continue
            if d.parent not in histories:
                histories[d.parent] = prompt_history(d.parent)
            s = parse(d, live, histories[d.parent])
        except Exception as exc:  # noqa: BLE001
            warn(f'skip {d}: {exc}')
            continue
        if s:
            out.append(s)
    return out
