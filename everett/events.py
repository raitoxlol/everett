"""Events: what a session is doing right now (done, blocked, needs-input, info), and who hears about it.

Layout under ~/.everett/:
    events.jsonl            every event, one JSON object per line
    state/<session>.json    each session's latest event, with `since` (when that state began)
    subscriptions.json      {subscriber session: [targets]}; a target is a session id,
                            "project:<name>", or "*"

Routing: subscribed sessions get events in their inbox; blocked/needs-input events also go to
the human through the notifier (a macOS notification by default, or `notify_command`), with a
one-time escalation when a session stays blocked longer than `escalate_minutes`.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time
import uuid
from pathlib import Path

from . import config
from .session import home

KINDS = ('done', 'blocked', 'needs-input', 'info')
HUMAN_KINDS = ('blocked', 'needs-input')   # these reach the human's notifier
DEBOUNCE = 600          # an automatic event repeating the current state within this window is dropped
MAX_TEXT = 300
ESCALATE_MINUTES = 30
ESCALATE_THROTTLE = 60  # hooks check for escalations at most once a minute


class EventError(Exception):
    def __init__(self, message: str, code: int = 2):
        super().__init__(message)
        self.code = code


def events_path() -> Path:
    return home() / '.everett' / 'events.jsonl'


def state_dir() -> Path:
    return home() / '.everett' / 'state'


def subs_path() -> Path:
    return home() / '.everett' / 'subscriptions.json'


def _valid_sid(sid: str) -> bool:
    return bool(sid) and bool(re.fullmatch(r'[A-Za-z0-9._-]{1,128}', sid)) and sid not in ('.', '..')


def _write_json(target: Path, data) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_name(f'.{target.name}.{os.getpid()}.tmp')
    tmp.write_text(json.dumps(data, ensure_ascii=False), encoding='utf-8')
    tmp.replace(target)


def state(session_id: str) -> dict | None:
    if not _valid_sid(session_id):
        return None
    try:
        data = json.loads((state_dir() / f'{session_id}.json').read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


# ---- record ---------------------------------------------------------------------------------

def record(kind: str, text: str, session: str | None = None, project: str | None = None, harness: str = '',
           cwd: str | None = None, source: str = 'manual', now: float | None = None,
           notifier=None) -> dict | None:
    """Store one event, update the session's state, route it. Returns None when debounced."""
    from .core import find_secret, project_for, slug
    from .send import caller_identity
    if kind not in KINDS:
        raise EventError(f'kind must be one of {", ".join(KINDS)}.')
    text = ' '.join((text or '').split())
    if not text:
        raise EventError('The event message is empty.')
    text = text[:MAX_TEXT]
    secret = find_secret(text)
    if secret:
        raise EventError(f'Rejected: this looks like it contains {secret}.')
    ident = caller_identity()
    session = session if session is not None else ident['session_id']
    harness = harness or (ident['harness'] if session == ident['session_id'] else '')
    cwd = cwd if cwd is not None else os.getcwd()
    project = slug(project) if project else project_for(cwd)
    now = time.time() if now is None else now
    prev = state(session) if session else None
    if source == 'auto' and prev and prev.get('kind') == kind and (
            prev.get('text') == text or now - float(prev.get('ts') or 0) < DEBOUNCE):
        return None
    event = {'id': 'e' + uuid.uuid4().hex[:10], 'ts': now, 'kind': kind, 'text': text, 'session': session or '',
             'project': project, 'harness': harness, 'cwd': cwd, 'source': source}
    events_path().parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(events_path(), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, (json.dumps(event, ensure_ascii=False) + '\n').encode('utf-8'))
    finally:
        os.close(fd)
    if session and _valid_sid(session):
        same = bool(prev) and prev.get('kind') == kind
        _write_json(state_dir() / f'{session}.json',
                    {**event, 'since': prev.get('since', prev['ts']) if same else now,
                     'escalated': bool(prev.get('escalated')) if same else False})
    route(event)
    if kind in HUMAN_KINDS:
        notify(event, runner=notifier)
    return event


# ---- subscriptions --------------------------------------------------------------------------

def subscriptions() -> dict[str, list[str]]:
    try:
        data = json.loads(subs_path().read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return {}
    if not isinstance(data, dict):
        return {}
    return {k: [t for t in v if isinstance(t, str)] for k, v in data.items() if isinstance(v, list)}


def subscribe(subscriber: str, target: str, remove: bool = False) -> list[str]:
    if not _valid_sid(subscriber):
        raise EventError('No subscriber session: run this inside a session or pass --as <session id>.')
    target = target.strip()
    if not target:
        raise EventError('Name a session id, "project:<name>", or "*".')
    subs = subscriptions()
    current = subs.get(subscriber, [])
    if remove:
        current = [t for t in current if t != target]
    elif target not in current:
        current.append(target)
    if current:
        subs[subscriber] = current
    else:
        subs.pop(subscriber, None)
    _write_json(subs_path(), subs)
    return current


def resolve_target(target: str) -> str:
    """CLI/MCP target -> stored form: a session id, "project:<slug>", or "*"."""
    from .core import slug
    target = target.strip()
    if target == '*' or target.startswith('project:'):
        return target if target == '*' else 'project:' + slug(target.split(':', 1)[1])
    from . import registry
    try:
        return registry.find(target, registry.scan(72 * 7, include_auto=True, limit=None)).id
    except registry.SessionLookupError:
        return 'project:' + slug(target)


def matches(target: str, event: dict) -> bool:
    if target == '*':
        return True
    if target.startswith('project:'):
        return bool(event.get('project')) and target[8:] == event['project']
    sid = event.get('session') or ''
    return bool(sid) and (sid == target or (len(target) >= 6 and sid.startswith(target)))


def render(event: dict) -> str:
    who = f'{event.get("harness") or "session"} {event["session"][:12]}' if event.get('session') else 'the human'
    where = f' in {event["project"]}' if event.get('project') else ''
    return f'{event["kind"].upper()} from {who}{where}: {event["text"]}'


def route(event: dict) -> list[str]:
    """Post the event to every subscribed session's inbox (never to the session it came from)."""
    from . import inbox
    delivered = []
    for subscriber, targets in subscriptions().items():
        if subscriber == event.get('session') or not any(matches(t, event) for t in targets):
            continue
        try:
            inbox.post(subscriber, render(event), sender=event.get('session') or inbox.HUMAN, kind='event',
                       from_harness=event.get('harness', ''), extra={'event': event['id']})
            delivered.append(subscriber)
        except inbox.InboxError:
            continue
    return delivered


# ---- the human's notifier -------------------------------------------------------------------

def _applescript(text: str) -> str:
    return '"' + text.replace('\\', '\\\\').replace('"', '\\"') + '"'


def notify_line(event: dict, reason: str = '') -> str:
    where = event.get('project') or (event.get('session') or '')[:8] or 'everett'
    return f'{reason + ": " if reason else ""}{event["kind"]} [{where}] {event["text"]}'


def notify(event: dict, reason: str = '', runner=None) -> list[list[str]]:
    """Tell the human. Mode: config `notify` (osascript | command | both | none; env EVERETT_NOTIFY).

    `notify_command` (env EVERETT_NOTIFY_COMMAND) runs under `sh -c` with the one-line summary as $1
    and EVERETT_EVENT_* in its environment. Commands run detached so hooks stay fast; `runner`
    replaces the launcher (tests)."""
    command = config.get('notify_command', env='EVERETT_NOTIFY_COMMAND')
    mode = config.get('notify', env='EVERETT_NOTIFY', default='both' if command else 'osascript')
    line = notify_line(event, reason)
    runs = []
    if mode in ('command', 'both') and command:
        env = {**os.environ, 'EVERETT_EVENT_KIND': event['kind'], 'EVERETT_EVENT_TEXT': event['text'],
               'EVERETT_EVENT_SESSION': event.get('session', ''), 'EVERETT_EVENT_PROJECT': event.get('project', ''),
               'EVERETT_EVENT_REASON': reason, 'EVERETT_EVENT_JSON': json.dumps(event, ensure_ascii=False)}
        runs.append((['/bin/sh', '-c', command, 'everett', line], env))
    if mode in ('osascript', 'both') and sys.platform == 'darwin':
        title = 'Everett: ' + ('still ' if reason else '') + event['kind']
        script = f'display notification {_applescript(event["text"][:200])} with title {_applescript(title)}'
        sub = event.get('project') or ''
        if sub:
            script += f' subtitle {_applescript(sub)}'
        runs.append((['osascript', '-e', script], None))
    for argv, env in runs:
        try:
            if runner is not None:
                runner(argv, env)
            else:
                subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, start_new_session=True)
        except OSError:
            continue
    return [argv for argv, _ in runs]


def escalate_minutes() -> float:
    try:
        return float(config.get('escalate_minutes', env='EVERETT_ESCALATE_MINUTES', default=str(ESCALATE_MINUTES)))
    except ValueError:
        return ESCALATE_MINUTES


def check_escalations(now: float | None = None, runner=None) -> list[dict]:
    """Notify once for every session blocked / waiting on input longer than escalate_minutes."""
    now = time.time() if now is None else now
    limit = escalate_minutes() * 60
    out = []
    if limit <= 0 or not state_dir().is_dir():
        return out
    for file in state_dir().glob('*.json'):
        try:
            data = json.loads(file.read_text(encoding='utf-8'))
        except (OSError, ValueError):
            continue
        if (not isinstance(data, dict) or data.get('kind') not in HUMAN_KINDS or data.get('escalated')
                or now - float(data.get('since') or data.get('ts') or now) < limit):
            continue
        minutes = int((now - float(data.get('since') or data['ts'])) // 60)
        notify(data, reason=f'{data["kind"]} for {minutes} min', runner=runner)
        data['escalated'] = True
        _write_json(file, data)
        out.append(data)
    return out


def maybe_escalate(now: float | None = None) -> None:
    """Throttled escalation check for hooks: at most once per ESCALATE_THROTTLE seconds."""
    now = time.time() if now is None else now
    stamp = state_dir() / '.escalate-check'
    try:
        if now - stamp.stat().st_mtime < ESCALATE_THROTTLE:
            return
    except FileNotFoundError:
        if not state_dir().is_dir():
            return  # no states, nothing to escalate
    except OSError:
        return
    try:
        stamp.touch()
        check_escalations(now)
    except Exception:  # noqa: BLE001  never break the calling hook
        pass


# ---- reading --------------------------------------------------------------------------------

def read(since: float = 0) -> list[dict]:
    out = []
    try:
        with events_path().open(encoding='utf-8') as f:
            for line in f:
                try:
                    item = json.loads(line)
                except ValueError:
                    continue
                if isinstance(item, dict) and float(item.get('ts') or 0) >= since and item.get('kind'):
                    out.append(item)
    except FileNotFoundError:
        pass
    return out


def parse_since(value: str) -> float:
    """'24h', '30m', '2d', or a number of hours -> seconds."""
    match = re.fullmatch(r'\s*(\d+(?:\.\d+)?)\s*([smhd]?)\s*', value or '')
    if not match:
        raise EventError('--since takes a duration like 30m, 24h, or 7d.')
    return float(match.group(1)) * {'s': 1, 'm': 60, 'h': 3600, 'd': 86400, '': 3600}[match.group(2)]


def age(seconds: float) -> str:
    s = max(0, int(seconds))
    return f'{s // 60}m' if s < 3600 else f'{s // 3600}h' if s < 172800 else f'{s // 86400}d'


def describe(data: dict, now: float | None = None) -> str:
    """'blocked 32h: waiting on Max prompt'."""
    now = time.time() if now is None else now
    return f'{data["kind"]} {age(now - float(data.get("since") or data.get("ts") or now))}: {data.get("text", "")}'


def apply(sessions, now: float | None = None) -> None:
    """Attach each session's latest event state (for ls / everett_ls)."""
    now = time.time() if now is None else now
    for s in sessions:
        data = state(s.id)
        if data and data.get('kind'):
            s.state = describe(data, now)
            s.state_kind = data['kind']


# ---- automatic detection (Stop hooks) ---------------------------------------------------------

_BLOCKED = re.compile(r"\b(?:blocked|stuck|waiting (?:on|for)|can(?:no|')t (?:proceed|continue)|"
                      r"unable to (?:proceed|continue))\b", re.I)
_NEEDS = re.compile(r"\b(?:need you to|needs? your (?:input|approval|decision|confirmation|go-ahead|answer)|"
                    r"please (?:confirm|approve|choose|decide|let me know|advise)|let me know (?:if|whether|which|how|what)|"
                    r"should i|do you want|would you like|want me to|shall i)\b", re.I)
_NEGATED = re.compile(r"(?:\bnot|\bno longer|\bnever|\bun|\bwithout|n't)\s*$", re.I)


def _tail(text: str, lines: int = 4, chars: int = 700) -> list[str]:
    """The last few prose lines of a reply (code blocks, quotes, and tables removed)."""
    kept, fence = [], False
    for raw in (text or '').splitlines():
        if re.match(r'^\s*(```|~~~)', raw):
            fence = not fence
            continue
        line = raw.strip()
        if fence or not line or line.startswith(('>', '|')):
            continue
        kept.append(re.sub(r'[*_`]+', '', line).strip())
    tail, total = [], 0
    for line in reversed(kept):
        if len(tail) >= lines or total + len(line) > chars and tail:
            break
        tail.insert(0, line)
        total += len(line)
    return tail


def _sentence(line: str, start: int) -> str:
    left = max(line.rfind('. ', 0, start), line.rfind('! ', 0, start), line.rfind('? ', 0, start))
    right = re.search(r'[.!?](?=\s|$)', line[start:])
    return line[left + 2 if left >= 0 else 0: start + right.end() if right else len(line)].strip()


def _hit(pattern: re.Pattern, lines: list[str], statements_only: bool = False) -> str:
    for line in reversed(lines):
        for m in pattern.finditer(line):
            if _NEGATED.search(line[:m.start()][-14:]):
                continue
            sentence = _sentence(line, m.start())
            if statements_only and sentence.endswith('?'):
                continue  # "is it blocked?" asks; it does not report a block
            return sentence
    return ''


def classify(message: str) -> tuple[str, str] | None:
    """Deterministic: (kind, summary) for the last assistant message of a turn, or None if empty.

    - blocked: the closing lines state blocked / stuck / waiting on|for / can't proceed (not negated
      by not / no longer / un-, and not inside a question)
    - needs-input: a closing line is a question (ends with "?"), or asks the user to act
      ("need you to", "please confirm", "should I", "do you want", "let me know if" ...)
    - done: anything else (a clean finish)
    """
    lines = _tail(message)
    if not lines:
        return None
    hit = _hit(_BLOCKED, lines, statements_only=True)
    if hit:
        return 'blocked', hit[:MAX_TEXT]
    question = next((l for l in reversed(lines) if l.rstrip(' )').endswith('?')), '')
    if question:
        start = question.rstrip().rfind('?')
        return 'needs-input', _sentence(question, max(0, start - 1))[:MAX_TEXT] or question[:MAX_TEXT]
    hit = _hit(_NEEDS, lines)
    if hit:
        return 'needs-input', hit[:MAX_TEXT]
    return 'done', lines[-1][:160]
