#!/usr/bin/env python3
"""Verify an installed Everett Local package without starting its MCP server."""
import argparse
import importlib.util
import json
from pathlib import Path
import sys

PORTABLE_FILES = ('plugin.json', 'mcp.json')
REQUIRED_FILES = ('.codex-plugin/plugin.json', '.mcp.json', 'scripts/launch.py',
                  'skills/everett-local/SKILL.md', 'README.md')

REMEDIES = {
    'installed_layout': 'Run install.py --binary /absolute/path/to/everett to materialize the package.',
    'no_portable_manifests': 'ChatGPT desktop ignores the local server when root plugin.json/mcp.json exist. Reinstall without --portable.',
    'codex_manifest': 'Reinstall with install.py; the generated .codex-plugin/plugin.json is required.',
    'mcp_manifest': 'Reinstall with install.py; .mcp.json must launch scripts/launch.py with --config runtime.json.',
    'runtime': 'Rerun install.py --binary /absolute/path/to/everett, or fix EVERETT_BIN to an absolute executable Everett 2.2.1+ path.',
    'marketplace': 'Register the generated marketplace: codex plugin marketplace add ~/.local/share/everett-direct-plugin --json, then reinstall the plugin and restart ChatGPT.',
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


def check(plugin):
    destination = plugin.parent.parent
    rows = []

    def add(name, ok, detail):
        row = {'name': name, 'ok': bool(ok), 'detail': detail}
        if not ok:
            row['remedy'] = REMEDIES[name]
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
    command = ((servers or {}).get('mcpServers') or {}).get('everett_local') if isinstance(servers, dict) else None
    args = command.get('args') if isinstance(command, dict) else None
    launcher_arg = args[0] if isinstance(args, list) and args and isinstance(args[0], str) else ''
    config_arg = args[args.index('--config') + 1] if isinstance(args, list) and '--config' in args and args.index('--config') + 1 < len(args) else ''
    program = command.get('command') if isinstance(command, dict) else None
    valid = (isinstance(command, dict) and isinstance(program, str) and Path(program).is_file()
             and Path(launcher_arg).name == 'launch.py' and Path(launcher_arg).is_file()
             and bool(config_arg) and Path(config_arg).is_file()
             and Path(config_arg).resolve() == (destination / 'runtime.json').resolve())
    add('mcp_manifest', valid,
        'everett_local launches scripts/launch.py --config runtime.json' if valid
        else f'missing or inconsistent server entry: {error or command}')

    try:
        binary, version = load_launcher(plugin).runtime(destination / 'runtime.json')
        add('runtime', True, f'{version} at {binary}')
    except (OSError, ValueError, KeyError) as failure:
        add('runtime', False, str(failure))

    market, error = read_json(destination / '.agents' / 'plugins' / 'marketplace.json')
    plugins = market.get('plugins') if isinstance(market, dict) else None
    registered = (isinstance(plugins, list) and any(
        isinstance(entry, dict) and (entry.get('source') or {}).get('path') == './plugins/everett-direct'
        for entry in plugins))
    add('marketplace', registered,
        'marketplace registers ./plugins/everett-direct' if registered
        else f'marketplace missing or does not register the plugin: {error or market}')

    return {'ready': all(row['ok'] for row in rows), 'plugin': str(plugin),
            'checks': rows,
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
