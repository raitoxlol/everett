"""Shared card context for Claude Code, Codex, OMP, and Grok hooks."""
from __future__ import annotations

import json
import os
import re
import tempfile
from pathlib import Path

from ..adapters import claude, codex, grok
from ..cards import AUTO_MARKER, INSTRUCTION, card_path
from ..session import read_edges

SKIP_ENV = 'EVERETT_SEND'  # set by `everett send`; headless resumes don't get cards


def session_start_context(session_id: str, cwd: str = '') -> str:
    """Card instruction plus the shared core for a new session ('' to stay silent)."""
    if os.environ.get(SKIP_ENV) or not session_id:
        return ''
    path = card_path(session_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    text = INSTRUCTION.format(path=path)
    try:  # the core is best effort: a broken core must never cost the card instruction
        from ..core import context
        shared = context(cwd or os.getcwd())
    except Exception:  # noqa: BLE001
        shared = ''
    return f'{text}\n\n{shared}' if shared else text


def session_start_output(raw: str) -> str:
    """Hook stdin JSON -> stdout JSON with the card instruction, or '' to stay silent."""
    try:
        data = json.loads(raw)
    except ValueError:
        return ''
    sid = data.get('session_id') if isinstance(data, dict) else None
    if not sid or not isinstance(sid, str):
        return ''
    cwd = data.get('cwd') if isinstance(data.get('cwd'), str) else ''
    context = session_start_context(sid, cwd)
    if not context:
        return ''
    return json.dumps({'hookSpecificOutput': {
        'hookEventName': 'SessionStart',
        'additionalContext': context,
    }})


_TABLE_SEPARATOR = re.compile(r'^\s*\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)+\|?\s*$')
_TOOL_NOISE = re.compile(
    r'^\s*(?:assistant\s+to=|user\s+to=|to=(?:functions|mcp__|tool)|'
    r'\[?(?:tool call|tool result|function call|function result)\]?:?)', re.I,
)


def _word_limit(text: str, limit: int) -> str:
    words = text.split()
    if len(words) <= limit:
        return ' '.join(words)
    if limit <= 0:
        return '…'
    return ' '.join((*words[:limit - 1], '…'))


def _sentence(text: str) -> str:
    text = ' '.join(text.split())
    match = re.search(r'[.!?。！？](?=\s|$)', text)
    return text[:match.end()].strip() if match else text


def _markdown_lines(text: str) -> list[tuple[str, bool, bool]]:
    """Return cleaned lines with heading and bold-only metadata."""
    source = (text or '').splitlines()
    table_lines: set[int] = set()
    setext_lines = {index for index in range(len(source) - 1)
                    if source[index].strip() and re.match(r'^\s*(?:=+|-+)\s*$', source[index + 1])}
    for index, line in enumerate(source):
        if not _TABLE_SEPARATOR.match(line):
            continue
        table_lines.add(index)
        for step in (-1, 1):
            cursor = index + step
            while 0 <= cursor < len(source) and '|' in source[cursor]:
                table_lines.add(cursor)
                cursor += step

    cleaned: list[tuple[str, bool, bool]] = []
    fence: tuple[str, int] | None = None
    for index, raw_line in enumerate(source):
        fence_match = re.match(r'^\s*(`{3,}|~{3,})', raw_line)
        if fence:
            if fence_match and fence_match.group(1)[0] == fence[0] and len(fence_match.group(1)) >= fence[1]:
                fence = None
            continue
        if fence_match:
            fence = (fence_match.group(1)[0], len(fence_match.group(1)))
            continue
        if index in table_lines or _TABLE_SEPARATOR.match(raw_line):
            continue
        if index + 1 in setext_lines or re.match(r'^\s*(?:=+|-+)\s*$', raw_line):
            continue
        if raw_line.lstrip().startswith('|') and raw_line.rstrip().endswith('|'):
            continue
        if _TOOL_NOISE.match(raw_line) or re.match(r'^\s*\[?(?:tool|function)_(?:call|result)\b', raw_line, re.I):
            continue
        if re.match(r'^\s*(?:[-*_]\s*){3,}$', raw_line):
            continue

        heading = bool(re.match(r'^\s{0,3}#{1,6}\s+', raw_line)) or index in setext_lines
        bold_match = re.fullmatch(r'\s*(?:\*\*|__)(.+?)(?:\*\*|__)\s*', raw_line)
        line = re.sub(r'<!--.*?-->', ' ', raw_line)
        if re.match(r'^\s*\[[^]]+\]:\s*\S+', line):
            continue
        line = re.sub(r'!?\[([^]]*)\]\([^)]*\)', r'\1', line)
        line = re.sub(r'\[([^]]+)\]\[[^]]*\]', r'\1', line)
        line = re.sub(r'<https?://[^>]+>', ' ', line)
        line = re.sub(r'https?://\S+', ' ', line)
        line = re.sub(r'<[^>]{1,200}>', ' ', line)
        line = re.sub(r'^\s{0,3}#{1,6}\s+', '', line)
        line = re.sub(r'^\s*>+\s?', '', line)
        line = re.sub(r'^\s*(?:[-+*]|\d+[.)])\s+', '', line)
        line = re.sub(r'[*_~`]', '', line)
        line = ' '.join(line.split())
        if line and not _TOOL_NOISE.match(line):
            cleaned.append((line, heading, bool(bold_match)))
    return cleaned


def _intent(text: str) -> str:
    lines = _markdown_lines(text)
    body = next((line for line, heading, _ in lines if not heading), '')
    return _sentence(body)


def _next_step(text: str) -> str:
    lines = _markdown_lines(text)
    explicit = [i for i, (line, heading, bold_only) in enumerate(lines)
                if not heading and (re.search(r'\bnext\b|次のステップ|次は|次に', line, re.I) or bold_only)]
    for index in reversed(explicit):
        line = lines[index][0]
        bold_only = lines[index][2]
        sentences = re.split(r'(?<=[.!?。！？])\s+', line)
        if not bold_only:
            line = next((sentence for sentence in sentences
                         if re.search(r'\bnext\b|次のステップ|次は|次に', sentence, re.I)), line)
        else:
            line = sentences[0]
        line = re.sub(r'^\s*(?:the\s+)?next(?:\s+step)?\s*(?:is\s+to\s+)?[:,—–-]?\s*', '', line, flags=re.I)
        line = re.sub(r'^\s*次のステップ\s*[:：、]?\s*', '', line)
        line = re.sub(r'^\s*次(?:は|に)\s*[:：、]?\s*', '', line)
        if line:
            return _sentence(line)
        for following, heading, _ in lines[index + 1:]:
            if not heading and following:
                return _sentence(following)
    return next((_sentence(line) for line, heading, _ in lines if not heading), '')


def _project_name(cwd: str) -> str:
    path = Path(cwd).expanduser() if cwd else None
    # The home directory is not a project; use the generic label instead.
    if path is None or path == Path.home() or path == Path('/'):
        return 'Project'
    name = path.name.strip()
    return name[:1].upper() + name[1:] if name else 'Project'


def _auto_card(transcript: Path, harness: str, cwd: str = '') -> str:
    adapter = {'claude': claude, 'codex': codex, 'grok': grok}.get(harness)
    if adapter is None:
        return ''
    head, tail = read_edges(transcript)
    rows = head + tail
    users = [text for row in rows if (text := adapter.user_text(row, raw=True))]
    tail_users = [text for row in tail if (text := adapter.user_text(row, raw=True))]
    assistants = [text for row in rows if (text := adapter.assistant_text(row, raw=True))]
    if not users and not assistants:
        return ''
    if not cwd:
        cwd = next((row.get('cwd') or (row.get('payload') or {}).get('cwd')
                    for row in rows if row.get('cwd') or (row.get('payload') or {}).get('cwd')), '')
    cwd = cwd or os.getcwd()
    latest_users = tail_users or users
    what = next((intent for text in users if (intent := _intent(text))), 'No user request found')
    state = next((intent for text in reversed(latest_users) if (intent := _intent(text))),
                 'No user request found')
    next_step = next((step for text in reversed(assistants) if (step := _next_step(text))),
                     'No assistant response found')
    project = _project_name(cwd)
    if project != 'Project':
        what = f'{project}: {what}'
    # The marker and labels occupy six words; these limits leave 44 for content.
    return '\n'.join((
        AUTO_MARKER,
        f'What: {_word_limit(what, 14)}',
        f'State: {_word_limit(state, 14)}',
        f'Next: {_word_limit(next_step, 16)}',
    )) + '\n'


def stop_hook(raw: str, harness: str) -> None:
    """Write a deterministic fallback card, silently ignoring every hook error."""
    temp_path: Path | None = None
    try:
        if os.environ.get(SKIP_ENV) or (harness == 'claude' and
                                        os.environ.get('CLAUDE_CODE_ENTRYPOINT') == 'sdk-cli'):
            return
        data = json.loads(raw)
        if not isinstance(data, dict):
            return
        session_id = data.get('session_id')
        transcript_value = data.get('transcript_path')
        if (not isinstance(session_id, str) or not re.fullmatch(r'[A-Za-z0-9._-]+', session_id)
                or session_id in ('.', '..') or not isinstance(transcript_value, str) or not transcript_value):
            return
        transcript = Path(transcript_value)
        transcript_mtime = transcript.stat().st_mtime_ns
        target = card_path(session_id)
        if target.is_symlink():
            return
        try:
            current = target.read_text(encoding='utf-8')
            current_stat = target.stat()
            existed = True
        except FileNotFoundError:
            current = ''
            current_stat = None
            existed = False
        except OSError:
            return

        if existed and (current.splitlines()[:1] != [AUTO_MARKER]
                        or current_stat.st_mtime_ns >= transcript_mtime):
            return
        content = _auto_card(transcript, harness, data.get('cwd', ''))
        if not content:
            return
        target.parent.mkdir(parents=True, exist_ok=True)
        if not existed:
            # Exclusive create makes an agent card that appeared meanwhile win.
            try:
                fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            except FileExistsError:
                return
            with os.fdopen(fd, 'w', encoding='utf-8') as f:
                f.write(content)
            return

        # Confirm the stale auto card did not become an agent card while parsing.
        latest = target.read_text(encoding='utf-8')
        latest_stat = target.stat()
        if (latest.splitlines()[:1] != [AUTO_MARKER]
                or latest_stat.st_mtime_ns != current_stat.st_mtime_ns
                or latest != current):
            return
        with tempfile.NamedTemporaryFile('w', encoding='utf-8', dir=target.parent,
                                         prefix=f'.{target.name}.', delete=False) as f:
            temp_path = Path(f.name)
            f.write(content)
        if (target.is_symlink()
                or target.read_text(encoding='utf-8') != current
                or target.stat().st_mtime_ns != current_stat.st_mtime_ns):
            temp_path.unlink(missing_ok=True)
            return
        os.replace(temp_path, target)
    except BaseException:  # Stop hooks must never interrupt a harness session.
        if temp_path is not None:
            try:
                temp_path.unlink(missing_ok=True)
            except OSError:
                pass
        return


def _last_assistant(data: dict, harness: str) -> str:
    """The turn's final assistant text: from the hook input when the harness sends it, else the transcript."""
    for key in ('last_assistant_message', 'lastAssistantMessage'):
        value = data.get(key)
        if isinstance(value, str) and value.strip():
            return value
    adapter = {'claude': claude, 'codex': codex, 'grok': grok}.get(harness)
    transcript = data.get('transcript_path')
    if adapter is None or not isinstance(transcript, str) or not transcript:
        return ''
    head, tail = read_edges(Path(transcript))
    for row in reversed(tail or head):
        text = adapter.assistant_text(row, raw=True)
        if text:
            return text
    return ''


def stop_event(raw: str, harness: str) -> None:
    """Stop hook awareness: record done / blocked / needs-input for this session (debounced), mark it idle,
    and run the throttled escalation check. Silent on every error."""
    try:
        if os.environ.get(SKIP_ENV) or (harness == 'claude' and
                                        os.environ.get('CLAUDE_CODE_ENTRYPOINT') == 'sdk-cli'):
            return
        data = json.loads(raw) if isinstance(raw, str) else raw
        if not isinstance(data, dict) or data.get('stop_hook_active') or data.get('stopHookActive'):
            return
        session_id = data.get('session_id') or data.get('sessionId')
        if not isinstance(session_id, str) or not re.fullmatch(r'[A-Za-z0-9._-]{1,128}', session_id):
            return
        from .. import events, inbox
        try:
            inbox.touch_live(session_id, harness, 'idle')
        except OSError:
            pass
        found = events.classify(_last_assistant(data, harness))
        if found:
            cwd = data.get('cwd') if isinstance(data.get('cwd'), str) else ''
            try:
                events.record(found[0], found[1], session=session_id, harness=harness, cwd=cwd or os.getcwd(),
                              source='auto')
            except events.EventError:
                pass
        events.maybe_escalate()
    except BaseException:  # noqa: BLE001  Stop hooks must never interrupt a harness session
        return


def grok_stop_hook(raw: str) -> None:
    """Grok's Stop payload -> the common stop hook (it has no transcript path of its own)."""
    try:
        data = json.loads(raw)
        if not isinstance(data, dict):
            return
        session_id = data.get('sessionId') or data.get('session_id') or os.environ.get('GROK_SESSION_ID', '')
        cwd = data.get('cwd') if isinstance(data.get('cwd'), str) else ''
        if not isinstance(session_id, str) or not re.fullmatch(r'[A-Za-z0-9._-]+', session_id):
            return
        path = grok.transcript(session_id, cwd)
        if path is not None:
            stop_hook(json.dumps({'session_id': session_id, 'transcript_path': str(path), 'cwd': cwd}), 'grok')
        stop_event(json.dumps({'session_id': session_id, 'transcript_path': str(path) if path else '', 'cwd': cwd,
                               'lastAssistantMessage': data.get('lastAssistantMessage') or '',
                               'stopHookActive': bool(data.get('stopHookActive'))}), 'grok')
    except BaseException:  # noqa: BLE001  hooks never interrupt the session
        return
