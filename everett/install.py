"""`everett install-hooks`: print or merge the harness hook registrations.

--apply backs up each file it changes (<file>.everett-bak-<timestamp>), merges idempotently,
never duplicates an Everett entry and never removes anyone else's hooks.
"""
from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from .session import home

HOOKS_DIR = Path(__file__).resolve().parent / 'hooks'

# harness -> [(event, script, timeout)]
# UserPromptSubmit + PostToolUse deliver a live session's Everett inbox (next turn / mid-task).
HOOKS = {
    'claude': [('SessionStart', 'claude_session_start.py', 3), ('Stop', 'claude_stop.py', 3),
               ('UserPromptSubmit', 'claude_inbox.py', 3), ('PostToolUse', 'claude_inbox.py', 3)],
    'codex': [('SessionStart', 'codex_session_start.py', 5), ('Stop', 'codex_stop.py', 3),
              ('UserPromptSubmit', 'codex_inbox.py', 3), ('PostToolUse', 'codex_inbox.py', 3)],
    # Grok ignores SessionStart stdout and discards an allowing UserPromptSubmit hook's context,
    # so it gets fallback cards (Stop) and mid-task delivery (PostToolUse) only.
    'grok': [('Stop', 'grok_stop.py', 5), ('PostToolUse', 'grok_inbox.py', 3)],
}
OMP_EXTENSION = 'omp_session_start.mjs'


def hook_command(script: str) -> str:
    return f'{shlex.quote(sys.executable)} {shlex.quote(str(HOOKS_DIR / script))}'


def settings_path(harness: str) -> Path:
    return home() / {'claude': '.claude/settings.json', 'codex': '.codex/hooks.json',
                     'grok': '.grok/hooks/everett.json'}[harness]


def omp_extension_path() -> Path:
    return home() / '.omp' / 'agent' / 'extensions' / 'everett.ts'


def omp_extension_source() -> str:
    target = (HOOKS_DIR / OMP_EXTENSION).as_uri()
    return f'// Everett session cards + shared core (installed by `everett install-hooks --omp`).\nexport {{ default }} from {json.dumps(target)};\n'


def _has_script(entries: list, script: str) -> bool:
    for entry in entries if isinstance(entries, list) else []:
        for hook in (entry or {}).get('hooks', []) if isinstance(entry, dict) else []:
            command = hook.get('command', '') if isinstance(hook, dict) else ''
            if script in command and 'everett' in command:
                return True
    return False


def installed(harness: str, data: dict | None = None) -> dict[str, bool]:
    """Which of Everett's hook events are registered for a harness."""
    if harness == 'omp':
        return {'extension': omp_extension_path().exists()}
    if data is None:
        data = _load(settings_path(harness))
    hooks = data.get('hooks') if isinstance(data.get('hooks'), dict) else {}
    return {event: _has_script(hooks.get(event, []), script) for event, script, _ in HOOKS[harness]}


def merge(data: dict, harness: str, events: list[str] | None = None) -> tuple[dict, list[str]]:
    """Return (merged settings, events added). Pure: does not touch the input.

    `events`, when given, restricts the merge to that subset of the harness's hook events
    (used by `everett onboard` to apply only the hooks the user toggled on).
    """
    data = json.loads(json.dumps(data))
    hooks = data.setdefault('hooks', {})
    if not isinstance(hooks, dict):
        raise ValueError('"hooks" is not an object')
    added = []
    for event, script, timeout in HOOKS[harness]:
        if events is not None and event not in events:
            continue
        entries = hooks.setdefault(event, [])
        if not isinstance(entries, list):
            raise ValueError(f'"hooks.{event}" is not a list')
        if _has_script(entries, script):
            continue
        entries.append({'hooks': [{'type': 'command', 'command': hook_command(script), 'timeout': timeout}]})
        added.append(event)
    return data, added


def _load(path: Path) -> dict:
    try:
        text = path.read_text(encoding='utf-8')
    except FileNotFoundError:
        return {}
    data = json.loads(text) if text.strip() else {}
    if not isinstance(data, dict):
        raise ValueError(f'{path} is not a JSON object')
    return data


def backup(path: Path) -> Path | None:
    if not path.exists():
        return None
    dest = path.with_name(f'{path.name}.everett-bak-{time.strftime("%Y%m%d-%H%M%S")}')
    n = 1
    while dest.exists():
        dest = path.with_name(f'{path.name}.everett-bak-{time.strftime("%Y%m%d-%H%M%S")}-{n}')
        n += 1
    shutil.copy2(path, dest)
    return dest


def write_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + '.everett-tmp')
    tmp.write_text(json.dumps(data, indent=2) + '\n', encoding='utf-8')
    tmp.replace(path)


def snippet(harness: str) -> str:
    if harness == 'omp':
        return (f'# OMP auto-loads extensions from {omp_extension_path().parent}/\n'
                f'# write {omp_extension_path()} with:\n{omp_extension_source()}'
                f'# or pass it per launch:\nomp --hook={HOOKS_DIR / OMP_EXTENSION}\n')
    hooks: dict = {}
    for event, script, timeout in HOOKS[harness]:
        hooks[event] = [{'hooks': [{'type': 'command', 'command': hook_command(script), 'timeout': timeout}]}]
    note = {'claude': '# merge into ~/.claude/settings.json (keep your existing hooks)',
            'codex': '# merge into ~/.codex/hooks.json; Codex needs `hooks = true` in ~/.codex/config.toml\n'
                     '# and asks you to trust new hooks on first run',
            'grok': '# write ~/.grok/hooks/everett.json (Grok loads every file in ~/.grok/hooks/)'}[harness]
    return f'{note}\n{json.dumps({"hooks": hooks}, indent=2)}\n'


def apply(harness: str, events: list[str] | None = None) -> str:
    """Install for one harness; returns a one-line report. `events` restricts which hook events
    are merged (see `merge`); omitted or None installs every event Everett knows for the harness."""
    if harness == 'omp':
        path = omp_extension_path()
        source = omp_extension_source()
        if path.exists() and path.read_text(encoding='utf-8') == source:
            return f'omp: already installed ({path})'
        saved = backup(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding='utf-8')
        return f'omp: wrote {path}' + (f' (backup {saved})' if saved else '')
    if events is not None and not events:
        return f'{harness}: skipped (no hooks selected)'
    path = settings_path(harness)
    data = _load(path)
    merged, added = merge(data, harness, events=events)
    if not added:
        return f'{harness}: already installed ({path})'
    saved = backup(path)
    write_json(path, merged)
    extra = ' (backup ' + str(saved) + ')' if saved else ''
    return f'{harness}: added {", ".join(added)} to {path}{extra}'


# ---- MCP registration (`everett install-mcp`) ----------------------------------------------

def mcp_launch() -> tuple[str, list[str], dict]:
    """(command, args, env) that starts `everett mcp` from this very install."""
    package_parent = Path(__file__).resolve().parent.parent
    env = {} if 'site-packages' in package_parent.parts else {'PYTHONPATH': str(package_parent)}
    return sys.executable, ['-m', 'everett', 'mcp'], env


def mcp_json_entry() -> dict:
    command, args, env = mcp_launch()
    entry = {'type': 'stdio', 'command': command, 'args': args}
    if env:
        entry['env'] = env
    return entry


def mcp_path(harness: str) -> Path:
    return {'claude': home() / '.claude.json', 'codex': home() / '.codex' / 'config.toml',
            'grok': home() / '.grok' / 'config.toml', 'omp': home() / '.omp' / 'agent' / 'mcp.json'}[harness]

TOML_MCP = ('codex', 'grok')  # both use an [mcp_servers.everett] table in a config.toml


def _toml_str(value: str) -> str:
    return json.dumps(value)  # a JSON string is a valid TOML basic string


def codex_block() -> str:
    command, args, env = mcp_launch()
    lines = ['[mcp_servers.everett]', f'command = {_toml_str(command)}',
             'args = [' + ', '.join(_toml_str(a) for a in args) + ']']
    if env:
        lines.append('env = { ' + ', '.join(f'{k} = {_toml_str(v)}' for k, v in env.items()) + ' }')
    return '\n'.join(lines) + '\n'


def _mcp_config(harness: str) -> dict:
    path = mcp_path(harness)
    if harness in TOML_MCP:
        try:
            import tomllib
        except ImportError:
            import tomli as tomllib
        try:
            text = path.read_text(encoding='utf-8')
        except FileNotFoundError:
            return {}
        return tomllib.loads(text)
    return _load(path)


def _mcp_entry(harness: str, data: dict) -> dict | None:
    key = 'mcp_servers' if harness in TOML_MCP else 'mcpServers'
    servers = data.get(key, {})
    if not isinstance(servers, dict):
        raise ValueError(f'{key} must be an object')
    return servers.get('everett')


def mcp_installed(harness: str) -> bool:
    try:
        return _mcp_entry(harness, _mcp_config(harness)) is not None
    except (OSError, ValueError):
        return False


@dataclass(frozen=True)
class MCPStatus:
    state: str
    detail: str

    @property
    def ready(self) -> bool:
        return self.state == 'ready'


def mcp_status(harness: str) -> MCPStatus:
    try:
        entry = _mcp_entry(harness, _mcp_config(harness))
    except (OSError, ValueError) as exc:
        return MCPStatus('invalid', f'cannot read {mcp_path(harness)}: {exc}')
    if entry is None:
        return MCPStatus('missing', 'not registered')
    if not isinstance(entry, dict):
        return MCPStatus('invalid', 'Everett MCP entry must be an object')
    if entry.get('disabled') or entry.get('enabled') is False:
        return MCPStatus('stale', 'registration is disabled')
    command, args = entry.get('command'), entry.get('args', [])
    if not isinstance(command, str) or not command:
        return MCPStatus('stale', 'registration has no executable')
    if not (os.path.isfile(command) and os.access(command, os.X_OK)) and not shutil.which(command):
        return MCPStatus('stale', 'registered executable is missing or not on PATH')
    if args != ['-m', 'everett', 'mcp'] and not (Path(command).name == 'everett' and args == ['mcp']):
        return MCPStatus('stale', 'registration does not launch Everett stdio MCP')
    env = entry.get('env', {})
    if not isinstance(env, dict):
        return MCPStatus('invalid', 'registration env must be an object')
    if any(not isinstance(v, str) for v in env.values()):
        return MCPStatus('invalid', 'registration env values must be strings')
    source = env.get('PYTHONPATH', '')
    if source and not any((Path(p) / 'everett/__init__.py').is_file() for p in str(source).split(os.pathsep)):
        return MCPStatus('stale', 'registered source checkout is missing')
    return MCPStatus('ready', 'Everett stdio MCP registered')


def _repaired_entry(old: dict) -> dict:
    entry = {**old, **mcp_json_entry()}
    env = old.get('env', {})
    if not isinstance(env, dict):
        raise ValueError('registration env must be an object')
    env = {k: v for k, v in env.items() if k != 'PYTHONPATH'} | mcp_launch()[2]
    if env:
        entry['env'] = env
    else:
        entry.pop('env', None)
    entry.pop('url', None)
    if 'disabled' in entry:
        entry['disabled'] = False
    if 'enabled' in entry:
        entry['enabled'] = True
    return entry


def _toml_value(value) -> str:
    if isinstance(value, str):
        return _toml_str(value)
    if isinstance(value, bool):
        return str(value).lower()
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, list):
        return '[' + ', '.join(_toml_value(v) for v in value) + ']'
    if isinstance(value, dict):
        return '{ ' + ', '.join(f'{_toml_str(k)} = {_toml_value(v)}' for k, v in value.items()) + ' }'
    raise ValueError('unsupported value in Everett MCP entry; edit the entry manually')


def _replace_toml_entry(text: str, entry: dict) -> str:
    lines, removing, found = [], False, False
    for line in text.splitlines(keepends=True):
        if line.lstrip().startswith('['):
            match = re.match(r'\s*\[([^\]]+)\]\s*(?:#.*)?$', line)
            table = match[1].replace('"', '').replace("'", '').replace(' ', '') if match else ''
            removing = table == 'mcp_servers.everett' or table.startswith('mcp_servers.everett.')
            found |= removing
        if not removing:
            lines.append(line)
    if not found:
        raise ValueError('Everett uses an inline MCP table; edit it manually before retrying --repair --apply')
    block = '[mcp_servers.everett]\n' + ''.join(f'{_toml_str(k)} = {_toml_value(v)}\n' for k, v in entry.items())
    return ''.join(lines).rstrip() + '\n\n' + block


def mcp_snippet(harness: str) -> str:
    command, args, env = mcp_launch()
    if harness == 'claude':
        env_flags = ''.join(f' -e {k}={shlex.quote(v)}' for k, v in env.items())
        return (f'# user scope, all projects:\nclaude mcp add --scope user{env_flags} everett -- '
                f'{shlex.quote(command)} {" ".join(args)}\n'
                f'# (or add under "mcpServers" in ~/.claude.json)\n'
                f'{json.dumps({"mcpServers": {"everett": mcp_json_entry()}}, indent=2)}\n')
    if harness == 'codex':
        return f'# append to ~/.codex/config.toml\n{codex_block()}'
    if harness == 'grok':
        env_flags = ''.join(f' -e {k}={shlex.quote(v)}' for k, v in env.items())
        return (f'# user scope (~/.grok/config.toml):\ngrok mcp add{env_flags} everett {shlex.quote(command)} -- '
                f'{" ".join(args)}\n# (or append to ~/.grok/config.toml; Grok also imports Claude Code\'s servers)\n'
                f'{codex_block()}')
    return (f'# merge into {mcp_path("omp")} (OMP also imports Claude Code\'s servers)\n'
            f'{json.dumps({"mcpServers": {"everett": mcp_json_entry()}}, indent=2)}\n')


def apply_mcp(harness: str, repair: bool = False) -> str:
    path = mcp_path(harness)
    data = _mcp_config(harness)
    old = _mcp_entry(harness, data)
    if old is not None and not repair:
        status = mcp_status(harness)
        if not status.ready:
            raise ValueError(f'{status.detail}; run `everett install-mcp --{harness} --repair --apply`')
        return f'{harness}: everett MCP server already registered ({path})'
    if old is not None and not isinstance(old, dict):
        raise ValueError('Everett MCP entry must be an object; correct it before retrying')
    entry = _repaired_entry(old or {})
    if harness in TOML_MCP:
        try:
            text = path.read_text(encoding='utf-8')
        except FileNotFoundError:
            text = ''
        if old is not None:
            updated = _replace_toml_entry(text, entry)
        else:
            updated = text + ('' if not text or text.endswith('\n') else '\n') + ('\n' if text else '') + codex_block()
        if updated == text:
            return f'{harness}: everett MCP server already registered ({path})'
        saved = backup(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(path.name + '.everett-tmp')
        tmp.write_text(updated, encoding='utf-8')
        tmp.replace(path)
    else:
        servers = data.setdefault('mcpServers', {})
        if not isinstance(servers, dict):
            raise ValueError(f'"mcpServers" in {path} is not an object')
        if old == entry:
            return f'{harness}: everett MCP server already registered ({path})'
        servers['everett'] = entry
        saved = backup(path)
        write_json(path, data)
    return f'{harness}: registered everett MCP server in {path}' + (f' (backup {saved})' if saved else '')
