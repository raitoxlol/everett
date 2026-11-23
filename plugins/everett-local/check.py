#!/usr/bin/env python3
"""Verify an installed Everett Local package without starting its MCP server."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

try:  # Python 3.11+
    import tomllib
except ImportError:  # Python 3.9/3.10: fall back to a section scan
    tomllib = None

PORTABLE_FILES = ('plugin.json', 'mcp.json')
REQUIRED_FILES = ('.codex-plugin/plugin.json', '.mcp.json', 'scripts/launch.py',
                  'skills/everett-local/SKILL.md', 'README.md')
MARKETPLACE = 'everett-direct-local'
PLUGIN_ID = 'everett-direct@' + MARKETPLACE

REINSTALL = ('From the Everett Local source directory, run python3 install.py --binary /absolute/path/to/everett '
             '--destination "{destination}"')
REGISTER = 'codex plugin marketplace add "{destination}" --json'
ADD = f'codex plugin add {PLUGIN_ID} --json'

REMEDIES = {
    'installed_layout': REINSTALL + ' to materialize the package.',
    'no_portable_manifests': 'ChatGPT desktop ignores the local server when root plugin.json/mcp.json exist. '
                             + REINSTALL + ' without --portable.',
    'codex_manifest': REINSTALL + '; the generated .codex-plugin/plugin.json is required.',
    'mcp_manifest': REINSTALL + '; .mcp.json must run an executable Python with this plugin\'s scripts/launch.py '
                    'and --config runtime.json.',
    'runtime': REINSTALL + ', or fix EVERETT_BIN to an absolute executable Everett 2.2.1+ path.',
    'marketplace': REINSTALL + ' to regenerate .agents/plugins/marketplace.json, then run '
                   + REGISTER + ' and ' + ADD + ', and restart ChatGPT.',
    'codex_marketplace': 'Register the generated marketplace: ' + REGISTER + ' (if it points elsewhere, first run '
                         f'codex plugin marketplace remove {MARKETPLACE}), then ' + ADD + ' and restart ChatGPT.',
    'codex_plugin': 'Install and enable the plugin: ' + ADD + ', then restart ChatGPT.',
}


def load_launcher(plugin):
    spec = importlib.util.spec_from_file_location('everett_local_launcher', plugin / 'scripts' / 'launch.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def read_json(path):
    try:
        return json.loads(path.read_text()), None
    except (OSError, ValueError) as error:
        return None, str(error)


def same_path(value, expected):
    """True when a manifest string names the expected existing file."""
    if not isinstance(value, str) or not value:
        return False
    try:
        return Path(value).is_file() and Path(value).resolve() == expected.resolve()
    except (OSError, ValueError):
        return False


def same_dir(value, expected):
    if not isinstance(value, str) or not value:
        return False
    try:
        return Path(value).expanduser().resolve() == expected.resolve()
    except (OSError, ValueError):
        return False


def executable(value):
    if not isinstance(value, str) or not value:
        return False
    try:
        return Path(value).is_file() and os.access(value, os.X_OK)
    except (OSError, ValueError):
        return False


def codex_home():
    return Path(os.environ.get('CODEX_HOME') or Path.home() / '.codex').expanduser()


_HEADER = re.compile(r'^\s*\[\s*([^\[\]]+?)\s*\]\s*(?:#.*)?$')
_PAIR = re.compile(r'^\s*([A-Za-z0-9_-]+)\s*=\s*(.+?)\s*$')


def _scan_toml(text):
    """Minimal reader for the marketplaces/plugins tables on Python < 3.11."""
    tables = {'marketplaces': {}, 'plugins': {}}
    current = None
    for line in text.splitlines():
        if line.lstrip().startswith('['):  # any header, including [[array.tables]], ends the table
            current = None
            header = _HEADER.match(line)
            if not header or line.lstrip().startswith('[['):
                continue
            root, dot, name = header.group(1).partition('.')
            if dot and root.strip() in tables:
                name = name.strip()
                if len(name) >= 2 and name[0] == name[-1] and name[0] in '"\'':
                    name = name[1:-1]
                current = tables[root.strip()].setdefault(name, {})
            continue
        pair = _PAIR.match(line) if current is not None else None
        if pair:
            raw = pair.group(2)
            if raw[:1] in '"\'':
                end = raw.find(raw[0], 1)
                current[pair.group(1)] = raw[1:end] if end > 0 else raw[1:]
            elif raw.split('#')[0].strip() in ('true', 'false'):
                current[pair.group(1)] = raw.split('#')[0].strip() == 'true'
    return tables


def codex_tables(config):
    """Return ({'marketplaces': {...}, 'plugins': {...}}, error)."""
    try:
        text = config.read_text()
    except OSError as error:
        return None, str(error)
    if tomllib is None:
        return _scan_toml(text), None
    try:
        data = tomllib.loads(text)
    except ValueError as error:
        return None, f'{config}: {error}'
    return {key: data.get(key) if isinstance(data.get(key), dict) else {}
            for key in ('marketplaces', 'plugins')}, None


def check(plugin):
    destination = plugin.parent.parent
    rows = []
    warnings = []

    def add(name, ok, detail):
        row = {'name': name, 'ok': bool(ok), 'detail': detail}
        if not ok:
            row['remedy'] = REMEDIES[name].format(destination=destination)
        rows.append(row)

    missing = [name for name in REQUIRED_FILES if not (plugin / name).is_file()]
    add('installed_layout', not missing, 'all files present' if not missing else 'missing: ' + ', '.join(missing))

    portable = [name for name in PORTABLE_FILES if (plugin / name).exists()]
    add('no_portable_manifests', not portable,
        'no root plugin.json/mcp.json' if not portable else 'present: ' + ', '.join(portable))

    manifest, error = read_json(plugin / '.codex-plugin' / 'plugin.json')
    valid = (isinstance(manifest, dict) and manifest.get('name') == 'everett-direct'
             and manifest.get('mcpServers') == './.mcp.json')
    add('codex_manifest', valid, 'name everett-direct with mcpServers ./.mcp.json' if valid else f'invalid or unreadable: {error or manifest}')

    servers, error = read_json(plugin / '.mcp.json')
    table = servers.get('mcpServers') if isinstance(servers, dict) else None
    command = table.get('everett_local') if isinstance(table, dict) else None
    args = command.get('args') if isinstance(command, dict) else None
    args = args if isinstance(args, list) else []
    launcher_arg = args[0] if args else None
    config_arg = None
    if '--config' in args and args.index('--config') + 1 < len(args):
        config_arg = args[args.index('--config') + 1]
    program = command.get('command') if isinstance(command, dict) else None
    if error:
        problems = [f'unreadable: {error}']
    elif not isinstance(command, dict):
        problems = ['no mcpServers.everett_local object']
    else:
        problems = []
        if not executable(program):
            problems.append(f'command is not an executable file: {program!r}')
        if not same_path(launcher_arg, plugin / 'scripts' / 'launch.py'):
            problems.append(f'first argument is not this plugin\'s scripts/launch.py: {launcher_arg!r}')
        if not same_path(config_arg, destination / 'runtime.json'):
            problems.append(f'--config is not {destination / "runtime.json"}: {config_arg!r}')
    add('mcp_manifest', not problems,
        'everett_local launches scripts/launch.py --config runtime.json' if not problems else '; '.join(problems))

    try:
        binary, version = load_launcher(plugin).runtime(destination / 'runtime.json')
        add('runtime', True, f'{version} at {binary}')
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as failure:
        add('runtime', False, str(failure))

    market, error = read_json(destination / '.agents' / 'plugins' / 'marketplace.json')
    plugins = market.get('plugins') if isinstance(market, dict) else None
    listed = (isinstance(plugins, list) and any(
        isinstance(entry, dict) and isinstance(entry.get('source'), dict)
        and entry['source'].get('path') == './plugins/everett-direct'
        for entry in plugins))
    add('marketplace', listed,
        'generated marketplace.json lists ./plugins/everett-direct' if listed
        else f'generated marketplace.json missing or does not list the plugin: {error or market}')

    config = codex_home() / 'config.toml'
    tables, error = codex_tables(config)
    entry = (tables or {}).get('marketplaces', {}).get(MARKETPLACE)
    source = entry.get('source') if isinstance(entry, dict) else None
    registered = same_dir(source, destination)
    add('codex_marketplace', registered,
        f'{config} registers marketplace {MARKETPLACE} at {destination}' if registered
        else (f'cannot read {config}: {error}' if error
              else f'[marketplaces.{MARKETPLACE}] in {config} is missing or has source {source!r}, not {destination}'))

    entry = (tables or {}).get('plugins', {}).get(PLUGIN_ID)
    enabled = isinstance(entry, dict) and entry.get('enabled', True) is not False
    add('codex_plugin', enabled,
        f'{config} installs {PLUGIN_ID} (enabled)' if enabled
        else (f'cannot read {config}: {error}' if error
              else f'[plugins."{PLUGIN_ID}"] in {config} is '
                   + ('disabled' if isinstance(entry, dict) else 'missing')))

    cache = codex_home() / 'plugins' / 'cache' / MARKETPLACE / 'everett-direct'
    stale = sorted(str(path) for path in cache.glob('*/plugin.json')) if cache.is_dir() else []
    if stale:
        warnings.append({'name': 'cached_portable_manifest', 'detail': 'present: ' + ', '.join(stale),
                         'remedy': 'ChatGPT desktop ignores the local server when the cached copy has a root '
                                   'plugin.json. Rerun ' + ADD + ' after reinstalling without --portable, '
                                   'then restart ChatGPT.'})

    return {'ready': all(row['ok'] for row in rows), 'plugin': str(plugin),
            'checks': rows, 'warnings': warnings,
            'client_steps': ['Quit and reopen ChatGPT after install or reinstall; it caches plugins.',
                             'Start a new chat in Codex mode; regular ChatGPT mode cannot run local plugins.',
                             'Confirm tool calls are named mcp__everett_local__*, not mcp__everett__*.']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plugin', type=Path, default=Path(__file__).resolve().parent,
                        help='Installed plugin directory (default: the directory containing check.py)')
    args = parser.parse_args()
    result = check(args.plugin.expanduser().resolve())
    print(json.dumps(result, indent=2))
    return 0 if result['ready'] else 1


if __name__ == '__main__':
    sys.exit(main())
