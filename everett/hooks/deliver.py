"""Live delivery: inject a running session's pending Everett messages at its next turn or tool call.

Used by the UserPromptSubmit and PostToolUse hooks of Claude Code, Codex, and Grok. Stdin is the
hook JSON; stdout is `hookSpecificOutput.additionalContext` JSON, or nothing. Imports only the
light inbox module (this runs after every tool call) and never raises.
"""
from __future__ import annotations

import json
import os
import re

EVENTS = ('UserPromptSubmit', 'PostToolUse')
_EVENT_ALIASES = {re.sub(r'[^a-z0-9]', '', e.lower()): e for e in EVENTS}


def _event_name(data: dict) -> str:
    """Harnesses send hook_event_name (Claude), hookEventName (Grok), or snake_case values."""
    raw = data.get('hook_event_name') or data.get('hookEventName') or ''
    key = re.sub(r'[^a-z0-9]', '', str(raw).lower())
    return _EVENT_ALIASES.get(key, '')


def output(raw: str, harness: str) -> str:
    """Hook stdin JSON -> hook stdout JSON ('' to stay silent)."""
    if os.environ.get('EVERETT_SEND'):
        return ''  # headless `everett send` resumes: the request is the message
    if harness == 'claude' and os.environ.get('CLAUDE_CODE_ENTRYPOINT') == 'sdk-cli':
        return ''
    try:
        data = json.loads(raw)
    except ValueError:
        return ''
    if not isinstance(data, dict):
        return ''
    grok = harness == 'grok' or 'sessionId' in data  # Grok also runs ~/.claude/settings.json hooks
    session_id = data.get('session_id') or data.get('sessionId') or ''
    event = _event_name(data)
    if not event or not isinstance(session_id, str):
        return ''
    from everett import inbox
    if not inbox.valid_id(session_id):
        return ''
    try:
        inbox.touch_live(session_id, 'grok' if grok else harness, 'turn')
    except OSError:
        pass
    try:  # blocked-too-long escalation, throttled to once a minute across all sessions
        from everett import events
        events.maybe_escalate()
    except Exception:  # noqa: BLE001
        pass
    if grok and event == 'UserPromptSubmit':
        return ''  # Grok discards UserPromptSubmit context; wait for its PostToolUse
    try:
        if not inbox.path(session_id).exists():
            return ''
    except inbox.InboxError:
        return ''
    text = inbox.take(session_id)
    if not text:
        return ''
    return json.dumps({'hookSpecificOutput': {'hookEventName': event, 'additionalContext': text}},
                      ensure_ascii=False)


def run(harness: str) -> int:
    import sys
    try:
        out = output(sys.stdin.read(), harness)
        if out:
            sys.stdout.write(out + '\n')
    except BaseException:  # noqa: BLE001  a hook must never break the session over Everett
        pass
    return 0
