from __future__ import annotations

import math
import os
import re
import shlex
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

from . import registry
from .adapters import grok as grok_adapter
from .adapters.hermes import INTERACTIVE as HERMES_RESUMABLE
from .session import Session


class SendError(Exception):
    def __init__(self, code: int, message: str):
        super().__init__(message)
        self.code = code


@dataclass
class SendResult:
    command: list[str]
    reply: str


def command_for(session: Session, text: str) -> list[str]:
    """Build the non-interactive resume command for a supported harness."""
    if not session.id:
        raise SendError(2, 'The selected session has no session id.')
    if not text.strip():
        raise SendError(2, 'The request must not be empty.')
    if session.source == 't3code':
        raise SendError(2, 'This session belongs to a T3 Code thread and is list/route only: T3 keeps its own '
                           'resume point, so a CLI resume would fork it. Continue it in T3 Code.')
    if session.harness == 'claude':
        return ['claude', '--resume', session.id, '--print', text]
    if session.harness == 'codex':
        return ['codex', 'exec', 'resume', session.id, text]
    if session.harness == 'omp':
        return ['omp', '-r', session.id, '-p', text]
    if session.harness == 'pi':
        # Pi's -r opens a picker; --session takes the exact session file.
        return ['pi', '--session', session.path, '-p', text]
    if session.harness == 'hermes':
        if session.source and session.source not in HERMES_RESUMABLE:
            raise SendError(2, f'Hermes {session.source} sessions are list/route only: a CLI resume '
                               'would not reach that chat. Talk to it on its own platform.')
        return ['hermes', '-p', session.profile or 'default', 'chat', '--resume', session.id, '-Q', '-q', text]
    if session.harness == 'grok':
        return ['grok', '--resume', session.id, '-p', text]
    raise SendError(2, f'Unsupported harness: {session.harness}')


HARNESS_BIN = re.compile(r'(^|/)(claude|codex|omp|pi|hermes|grok)(\s|$)')  # only harness processes count, not greps/scripts
IDLE_QUIET = 60   # session file untouched this long = not mid-turn
IDLE_WAIT = 120   # how long send waits for a busy session before refusing
POLL = 5


def is_busy(session: Session, ps_out: str, now: float) -> bool:
    """Busy = its id is in a live process, or its session file was written in the last IDLE_QUIET s."""
    if session.id and any(session.id in line and HARNESS_BIN.search(line) for line in ps_out.splitlines()):
        return True
    if session.harness == 'hermes':
        mtime = session.last_active  # the path is a shared database, not this session's file
    elif session.harness == 'grok':
        try:
            mtime = grok_adapter.activity(Path(session.path))  # a directory: check its files
        except OSError:
            mtime = session.last_active
    else:
        try:
            mtime = os.stat(session.path).st_mtime
        except OSError:
            mtime = session.last_active
    return now - mtime < IDLE_QUIET


def wait_idle(session: Session, wait: float = IDLE_WAIT, poll: float = POLL,
              ps=registry._ps, clock=time.time, sleep=time.sleep) -> bool:
    deadline = clock() + wait
    while True:
        if not is_busy(session, ps(), clock()):
            return True
        if clock() >= deadline:
            return False
        sleep(poll)


def format_command(command: list[str]) -> str:
    return shlex.join(command)


def send(session: Session, text: str, timeout: float = 120, wait: float = IDLE_WAIT,
         env: dict | None = None) -> SendResult:
    """Wait for the session to go idle (up to `wait` s), resume it, capture its reply."""
    if not wait_idle(session, wait):
        raise SendError(
            4,
            f'Target session stayed busy for {wait:g}s; nothing was sent. '
            'Use `everett route` for a manual resume command.',
        )
    if not math.isfinite(timeout) or timeout <= 0:
        raise SendError(2, '--timeout must be a finite number greater than zero.')

    command = command_for(session, text)
    try:
        result = subprocess.run(
            command,
            cwd=session.cwd or None,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
            env=env or child_env(),  # card hooks stay silent for headless resumes
        )
    except subprocess.TimeoutExpired as exc:
        raise SendError(5, f'{session.harness} did not reply within {timeout:g}s.') from exc
    except OSError as exc:
        raise SendError(5, f'Could not start {session.harness}: {exc}') from exc

    if result.returncode:
        detail = (result.stderr or '').strip()
        if detail:
            detail = detail[-2000:]
            raise SendError(6, f'{session.harness} exited {result.returncode}: {detail}')
        raise SendError(6, f'{session.harness} exited with status {result.returncode}.')
    return SendResult(command, (result.stdout or '').strip())


# ---- spawning a NEW session ------------------------------------------------------------

SPAWNABLE = ('claude', 'codex', 'omp', 'pi', 'hermes', 'grok')


@dataclass
class SpawnResult:
    command: list[str]
    reply: str
    session_id: str
    harness: str
    cwd: str


def spawn_command(harness: str, text: str, session_id: str = '', out_file: str = '') -> list[str]:
    """Headless command that starts a NEW session with this request."""
    if not text.strip():
        raise SendError(2, 'The request must not be empty.')
    if harness == 'claude':
        return ['claude', '--session-id', session_id, '--print', text] if session_id else ['claude', '--print', text]
    if harness == 'codex':
        return ['codex', 'exec', '--skip-git-repo-check'] + (['-o', out_file] if out_file else []) + [text]
    if harness == 'omp':
        return ['omp', '-p', text]
    if harness == 'pi':
        return ['pi', '-p', text]
    if harness == 'hermes':
        return ['hermes', 'chat', '-Q', '-q', text]
    if harness == 'grok':
        return ['grok', '--session-id', session_id, '-p', text] if session_id else ['grok', '-p', text]
    raise SendError(2, f'Cannot spawn harness {harness!r}; choose one of {", ".join(SPAWNABLE)}.')


MAX_HOPS = 3


def hop_env() -> dict:
    """Env for a delivery: EVERETT_HOPS counts agent-to-agent forwards; refuse past MAX_HOPS."""
    try:
        hops = int(os.environ.get('EVERETT_HOPS', '0') or 0)
    except ValueError:
        hops = 0
    if hops >= MAX_HOPS:
        raise SendError(7, f'Hop limit reached ({hops}/{MAX_HOPS}): this request was already forwarded '
                           f'{hops} times between sessions. Answer it here instead of forwarding again.')
    return child_env({'EVERETT_HOPS': str(hops + 1)})


# Env vars a harness sets for its child processes (tools, MCP servers), checked 2026-09-24:
# Claude Code CLAUDE_CODE_SESSION_ID; Codex CODEX_THREAD_ID; Hermes HERMES_SESSION_ID;
# Pi PI_SESSION_FILE (a path; the id is the file's suffix). Grok documents GROK_SESSION_ID for hook
# processes (not verified for MCP servers). OMP sets none, so OMP callers pass session_id explicitly
# or set EVERETT_SESSION_ID.
IDENTITY_ENV = (
    ('EVERETT_SESSION_ID', ''),
    ('CLAUDE_CODE_SESSION_ID', 'claude'),
    ('CLAUDE_SESSION_ID', 'claude'),
    ('CODEX_THREAD_ID', 'codex'),
    ('HERMES_SESSION_ID', 'hermes'),
    ('PI_SESSION_FILE', 'pi'),
    ('GROK_SESSION_ID', 'grok'),
)


def caller_identity() -> dict:
    """{'session_id', 'harness', 'source'} of the session running this process, if detectable."""
    for name, harness in IDENTITY_ENV:
        value = os.environ.get(name, '').strip()
        if not value:
            continue
        if name == 'PI_SESSION_FILE':
            value = Path(value).stem.rsplit('_', 1)[-1]
        return {'session_id': value, 'harness': harness or os.environ.get('EVERETT_HARNESS_NAME', ''),
                'source': f'env {name}'}
    return {'session_id': '', 'harness': '', 'source': 'none'}


def caller_session_id() -> str:
    return caller_identity()['session_id']


def refuse_self(session: Session, caller: str | None = None) -> None:
    caller = caller_session_id() if caller is None else caller
    if caller and session.id and (session.id == caller or session.id.startswith(caller) or caller.startswith(session.id)):
        raise SendError(2, 'Refusing to send to the calling session itself.')


def child_env(extra: dict | None = None) -> dict:
    """Environment for a headless harness run: card hooks stay quiet, hop count carries over."""
    return {**os.environ, 'EVERETT_SEND': '1', **(extra or {})}


def _new_session_id(harness: str, cwd: str, since: float, output: str) -> str:
    """Find the session a spawn just created: parse the harness output, else rescan its store."""
    match = re.search(r'session id:\s*([0-9a-fA-F-]{36})', output) or re.search(
        r'"thread_id"\s*:\s*"([^"]+)"', output)
    if match:
        return match.group(1)
    adapter = registry.ADAPTERS.get(harness)
    if adapter is None:
        return ''
    try:
        fresh = [s for s in adapter.scan(1) if s.last_active >= since - 1
                 and (not cwd or os.path.realpath(s.cwd or '/') == os.path.realpath(cwd))]
    except Exception:  # noqa: BLE001
        return ''
    fresh.sort(key=lambda s: s.last_active, reverse=True)
    return fresh[0].id if fresh else ''


def record_spawn(harness: str, session_id: str, cwd: str, text: str) -> None:
    """Remember sessions Everett started, so ls/route show them although they ran headless."""
    from .session import home
    import json
    path = home() / '.everett' / 'spawned.jsonl'
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a', encoding='utf-8') as f:
        f.write(json.dumps({'ts': time.time(), 'harness': harness, 'id': session_id, 'cwd': cwd,
                            'text': text[:200]}) + '\n')


def spawn(harness: str, text: str, cwd: str, timeout: float = 300, env: dict | None = None) -> SpawnResult:
    """Start a NEW headless session in `cwd`, wait for its reply, return it with the new session id."""
    import tempfile
    import uuid
    if not os.path.isdir(cwd):
        raise SendError(2, f'Directory does not exist: {cwd}')
    if not math.isfinite(timeout) or timeout <= 0:
        raise SendError(2, '--timeout must be a finite number greater than zero.')
    session_id = str(uuid.uuid4()) if harness in ('claude', 'grok') else ''
    out_file = ''
    if harness == 'codex':
        fd, out_file = tempfile.mkstemp(prefix='everett-codex-', suffix='.txt')
        os.close(fd)
    command = spawn_command(harness, text, session_id, out_file)
    started = time.time()
    try:
        result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=timeout,
                                check=False, env=env or child_env())
    except subprocess.TimeoutExpired as exc:
        raise SendError(5, f'{harness} did not reply within {timeout:g}s.') from exc
    except OSError as exc:
        raise SendError(5, f'Could not start {harness}: {exc}') from exc
    try:
        if result.returncode:
            detail = (result.stderr or '').strip()[-2000:]
            raise SendError(6, f'{harness} exited {result.returncode}' + (f': {detail}' if detail else '.'))
        reply = (result.stdout or '').strip()
        if out_file:
            try:
                reply = Path(out_file).read_text(encoding='utf-8').strip() or reply
            except OSError:
                pass
    finally:
        if out_file:
            Path(out_file).unlink(missing_ok=True)
    session_id = session_id or _new_session_id(harness, cwd, started, (result.stdout or '') + (result.stderr or ''))
    if session_id:
        record_spawn(harness, session_id, cwd, text)
    return SpawnResult(command, reply, session_id, harness, cwd)


# ---- live delivery: running sessions get an inbox message instead of a resume --------------

MODES = ('auto', 'resume', 'inbox')
INBOX_HARNESSES = ('claude', 'codex', 'grok', 'omp')  # the harnesses whose hooks deliver the inbox


def attached(session: Session, ps_out: str | None = None) -> bool:
    """A harness process is attached to this session right now (its TUI is open or it is mid-run).

    Evidence: Everett's own hooks recorded a live process for it, or its id is on a harness
    command line. A headless resume here would race the open session, so it gets the inbox."""
    from . import inbox
    if inbox.live(session.id):
        return True
    ps_out = registry._ps() if ps_out is None else ps_out
    return bool(session.id) and any(session.id in line and HARNESS_BIN.search(line) for line in ps_out.splitlines())


def delivery_mode(session: Session, mode: str = 'auto', ps_out: str | None = None) -> str:
    if mode not in MODES:
        raise SendError(2, f'--mode must be one of {", ".join(MODES)}.')
    if mode != 'auto':
        return mode
    if session.source == 't3code':
        return 'inbox'  # a CLI resume would fork the T3 thread; its hooks can still deliver
    return 'inbox' if attached(session, ps_out) else 'resume'


def current_hops() -> int:
    try:
        return int(os.environ.get('EVERETT_HOPS', '0') or 0)
    except ValueError:
        return 0


def sender_info(caller: str | None = None) -> dict:
    """Who is sending: the calling session (with its card), else the human at a terminal."""
    from . import cards, inbox
    ident = caller_identity()
    sid = caller if caller is not None else ident['session_id']
    card = ''
    if sid:
        found = cards.read_card(sid)
        card = found[0] if found else ''
    return {'sender': sid or inbox.HUMAN, 'from_harness': ident['harness'] if sid else '',
            'from_card': card}


def send_inbox(session: Session, text: str, wait: float = 0, caller: str | None = None,
               poll: float = 1.0) -> dict:
    """Queue `text` in a running session's inbox; optionally wait for its reply."""
    from . import inbox
    if not math.isfinite(wait) or wait < 0:
        raise SendError(2, '--wait must be a finite number of seconds, 0 or more.')
    hops = current_hops()
    if hops >= MAX_HOPS:
        raise SendError(7, f'Hop limit reached ({hops}/{MAX_HOPS}): answer here instead of forwarding again.')
    who = sender_info(caller)
    try:
        message = inbox.post(session.id, text, sender=who['sender'], hops=hops + 1,
                             from_harness=who['from_harness'], from_card=who['from_card'])
    except inbox.InboxError as e:
        raise SendError(e.code, str(e)) from e
    result = {'mode': 'inbox', 'message_id': message['id'], 'reply_inbox': who['sender'], 'reply': None,
              'hooked': session.harness in INBOX_HARNESSES}
    if wait > 0:
        reply = inbox.wait_reply(who['sender'], message['id'], wait, poll=poll)
        if reply:
            result['reply'] = reply['text']
            result['reply_from'] = reply.get('from')
    return result


def reply(message_id: str, text: str, caller: str | None = None) -> dict:
    """Answer an inbox message: the reply goes to the original sender's inbox."""
    from . import inbox
    original = inbox.find(message_id.strip())
    if original is None:
        raise SendError(2, f'No message with id {message_id!r} in any Everett inbox.')
    try:
        hops = inbox.reply_hops(original, current_hops())
        who = sender_info(caller)
        message = inbox.post(original.get('from') or inbox.HUMAN, text, sender=who['sender'], kind='reply',
                             reply_to=original['id'], hops=hops, from_harness=who['from_harness'],
                             from_card=who['from_card'])
    except inbox.InboxError as e:
        raise SendError(e.code, str(e)) from e
    return {'mode': 'reply', 'message_id': message['id'], 'reply_to': original['id'], 'to': message['to']}
