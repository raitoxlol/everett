from __future__ import annotations

from pathlib import Path

from ..session import Session, clean, home, is_injected, read_edges, recent_files, warn


def _user_text(d: dict) -> str:
    if d.get('type') != 'message':
        return ''
    m = d.get('message') or {}
    if m.get('role') != 'user':
        return ''
    content = m.get('content')
    if isinstance(content, str):
        text = content
    else:
        text = ' '.join(c.get('text', '') for c in content or [] if isinstance(c, dict) and c.get('type') == 'text')
    return '' if is_injected(text) else clean(text)


def parse(path: Path, harness: str = 'omp') -> Session | None:
    """Parse an OMP or Pi session file (Pi's JSONL format, which OMP extends)."""
    head, tail = read_edges(path)
    sess = next((d for d in head if d.get('type') == 'session'), None)
    if sess is None:
        return None
    title = next((d.get('title', '') for d in head if d.get('type') == 'title'), '') or sess.get('title', '')
    users = [t for t in map(_user_text, head) if t]
    last = [t for t in map(_user_text, tail) if t] or users
    return Session(harness, sess.get('id', ''), sess.get('cwd', ''), str(path), sess.get('timestamp', ''),
                   path.stat().st_mtime, title=clean(title, 120),
                   first_user=users[0] if users else '', last_user=last[-1] if last else '')


def scan_root(root: Path, harness: str, since_hours: float) -> list[Session]:
    out = []
    for p in recent_files(root.glob('*/*.jsonl'), since_hours):
        try:
            s = parse(p, harness)
        except Exception as exc:  # noqa: BLE001
            warn(f'skip {p}: {exc}')
            continue
        if s:
            out.append(s)
    return out


def scan(since_hours: float = 72) -> list[Session]:
    return scan_root(home() / '.omp' / 'agent' / 'sessions', 'omp', since_hours)
