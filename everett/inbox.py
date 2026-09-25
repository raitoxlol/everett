"""Per-session inboxes: messages for sessions that are running right now.

Layout under ~/.everett/inbox/:
    <session-id>.jsonl   messages, one JSON object per line (append-only)
    <session-id>.done    ids of messages already delivered, one per line (append-only)
    live/<session-id>.json   the harness process a session's hooks last ran under

A message: {id, from, from_harness, from_card, to, text, ts, reply_to, hops, kind}.
kind is "message", "reply", or "event". Hooks (UserPromptSubmit, PostToolUse, the OMP
extension) inject pending messages into the live session and mark them done.

This module is imported by hooks that run on every tool call, so it stays standard-library
only and imports nothing heavy at module level.
"""
from __future__ import annotations

import json
import os
import re
import time
import uuid
from pathlib import Path

from .session import home

MAX_BATCH = 5            # messages injected per hook run
MAX_TEXT = 2000          # characters kept per message in the injection
MAX_INJECT = 6000        # characters per injection in total
TTL = 7 * 24 * 3600      # undelivered messages older than this are dropped
MAX_HOPS = 3
HUMAN = 'human'          # inbox for senders outside any session (a terminal)
_SID = re.compile(r'[A-Za-z0-9._-]{1,128}')


class InboxError(Exception):
    def __init__(self, message: str, code: int = 2):
        super().__init__(message)
        self.code = code


def inbox_dir() -> Path:
    return home() / '.everett' / 'inbox'


def valid_id(session_id: str) -> bool:
    return bool(session_id) and bool(_SID.fullmatch(session_id)) and session_id not in ('.', '..')


def path(session_id: str) -> Path:
    if not valid_id(session_id):
        raise InboxError(f'not a valid session id: {session_id!r}')
    return inbox_dir() / f'{session_id}.jsonl'


def _done_path(session_id: str) -> Path:
    return inbox_dir() / f'{session_id}.done'


def _append(target: Path, line: str) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, line.encode('utf-8'))  # one write() of one line: O_APPEND keeps it whole
    finally:
        os.close(fd)


def new_id() -> str:
    return 'm' + uuid.uuid4().hex[:10]


def post(to: str, text: str, sender: str = '', kind: str = 'message', reply_to: str = '',
         hops: int = 1, from_harness: str = '', from_card: str = '', extra: dict | None = None) -> dict:
    """Append one message to a session's inbox and return it."""
    text = (text or '').strip()
    if not text:
        raise InboxError('The message is empty.')
    message = {'id': new_id(), 'from': sender or HUMAN, 'from_harness': from_harness,
               'from_card': (from_card or '')[:160], 'to': to, 'text': text, 'ts': time.time(),
               'reply_to': reply_to, 'hops': int(hops), 'kind': kind, **(extra or {})}
    _append(path(to), json.dumps(message, ensure_ascii=False) + '\n')
    return message


def read(session_id: str) -> list[dict]:
    try:
        raw = path(session_id).read_text(encoding='utf-8')
    except (FileNotFoundError, InboxError):
        return []
    out = []
    for line in raw.splitlines():
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if isinstance(item, dict) and item.get('id') and item.get('text'):
            out.append(item)
    return out


def done_ids(session_id: str) -> set[str]:
    try:
        return set(_done_path(session_id).read_text(encoding='utf-8').split())
    except OSError:
        return set()


def pending(session_id: str, now: float | None = None) -> list[dict]:
    """Undelivered, unexpired messages, oldest first."""
    now = time.time() if now is None else now
    done = done_ids(session_id)
    return [m for m in read(session_id) if m['id'] not in done and now - float(m.get('ts') or 0) < TTL]


def mark_done(session_id: str, ids) -> None:
    ids = [i for i in ids if i]
    if ids:
        _append(_done_path(session_id), ''.join(f'{i}\n' for i in ids))


def find(message_id: str) -> dict | None:
    """Look a message up by id across every inbox (for replies)."""
    folder = inbox_dir()
    if not folder.is_dir() or not message_id:
        return None
    for file in folder.glob('*.jsonl'):
        try:
            text = file.read_text(encoding='utf-8')
        except OSError:
            continue
        if message_id not in text:
            continue
        for line in text.splitlines():
            try:
                item = json.loads(line)
            except ValueError:
                continue
            if isinstance(item, dict) and item.get('id') == message_id:
                return item
    return None


def reply_hops(original: dict, env_hops: int = 0) -> int:
    hops = max(int(original.get('hops') or 0), env_hops) + 1
    if hops > MAX_HOPS:
        raise InboxError(f'Hop limit reached ({hops - 1}/{MAX_HOPS}): this thread already went back and forth '
                         f'{hops - 1} times. Answer here instead of replying again.', 7)
    return hops


def wait_reply(sender: str, message_id: str, wait: float, poll: float = 1.0,
               clock=time.time, sleep=time.sleep) -> dict | None:
    """Poll the sender's inbox for a reply to message_id; consume and return it, or None on timeout."""
    deadline = clock() + max(0.0, wait)
    while True:
        for item in pending(sender):
            if item.get('reply_to') == message_id:
                mark_done(sender, [item['id']])
                return item
        if clock() >= deadline:
            return None
        sleep(min(poll, max(0.0, deadline - clock())) or poll)


# ---- rendering for hooks -------------------------------------------------------------------

def _ago(ts: float, now: float) -> str:
    d = max(0, int(now - ts))
    return f'{d}s ago' if d < 60 else f'{d // 60}m ago' if d < 3600 else f'{d // 3600}h ago'


def render(messages: list[dict], now: float | None = None) -> tuple[str, list[str]]:
    """The injection text for a batch, and the ids it actually contains (bounded)."""
    now = time.time() if now is None else now
    parts, ids, total = [], [], 0
    for m in messages[:MAX_BATCH]:
        text = m['text'] if len(m['text']) <= MAX_TEXT else m['text'][:MAX_TEXT] + ' […clipped]'
        sender = m.get('from') or HUMAN
        who = 'the human (terminal)' if sender == HUMAN else f'{m.get("from_harness") or "agent"} session {sender[:12]}'
        card = f' — card: {m["from_card"]}' if m.get('from_card') else ''
        kind = m.get('kind') or 'message'
        head = {'reply': f'REPLY to your message {m.get("reply_to")}', 'event': 'EVENT'}.get(kind, 'MESSAGE')
        how = '' if kind in ('reply', 'event') else (
            f'\nAnswer with everett_send(reply_to="{m["id"]}", text=...) or `everett reply {m["id"]} "<text>"`.')
        block = (f'--- {head} {m["id"]} from {who}{card} ({_ago(float(m.get("ts") or now), now)}, '
                 f'hop {m.get("hops", 1)}/{MAX_HOPS})\n{text}{how}')
        if parts and total + len(block) > MAX_INJECT:
            break
        parts.append(block)
        ids.append(m['id'])
        total += len(block)
    if not parts:
        return '', []
    more = len(messages) - len(ids)
    header = (f'[Everett] {len(ids)} message(s) from other sessions on this machine, delivered by Everett '
              '(not typed by the user). Handle them alongside your current task; the user can see this.')
    tail = f'\n({more} more waiting; they arrive at your next turn or tool call.)' if more > 0 else ''
    return header + '\n' + '\n'.join(parts) + tail, ids


def take(session_id: str) -> str:
    """Render pending messages for injection and mark exactly those delivered."""
    text, ids = render(pending(session_id))
    if ids:
        mark_done(session_id, ids)
    return text


# ---- liveness: which sessions have a harness process attached ---------------------------------

def live_path(session_id: str) -> Path:
    return inbox_dir() / 'live' / f'{session_id}.json'


def pid_alive(pid: int) -> bool:
    if pid <= 1:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


SHELLS = {'sh', 'bash', 'zsh', 'dash', 'fish', '-sh', '-bash', '-zsh'}


def harness_pid() -> int:
    """The harness process that ran this hook: our parent, or its parent when a shell sits between."""
    pid = os.getppid()
    try:
        import subprocess
        out = subprocess.run(['ps', '-o', 'ppid=,comm=', '-p', str(pid)], capture_output=True, text=True,
                             timeout=1).stdout.strip()
        ppid, _, comm = out.partition(' ')
        if os.path.basename(comm.strip()) in SHELLS and ppid.strip().isdigit():
            return int(ppid)
    except Exception:  # noqa: BLE001
        pass
    return pid


def touch_live(session_id: str, harness: str, state: str = 'turn') -> None:
    """Record that this session's harness process is alive (called from hooks; cheap after the first)."""
    target = live_path(session_id)
    try:
        current = json.loads(target.read_text(encoding='utf-8'))
    except (OSError, ValueError):
        current = {}
    pid = int(current.get('pid') or 0) if isinstance(current, dict) else 0
    now = time.time()
    if pid and pid_alive(pid) and current.get('state') == state and now - float(current.get('ts') or 0) < 30:
        return
    if not pid or not pid_alive(pid):
        pid = harness_pid()
        _release_pid(pid, session_id)
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_name(f'.{target.name}.{os.getpid()}.tmp')
    tmp.write_text(json.dumps({'pid': pid, 'harness': harness, 'state': state, 'ts': now}), encoding='utf-8')
    tmp.replace(target)


def _release_pid(pid: int, keep: str) -> None:
    """A harness process holds one session at a time: after /clear or /resume, the old session's
    live record for the same process is stale, so drop it."""
    folder = inbox_dir() / 'live'
    if not folder.is_dir():
        return
    for file in folder.glob('*.json'):
        if file.stem == keep:
            continue
        try:
            if int(json.loads(file.read_text(encoding='utf-8')).get('pid') or 0) == pid:
                file.unlink()
        except (OSError, ValueError, AttributeError):
            continue


def live(session_id: str) -> dict | None:
    """The live record of a session whose harness process is still running, else None."""
    if not valid_id(session_id):
        return None
    try:
        data = json.loads(live_path(session_id).read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict) or not pid_alive(int(data.get('pid') or 0)):
        return None
    return data
