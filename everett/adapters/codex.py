from __future__ import annotations

import json
from pathlib import Path

from ..session import Session, clean, home, is_injected, read_edges, recent_files, warn


def user_text(d: dict, *, raw: bool = False) -> str:
    if d.get('type') != 'response_item':
        return ''
    p = d.get('payload') or {}
    if p.get('type') != 'message' or p.get('role') != 'user':
        return ''
    text = ' '.join(c.get('text', '') for c in p.get('content') or [] if isinstance(c, dict))
    return '' if is_injected(text) else (text if raw else clean(text))


def assistant_text(d: dict, *, raw: bool = False) -> str:
    if d.get('type') != 'response_item':
        return ''
    p = d.get('payload') or {}
    if p.get('type') != 'message' or p.get('role') != 'assistant':
        return ''
    text = ' '.join(c.get('text', '') for c in p.get('content') or []
                    if isinstance(c, dict) and c.get('type') in ('text', 'output_text'))
    return text if raw else clean(text)


def thread_names() -> dict[str, str]:
    """Codex Desktop thread names from session_index.jsonl (later lines win)."""
    names: dict[str, str] = {}
    try:
        with (home() / '.codex' / 'session_index.jsonl').open() as f:
            for line in f:
                try:
                    d = json.loads(line)
                except ValueError:
                    continue
                if d.get('id') and d.get('thread_name'):
                    names[d['id']] = d['thread_name'].lstrip('✅ ').strip()
    except OSError:
        pass
    return names


def parse(path: Path, names: dict[str, str] | None = None) -> Session | None:
    head, tail = read_edges(path)
    meta = next((d.get('payload') or {} for d in head if d.get('type') == 'session_meta'), None)
    if meta is None:
        return None
    users = [t for t in map(user_text, head) if t]
    last = [t for t in map(user_text, tail) if t] or users
    src = meta.get('source')
    auto = src == 'exec' or (isinstance(src, dict) and 'subagent' in src)  # scripted run or spawned child thread
    sid = meta.get('id') or meta.get('session_id') or path.stem
    return Session('codex', sid,
                   meta.get('cwd', ''), str(path), meta.get('timestamp', ''), path.stat().st_mtime,
                   first_user=users[0] if users else '', last_user=last[-1] if last else '', auto=auto,
                   title=clean((names or {}).get(sid, ''), 120),
                   source='t3code' if str(meta.get('originator', '')).startswith('t3code') else '')


def scan(since_hours: float = 72) -> list[Session]:
    root = home() / '.codex' / 'sessions'
    out = []
    names = thread_names()
    for p in recent_files(root.glob('*/*/*/*.jsonl'), since_hours):
        try:
            s = parse(p, names)
        except Exception as exc:  # noqa: BLE001
            warn(f'skip {p}: {exc}')
            continue
        if s:
            out.append(s)
    return out
