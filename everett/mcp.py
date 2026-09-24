"""`everett mcp`: a stdio MCP server (JSON-RPC 2.0, newline-delimited), standard library only.

Every harness that speaks MCP gets Everett as native tools: see the sessions, route work,
deliver it, and share one core. Protocol output goes to stdout only; diagnostics go to
~/.everett/mcp.log.
"""
from __future__ import annotations

import json
import os
import sys
import time
import traceback

from . import __version__, cards, core, registry
from .hooks.common import _word_limit
from .route import MIN_CONFIDENCE, RouteError, best_dir, default_harness, route
from .send import (SPAWNABLE, SendError, caller_identity, command_for, hop_env, refuse_self, send, spawn)
from .session import Session, home

PROTOCOL_VERSIONS = ('2025-06-18', '2025-03-26', '2024-11-05')  # newest first

PARSE_ERROR, INVALID_REQUEST, METHOD_NOT_FOUND, INVALID_PARAMS, INTERNAL_ERROR = -32700, -32600, -32601, -32602, -32603

INSTRUCTIONS = (
    'Everett is the layer above every coding-agent session on this machine (Claude Code, Codex, OMP, Pi, Hermes). '
    'Use everett_ls to see what other sessions are working on, everett_route to find the session a task belongs to, '
    'and everett_send to hand that session work and get its reply. Use everett_learn to push durable facts that '
    'every session should know into the shared core, and everett_core to read it. Keep your own card current '
    'with everett_card so others can route to you.'
)

S = {'type': 'string'}
TOOLS = [
    {
        'name': 'everett_ls',
        'description': ('List recent coding-agent sessions on this machine (all harnesses), newest first, each with '
                        'its card: what it is working on, state, next step. Use it to see what the parallel '
                        'sessions are doing before routing or sending work.'),
        'inputSchema': {'type': 'object', 'properties': {
            'hours': {'type': 'number', 'description': 'Look-back window in hours (default 72).', 'minimum': 0},
            'harness': {'type': 'string', 'enum': ['claude', 'codex', 'omp', 'pi', 'hermes', 'grok'],
                        'description': 'Only this harness.'},
        }, 'additionalProperties': False},
    },
    {
        'name': 'everett_route',
        'description': ('Decide which existing session a request continues. Returns decision SESSION (with the '
                        'session), NEW (belongs to no session), or ASK (ambiguous; the closest candidate is '
                        'given), plus a confidence. Read-only: it never sends anything.'),
        'inputSchema': {'type': 'object', 'properties': {
            'text': {**S, 'description': 'The request, as you would send it.'},
            'router': {'type': 'string', 'enum': ['local', 'jev'], 'description': 'Default: jev if configured, else local.'},
        }, 'required': ['text'], 'additionalProperties': False},
    },
    {
        'name': 'everett_send',
        'description': ('Deliver a request to another session and return its reply. Without `to`, Everett routes '
                        'the text; only a SESSION decision is delivered. With `to` (session id prefix, card name, '
                        'or project folder) routing is skipped. A NEW decision starts a new headless session only '
                        'when spawn=true. It waits for a busy target to go idle (up to 2 minutes). It refuses to '
                        'send to your own session and refuses requests forwarded more than 3 times.'),
        'inputSchema': {'type': 'object', 'properties': {
            'text': {**S, 'description': 'The request for the other session. Make it self-contained.'},
            'to': {**S, 'description': 'Target session: id prefix, card name, or project folder name.'},
            'spawn': {'type': 'boolean', 'description': 'Allow starting a NEW session when routing says NEW.', 'default': False},
            'dir': {**S, 'description': 'Working directory for a spawned session.'},
            'harness': {'type': 'string', 'enum': list(SPAWNABLE), 'description': 'Harness for a spawned session.'},
            'timeout': {'type': 'number', 'description': 'Seconds to wait for the reply (default 300).', 'minimum': 1},
            'session_id': {**S, 'description': 'Your own session id, if Everett cannot detect it (see everett_whoami).'},
        }, 'required': ['text'], 'additionalProperties': False},
    },
    {
        'name': 'everett_learn',
        'description': ('Push one durable fact (a decision, convention, gotcha, path, or command) into the shared '
                        'core that every session starts from. Max 500 characters. Anything that looks like a '
                        'secret is rejected. Use scope "project" for facts only this project needs.'),
        'inputSchema': {'type': 'object', 'properties': {
            'fact': {**S, 'description': 'One self-contained fact.', 'maxLength': core.MAX_LEARNING},
            'project': {**S, 'description': 'Project name (default: the current folder\'s project).'},
            'scope': {'type': 'string', 'enum': ['global', 'project']},
        }, 'required': ['fact'], 'additionalProperties': False},
    },
    {
        'name': 'everett_core',
        'description': 'Read the shared core: the global facts plus this (or the named) project\'s facts.',
        'inputSchema': {'type': 'object', 'properties': {
            'project': {**S, 'description': 'Project name (default: the current folder\'s project).'},
        }, 'additionalProperties': False},
    },
    {
        'name': 'everett_card',
        'description': ('Write YOUR session\'s card, which other agents use to route work to you. Keep it short '
                        '(50 words total). Rewrite it when your topic or state changes.'),
        'inputSchema': {'type': 'object', 'properties': {
            'what': {**S, 'description': 'What this session is about.'},
            'state': {**S, 'description': 'Where it stands now.'},
            'next': {**S, 'description': 'The next step.'},
            'session_id': {**S, 'description': 'Your session id, if Everett cannot detect it.'},
        }, 'required': ['what', 'state', 'next'], 'additionalProperties': False},
    },
    {
        'name': 'everett_whoami',
        'description': ('Your own session id and harness as Everett detects them, and how it detected them. If '
                        'none is detected, pass session_id explicitly to everett_send and everett_card.'),
        'inputSchema': {'type': 'object', 'properties': {}, 'additionalProperties': False},
    },
]
TOOL_NAMES = {t['name'] for t in TOOLS}


class ToolError(Exception):
    """A tool-level failure: reported to the agent as isError content, not a protocol error."""


class ParamsError(Exception):
    """Invalid arguments: reported as JSON-RPC -32602."""


# ---- logging -------------------------------------------------------------------------------

def log(event: str, **fields) -> None:
    try:
        path = home() / '.everett' / 'mcp.log'
        path.parent.mkdir(parents=True, exist_ok=True)
        clipped = {k: (v[:200] + '…' if isinstance(v, str) and len(v) > 200 else v) for k, v in fields.items()}
        with path.open('a', encoding='utf-8') as f:
            f.write(json.dumps({'ts': time.strftime('%Y-%m-%dT%H:%M:%S'), 'event': event, **clipped},
                               ensure_ascii=False) + '\n')
    except Exception:  # noqa: BLE001  logging must never break the protocol
        pass


# ---- tools ---------------------------------------------------------------------------------

def _brief(s: Session) -> dict:
    return {'id': s.id, 'harness': s.harness, 'cwd': s.cwd, 'running': s.running,
            'minutes_ago': int(max(0, time.time() - s.last_active) // 60),
            'card': s.card, 'card_source': s.card_source, 'title': s.title,
            'first_request': s.first_user[:160], 'last_request': s.last_user[:160],
            **({'profile': s.profile} if s.profile else {}), **({'source': s.source} if s.source else {})}


def _str(args: dict, key: str, required: bool = False) -> str:
    value = args.get(key)
    if value is None:
        if required:
            raise ParamsError(f'"{key}" is required')
        return ''
    if not isinstance(value, str):
        raise ParamsError(f'"{key}" must be a string')
    if required and not value.strip():
        raise ParamsError(f'"{key}" must not be empty')
    return value


def tool_ls(args):
    hours = args.get('hours', 72)
    if not isinstance(hours, (int, float)) or isinstance(hours, bool) or hours < 0:
        raise ParamsError('"hours" must be a non-negative number')
    sessions = registry.scan(float(hours), harness=_str(args, 'harness'))
    me = caller_identity()['session_id']
    return {'sessions': [{**_brief(s), **({'you': True} if me and s.id == me else {})} for s in sessions]}


def tool_route(args):
    text = _str(args, 'text', True)
    router = _str(args, 'router') or None
    try:
        r = route(text, registry.scan(72), router=router)
    except RouteError as e:
        raise ToolError(str(e)) from e
    out = {k: r.get(k) for k in ('decision', 'confidence', 'router', 'suggested', 'command') if r.get(k) is not None}
    if r.get('session'):
        out['session'] = _brief(Session(**r['session']))
    return out


def tool_send(args):
    text = _str(args, 'text', True)
    to = _str(args, 'to')
    spawn_ok = args.get('spawn', False)
    if not isinstance(spawn_ok, bool):
        raise ParamsError('"spawn" must be a boolean')
    timeout = args.get('timeout', 300)
    if not isinstance(timeout, (int, float)) or isinstance(timeout, bool) or timeout <= 0:
        raise ParamsError('"timeout" must be a positive number')
    caller = _str(args, 'session_id') or caller_identity()['session_id']
    try:
        env = hop_env()
        if to:
            if spawn_ok:
                raise ParamsError('"to" and "spawn" are exclusive')
            try:
                session = registry.find(to, registry.scan(72, include_auto=True, limit=None))
            except registry.SessionLookupError as e:
                raise ToolError(str(e)) from e
            decision = {'decision': 'SESSION', 'router': 'direct', 'confidence': 1.0}
        else:
            sessions = registry.scan(72)
            try:
                r = route(text, sessions)
            except RouteError as e:
                raise ToolError(str(e)) from e
            decision = {k: r.get(k) for k in ('decision', 'confidence', 'router')}
            if r['decision'] == 'NEW':
                if not spawn_ok or r['confidence'] < MIN_CONFIDENCE:
                    return {**decision, 'delivered': False,
                            'note': 'No existing session fits. Nothing was sent; call again with spawn=true to start one.'}
                harness = _str(args, 'harness') or default_harness()
                cwd = os.path.abspath(os.path.expanduser(_str(args, 'dir'))) if args.get('dir') else (
                    best_dir(text, sessions) or os.getcwd())
                result = spawn(harness, text, cwd, timeout=float(timeout), env=env)
                return {**decision, 'delivered': True, 'spawned': True, 'harness': harness, 'cwd': cwd,
                        'session_id': result.session_id, 'reply': result.reply}
            if r['decision'] != 'SESSION':
                return {**decision, 'delivered': False, 'suggested': r.get('suggested'),
                        'note': 'Ambiguous. Nothing was sent; pass `to` with the session you mean.'}
            session = Session(**r['session'])
        refuse_self(session, caller)
        command_for(session, text)  # validates the harness can be resumed before waiting
        result = send(session, text, timeout=float(timeout), env=env)
    except SendError as e:
        raise ToolError(str(e)) from e
    return {**decision, 'delivered': True, 'session': _brief(session), 'reply': result.reply}


def tool_learn(args):
    fact = _str(args, 'fact', True)
    try:
        entry = core.learn(fact, project=_str(args, 'project') or None, scope=_str(args, 'scope') or None)
    except core.CoreError as e:
        raise ToolError(str(e)) from e
    return {'learned': True, 'scope': entry['scope'], 'project': entry['project'],
            'note': 'Queued in the inbox; it reaches every session after the next `everett trunk merge`.'}


def tool_core(args):
    project = _str(args, 'project')
    if project:
        text = '\n\n'.join(t for t in (core._read(core.global_path()).strip(),
                                        core._read(core.project_path(project)).strip()) if t)
    else:
        text = core.context(os.getcwd())
    return {'core': text or '(the shared core is empty)', 'pending_learnings': len(core.read_inbox())}


def tool_card(args):
    parts = {k: ' '.join(_str(args, k, True).split()) for k in ('what', 'state', 'next')}
    sid = _str(args, 'session_id') or caller_identity()['session_id']
    if not sid:
        raise ToolError('Cannot tell which session you are. Pass session_id (your harness session id).')
    if '/' in sid or sid in ('.', '..'):
        raise ParamsError('"session_id" is not a valid session id')
    body = '\n'.join((f'What: {_word_limit(parts["what"], 16)}', f'State: {_word_limit(parts["state"], 14)}',
                      f'Next: {_word_limit(parts["next"], 14)}')) + '\n'
    path = cards.card_path(sid)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f'.{path.name}.tmp')
    tmp.write_text(body, encoding='utf-8')
    tmp.replace(path)
    return {'written': str(path), 'card': body.strip()}


def tool_whoami(args):
    ident = caller_identity()
    hops = os.environ.get('EVERETT_HOPS', '0')
    return {**ident, 'hops': int(hops) if hops.isdigit() else 0, 'cwd': os.getcwd(),
            'note': '' if ident['session_id'] else
            'Not detected. Claude Code, Codex, Hermes, and Pi expose it; others must pass session_id explicitly.'}


HANDLERS = {'everett_ls': tool_ls, 'everett_route': tool_route, 'everett_send': tool_send,
            'everett_learn': tool_learn, 'everett_core': tool_core, 'everett_card': tool_card,
            'everett_whoami': tool_whoami}


# ---- JSON-RPC ------------------------------------------------------------------------------

def _error(msg_id, code: int, message: str) -> dict:
    return {'jsonrpc': '2.0', 'id': msg_id, 'error': {'code': code, 'message': message}}


def _result(msg_id, result: dict) -> dict:
    return {'jsonrpc': '2.0', 'id': msg_id, 'result': result}


def call_tool(params: dict) -> dict:
    name = params.get('name')
    args = params.get('arguments') or {}
    if name not in TOOL_NAMES:
        raise ParamsError(f'unknown tool: {name}')
    if not isinstance(args, dict):
        raise ParamsError('"arguments" must be an object')
    started = time.time()
    log('call', tool=name, args=json.dumps(args, ensure_ascii=False), caller=caller_identity()['session_id'])
    try:
        data = HANDLERS[name](args)
        is_error = False
    except ToolError as e:
        data, is_error = {'error': str(e)}, True
    log('result', tool=name, ok=not is_error, ms=int((time.time() - started) * 1000),
        detail=data.get('error', '') if is_error else '')
    text = data['error'] if is_error else json.dumps(data, ensure_ascii=False, indent=1)
    out = {'content': [{'type': 'text', 'text': text}], 'isError': is_error}
    if not is_error:
        out['structuredContent'] = data
    return out


def handle(message) -> dict | None:
    """One JSON-RPC message in, one response out (None for notifications)."""
    if not isinstance(message, dict) or message.get('jsonrpc') != '2.0' or not isinstance(message.get('method'), str):
        msg_id = message.get('id') if isinstance(message, dict) else None
        return _error(msg_id, INVALID_REQUEST, 'Invalid Request')
    method, msg_id = message['method'], message.get('id')
    is_notification = 'id' not in message
    params = message.get('params') or {}
    if not isinstance(params, dict):
        return None if is_notification else _error(msg_id, INVALID_PARAMS, 'params must be an object')
    if is_notification:
        return None  # notifications/initialized, notifications/cancelled, ...: nothing to answer
    try:
        if method == 'initialize':
            asked = params.get('protocolVersion')
            version = asked if asked in PROTOCOL_VERSIONS else PROTOCOL_VERSIONS[0]
            log('initialize', client=json.dumps(params.get('clientInfo') or {}), asked=str(asked), version=version)
            return _result(msg_id, {'protocolVersion': version,
                                    'capabilities': {'tools': {'listChanged': False}},
                                    'serverInfo': {'name': 'everett', 'version': __version__},
                                    'instructions': INSTRUCTIONS})
        if method == 'ping':
            return _result(msg_id, {})
        if method == 'tools/list':
            return _result(msg_id, {'tools': TOOLS})
        if method == 'tools/call':
            return _result(msg_id, call_tool(params))
        return _error(msg_id, METHOD_NOT_FOUND, f'Method not found: {method}')
    except ParamsError as e:
        return _error(msg_id, INVALID_PARAMS, str(e))
    except Exception as e:  # noqa: BLE001
        log('internal_error', method=method, error=f'{e}', trace=traceback.format_exc()[-200:])
        return _error(msg_id, INTERNAL_ERROR, f'Internal error: {e}')


def serve(stdin=None, stdout=None) -> int:
    stdin = stdin or sys.stdin
    stdout = stdout or sys.stdout
    sys.stdout = sys.stderr  # stray prints from tool code must never corrupt the protocol stream
    log('start', pid=os.getpid(), cwd=os.getcwd(), hops=os.environ.get('EVERETT_HOPS', '0'))
    for line in stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except ValueError:
            responses = [_error(None, PARSE_ERROR, 'Parse error')]
        else:
            if isinstance(message, list):  # batches (2025-03-26); answer each
                responses = [handle(m) for m in message] if message else [_error(None, INVALID_REQUEST, 'Invalid Request')]
                responses = [r for r in responses if r is not None]
                if responses:
                    stdout.write(json.dumps(responses, ensure_ascii=False) + '\n')
                    stdout.flush()
                continue
            responses = [handle(message)]
        for response in responses:
            if response is not None:
                stdout.write(json.dumps(response, ensure_ascii=False) + '\n')
                stdout.flush()
    log('stop')
    return 0
