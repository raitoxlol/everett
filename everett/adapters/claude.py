from __future__ import annotations

from pathlib import Path

from ..cards import card_path
from ..session import Session, clean, home, is_injected, read_edges, recent_files, warn


def user_text(d: dict, *, raw: bool = False) -> str:
    if d.get('type') != 'user' or d.get('isSidechain'):
        return ''
    content = (d.get('message') or {}).get('content')
    if isinstance(content, str):
        text = content
    elif isinstance(content, list):
        parts = [c.get('text', '') for c in content if isinstance(c, dict) and c.get('type') == 'text']
        text = ' '.join(parts)
    else:
        return ''
    return '' if is_injected(text) else (text if raw else clean(text))


def assistant_text(d: dict, *, raw: bool = False) -> str:
    if d.get('type') != 'assistant' or d.get('isSidechain'):
        return ''
    content = (d.get('message') or {}).get('content')
    if isinstance(content, str):
        text = content
    elif isinstance(content, list):
        parts = [c.get('text', '') for c in content
                 if isinstance(c, dict) and c.get('type') in ('text', 'output_text')]
        text = ' '.join(parts)
    else:
        return ''
    return text if raw else clean(text)


def parse(path: Path) -> Session | None:
    head, tail = read_edges(path)
    rows = head + tail
    cwd = next((d['cwd'] for d in rows if d.get('cwd')), '')
    sid = next((d['sessionId'] for d in rows if d.get('sessionId')), path.stem)
    started = next((d['timestamp'] for d in head if d.get('timestamp')), '')
    users = [t for t in map(user_text, head) if t]
    last = [t for t in map(user_text, tail) if t] or users
    titles = [d['aiTitle'] for d in rows if d.get('type') == 'ai-title' and d.get('aiTitle')]
    if not users and not last and not titles and not card_path(sid).exists():
        return None  # nothing to show; a session that wrote a card is always kept
    auto = any(d.get('entrypoint') == 'sdk-cli' for d in head)
    return Session('claude', sid, cwd, str(path), started, path.stat().st_mtime,
                   first_user=users[0] if users else '', last_user=last[-1] if last else '', auto=auto,
                   title=clean(titles[-1], 120) if titles else '')


def scan(since_hours: float = 72) -> list[Session]:
    root = home() / '.claude' / 'projects'
    out = []
    for p in recent_files(root.glob('*/*.jsonl'), since_hours):
        try:
            s = parse(p)
        except Exception as exc:  # noqa: BLE001
            warn(f'skip {p}: {exc}')
            continue
        if s:
            out.append(s)
    return out
