from __future__ import annotations

import json
import os
import re
import sys
from dataclasses import dataclass, asdict
from pathlib import Path

CHUNK = 64 * 1024


@dataclass
class Session:
    harness: str
    id: str
    cwd: str
    path: str
    started: str
    last_active: float
    title: str = ''
    first_user: str = ''
    last_user: str = ''
    running: bool = False
    auto: bool = False
    card: str = ''
    card_source: str | None = None
    profile: str = ''   # Hermes profile name
    source: str = ''    # harness-specific origin (e.g. Hermes cli/telegram)
    state: str = ''     # latest Everett event, e.g. "blocked 32h: waiting on Max prompt"
    state_kind: str = ''  # done | blocked | needs-input | info

    def to_dict(self) -> dict:
        return asdict(self)


def read_edges(path: Path) -> tuple[list[dict], list[dict]]:
    """Parse JSON lines from the first and last CHUNK bytes only."""
    size = path.stat().st_size
    with path.open('rb') as f:
        head = f.read(CHUNK)
        tail = b''
        if size > CHUNK:
            f.seek(max(CHUNK, size - CHUNK))
            tail = f.read()
    head_lines = head.split(b'\n')
    if size > CHUNK:
        head_lines = head_lines[:-1]  # last line may be cut
    tail_lines = tail.split(b'\n')[1:] if tail else []
    return _parse(head_lines), _parse(tail_lines)


def _parse(lines: list[bytes]) -> list[dict]:
    out = []
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        if isinstance(obj, dict):
            out.append(obj)
    return out


def scan_full(path: Path, limit_lines: int = 20000) -> list[dict]:
    """Parse the whole file as JSON lines (bounded), line by line. A fallback for when the
    edge-window read (read_edges) misses real content buried behind an oversized head record --
    e.g. a Codex session whose injected preamble (AGENTS.md, environment_context, recommended
    plugins -- each tens of KB) fills the entire head budget before the real first user message."""
    out: list[dict] = []
    try:
        with path.open('r', encoding='utf-8', errors='ignore') as f:
            for i, line in enumerate(f):
                if i >= limit_lines:
                    break
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except ValueError:
                    continue
                if isinstance(obj, dict):
                    out.append(obj)
    except OSError:
        return []
    return out


def clean(text: str, limit: int = 200) -> str:
    text = re.sub(r'<[^>]{1,80}>', ' ', text or '')  # inline tags like <pasted_content id=…>
    return ' '.join(text.split())[:limit]


USER_WRAPPERS = ('<pasted_content',)  # tags that wrap text the user typed or pasted


def is_injected(text: str) -> bool:
    t = text.lstrip()
    return not t or (t.startswith('<') and not t.startswith(USER_WRAPPERS))


def recent_files(paths, since_hours: float):
    cutoff = __import__('time').time() - since_hours * 3600
    for p in paths:
        try:
            if p.stat().st_mtime >= cutoff:
                yield p
        except OSError:
            continue


def warn(msg: str) -> None:
    print(f'everett: {msg}', file=sys.stderr)


def home() -> Path:
    return Path(os.environ.get('EVERETT_HOME') or Path.home())
