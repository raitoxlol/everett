"""`everett install-hooks`: print or merge the harness hook registrations.

--apply backs up each file it changes (<file>.everett-bak-<timestamp>), merges idempotently,
never duplicates an Everett entry and never removes anyone else's hooks.
"""
from __future__ import annotations

import json
import shlex
import shutil
import sys
import time
from pathlib import Path

from .session import home

HOOKS_DIR = Path(__file__).resolve().parent / 'hooks'

# harness -> [(event, script, timeout)]
HOOKS = {
    'claude': [('SessionStart', 'claude_session_start.py', 3), ('Stop', 'claude_stop.py', 3)],
    'codex': [('SessionStart', 'codex_session_start.py', 5), ('Stop', 'codex_stop.py', 3)],
    # Grok ignores SessionStart stdout, so it gets fallback cards only (no card instruction or core).
    'grok': [('Stop', 'grok_stop.py', 5)],
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


def merge(data: dict, harness: str) -> tuple[dict, list[str]]:
    """Return (merged settings, events added). Pure: does not touch the input."""
    data = json.loads(json.dumps(data))
    hooks = data.setdefault('hooks', {})
    if not isinstance(hooks, dict):
        raise ValueError('"hooks" is not an object')
    added = []
    for event, script, timeout in HOOKS[harness]:
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


def apply(harness: str) -> str:
    """Install for one harness; returns a one-line report."""
    if harness == 'omp':
        path = omp_extension_path()
        source = omp_extension_source()
        if path.exists() and path.read_text(encoding='utf-8') == source:
            return f'omp: already installed ({path})'
        saved = backup(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding='utf-8')
        return f'omp: wrote {path}' + (f' (backup {saved})' if saved else '')
    path = settings_path(harness)
    data = _load(path)
    merged, added = merge(data, harness)
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
            'omp': home() / '.omp' / 'agent' / 'mcp.json'}[harness]


def _toml_str(value: str) -> str:
    return json.dumps(value)  # a JSON string is a valid TOML basic string


def codex_block() -> str:
    command, args, env = mcp_launch()
    lines = ['[mcp_servers.everett]', f'command = {_toml_str(command)}',
             'args = [' + ', '.join(_toml_str(a) for a in args) + ']']
    if env:
        lines.append('env = { ' + ', '.join(f'{k} = {_toml_str(v)}' for k, v in env.items()) + ' }')
    return '\n'.join(lines) + '\n'


def mcp_installed(harness: str) -> bool:
    path = mcp_path(harness)
    if harness == 'codex':
        try:
            return any(line.strip() in ('[mcp_servers.everett]', '[mcp_servers."everett"]')
                       for line in path.read_text(encoding='utf-8').splitlines())
        except OSError:
            return False
    try:
        data = _load(path)
    except (OSError, ValueError):
        return False
    return isinstance(data.get('mcpServers'), dict) and 'everett' in data['mcpServers']


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
    return (f'# merge into {mcp_path("omp")} (OMP also imports Claude Code\'s servers)\n'
            f'{json.dumps({"mcpServers": {"everett": mcp_json_entry()}}, indent=2)}\n')


def apply_mcp(harness: str) -> str:
    path = mcp_path(harness)
    if mcp_installed(harness):
        return f'{harness}: everett MCP server already registered ({path})'
    if harness == 'codex':
        try:
            text = path.read_text(encoding='utf-8')
        except FileNotFoundError:
            text = ''
        saved = backup(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(path.name + '.everett-tmp')
        tmp.write_text(text + ('' if not text or text.endswith('\n') else '\n') + ('\n' if text else '')
                       + codex_block(), encoding='utf-8')
        tmp.replace(path)
    else:
        data = _load(path)
        servers = data.setdefault('mcpServers', {})
        if not isinstance(servers, dict):
            raise ValueError(f'"mcpServers" in {path} is not an object')
        servers['everett'] = mcp_json_entry()
        saved = backup(path)
        write_json(path, data)
    return f'{harness}: registered everett MCP server in {path}' + (f' (backup {saved})' if saved else '')
