"""`everett doctor`: one screen of what Everett can see and what is missing."""
from __future__ import annotations

import json
import math
import os
import shutil
import subprocess
import sys

from . import config, install, registry, trunk, trunk_schedule
from .adapters import devin as devin_adapter
from .adapters import t3code
from .route import find_api_key
from .session import home

STORES = {
    'claude': '.claude/projects',
    'codex': '.codex/sessions',
    'omp': '.omp/agent/sessions',
    'pi': '.pi/agent/sessions',
    'hermes': '.hermes',
    'grok': '.grok/sessions',
    'devin': devin_adapter.DEFAULT_REL,  # sessions.db + transcripts/; $DEVIN_HOME overrides
    't3code': '.t3/userdata/state.sqlite',  # a database; its threads annotate codex/claude/grok sessions
}


WARNINGS = []


def detected_harnesses() -> list[str]:
    return [h for h, rel in STORES.items() if h != 't3code' and
            (shutil.which(h) or (home() / rel).exists() or
             (h == 'devin' and any(d.is_dir() for d in devin_adapter.data_dirs())))]


def probe_mcp(launch: tuple | None = None) -> tuple[bool, str]:
    from . import __version__
    from .mcp import TOOLS
    command, args, env = launch or install.mcp_launch()
    requests = [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'protocolVersion': '2025-06-18', 'capabilities': {},
            'clientInfo': {'name': 'everett-doctor', 'version': '1'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'},
    ]
    try:
        result = subprocess.run([command, *args], input=''.join(json.dumps(r) + '\n' for r in requests),
                                capture_output=True, text=True, timeout=5, env={**os.environ, **env})
        replies = {r.get('id'): r for r in map(json.loads, result.stdout.splitlines())}
        tools = replies.get(2, {}).get('result', {}).get('tools', [])
        names = {tool['name'] for tool in tools}
        if result.returncode or names != {tool['name'] for tool in TOOLS} or len(tools) != len(TOOLS):
            return False, f'MCP probe failed: expected {len(TOOLS)} tools, received {len(tools)}; run `python -m everett mcp`'
        if 'tools' not in replies.get(1, {}).get('result', {}).get('capabilities', {}):
            return False, 'MCP initialize did not advertise tools'
        if replies[1]['result'].get('serverInfo', {}).get('version') != __version__:
            return False, 'registered server runs an older Everett; refresh it with `everett install-mcp --repair --apply`'
    except (OSError, subprocess.TimeoutExpired, ValueError, KeyError, TypeError, AttributeError) as exc:
        return False, f'MCP stdio probe failed: {type(exc).__name__}; reinstall Everett and run `everett doctor`'
    return True, f'{len(tools)} tools over stdio (initialize + tools/list passed)'


def _line(ok: bool | None, text: str) -> None:
    if ok is False:
        WARNINGS.append(text)
    mark = {True: 'ok  ', False: 'warn', None: '--  '}[ok]
    print(f'[{mark}] {text}')


def run(hours: float = 72) -> int:
    from .cli import card_coverage
    if not math.isfinite(hours) or hours < 0:
        print('everett doctor: --hours must be finite and nonnegative.', file=sys.stderr)
        return 2
    problems = 0
    next_steps = []
    WARNINGS.clear()
    version_ok = sys.version_info >= (3, 10)
    problems += not version_ok
    _line(version_ok, f'python {sys.version.split()[0]} (needs 3.10+)')

    try:
        config.values()
        _line(True if config.config_path().exists() else None, f'config {config.config_path()}'
              + ('' if config.config_path().exists() else ' (none; defaults in use)'))
    except config.ConfigError as e:
        problems += 1
        _line(False, str(e))

    seen = detected_harnesses()
    print('harnesses:')
    for harness, rel in STORES.items():
        path = devin_adapter.data_dir() if harness == 'devin' else home() / rel
        found = path.exists()
        state = 'found' if found else 'not found'
        if harness == 't3code':
            extra = f'{len(t3code.threads())} thread(s) with a harness session id' if found else 'desktop app'
        else:
            extra = f'cli: {shutil.which(harness) or "not on PATH"}'
        cli = shutil.which(harness) if harness != 't3code' else None
        _line(True if cli or found else None, f'  {harness:<7} {path} ({state}); {extra}')
        if harness != 't3code' and found and not cli:
            _line(False, f'  {harness}: install its CLI and add it to PATH before headless resume')
    if not seen:
        _line(False, 'No harness CLI or session store found. Install a supported harness first.')
        next_steps.append('Install a harness, then run `everett onboard --yes`.')

    print('hooks:')
    for harness in ('claude', 'codex', 'omp', 'grok'):
        if harness not in seen:
            continue
        try:
            events = install.installed(harness)
        except (OSError, ValueError) as e:
            problems += 1
            _line(False, f'  {harness:<7} cannot read config: {e}')
            continue
        missing = [event for event, ok in events.items() if not ok]
        if missing:
            _line(False, f'  {harness:<7} missing {", ".join(missing)}; run `everett install-hooks --{harness} --apply`')
        else:
            _line(True, f'  {harness:<7} installed')

    print('mcp:')
    ok, detail = probe_mcp()
    problems += not ok
    _line(ok, f'  server: {detail}')
    mcp_harnesses = [h for h in ('claude', 'codex', 'omp', 'grok') if h in seen or install.mcp_path(h).exists()]
    missing_mcp, stale_mcp = [], []
    for harness in mcp_harnesses:
        status = install.mcp_status(harness)
        ready, detail = status.ready, status.detail
        if ready:
            entry = install._mcp_entry(harness, install._mcp_config(harness))
            ready, detail = probe_mcp((entry['command'], entry['args'], entry.get('env', {})))
        _line(ready, f'  {harness:<7} {detail}')
        if status.state == 'missing':
            missing_mcp.append(harness)
        elif not ready:
            problems += 1
            stale_mcp.append(harness)
    if missing_mcp:
        next_steps.append('everett install-mcp ' + ' '.join('--' + h for h in missing_mcp) + ' --apply')
    if stale_mcp:
        next_steps.append('everett install-mcp ' + ' '.join('--' + h for h in stale_mcp) + ' --repair --apply')
    if missing_mcp or stale_mcp:
        next_steps.append('Restart your MCP client after registering or repairing Everett, then run `everett doctor`.')

    sessions = registry.scan(hours, limit=None)
    _, totals = card_coverage(sessions)
    _line(bool(totals['total']) or None,
          f'cards (last {hours:g} h, interactive): {totals["total"]} sessions, {totals["agent"]} agent, '
          f'{totals["auto"]} auto, {totals["missing"]} missing')

    if find_api_key():
        _line(True, 'router: jev (key found; --router local also available)')
    else:
        _line(True, 'router: local BM25 (no Jev key; routing stays on this machine)')

    folder = trunk.vault_folder()
    _line(True if folder else None, f'vault: {folder}' if folder else 'vault: not configured (optional)')

    if sys.platform == 'darwin':
        scheduled = trunk_schedule.is_scheduled()
        _line(True if scheduled else None,
              f'trunk schedule: scheduled ({trunk_schedule.plist_path()})' if scheduled
              else 'trunk schedule: not scheduled (optional, macOS; `everett trunk schedule --apply`)')
    else:
        _line(None, 'trunk schedule: automatic scheduling requires macOS; run `everett trunk merge --llm none` manually')
    warnings = len(WARNINGS) - problems
    print('ready' if not WARNINGS else f'{problems} problem(s), {warnings} warning(s); setup needs attention')
    print('Next:')
    for step in next_steps or ['everett ls', 'Use `everett_whoami` in your MCP client to check caller identity.']:
        print(f'  {step}')
    return 1 if problems else 0
