#!/usr/bin/env python3
"""Materialize machine-specific MCP paths outside the distributable source."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import sys

SOURCE = Path(__file__).resolve().parent


# ChatGPT desktop ignores a plugin's local stdio server when root portable
# manifests are present (verified 2026-11-10), so they are opt-in.
PORTABLE_FILES = ('plugin.json', 'mcp.json')


def install(destination, binary, portable=False):
    destination = destination.expanduser().resolve()
    binary = binary.expanduser().absolute()
    spec = importlib.util.spec_from_file_location('everett_local_launcher', SOURCE / 'scripts/launch.py')
    launcher = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(launcher)
    # Validate the requested --binary itself (not EVERETT_BIN) before touching an installed package.
    selected, version = launcher.validate(binary)
    plugin = destination / 'plugins/everett-direct'
    plugin.mkdir(parents=True, exist_ok=True)
    for name in ['.codex-plugin', 'skills', 'scripts']:
        shutil.copytree(SOURCE / name, plugin / name, dirs_exist_ok=True, ignore=shutil.ignore_patterns('__pycache__'))
    config = destination / 'runtime.json'
    config.write_text(json.dumps({'binary': str(selected)}, indent=2) + '\n')
    command = {'command': sys.executable, 'args': [str(plugin / 'scripts/launch.py'), '--config', str(config)]}
    (plugin / '.mcp.json').write_text(json.dumps({'mcpServers': {'everett_local': command}}, indent=2) + '\n')
    if portable:
        shutil.copyfile(SOURCE / 'plugin.json', plugin / 'plugin.json')
        manifest = {'$schema': 'https://agent-plugins.org/schemas/1.0.0/mcp.schema.json',
                    'mcpServers': {'everett_local': {'type': 'stdio', **command}}}
        (plugin / 'mcp.json').write_text(json.dumps(manifest, indent=2) + '\n')
    else:
        for name in PORTABLE_FILES:  # left behind by a 0.3.0 install
            (plugin / name).unlink(missing_ok=True)
    shutil.copyfile(SOURCE / 'README.md', plugin / 'README.md')
    market = destination / '.agents/plugins'
    market.mkdir(parents=True, exist_ok=True)
    (market / 'marketplace.json').write_text(json.dumps({'name': 'everett-direct-local', 'interface': {'displayName': 'Everett Local'}, 'plugins': [{'name': 'everett-direct', 'source': {'source': 'local', 'path': './plugins/everett-direct'}, 'policy': {'installation': 'AVAILABLE', 'authentication': 'ON_INSTALL'}, 'category': 'Developer Tools'}]}, indent=2) + '\n')
    return {'marketplace': str(destination), 'plugin': str(plugin), 'runtime': str(selected), 'version': version,
            'portable': portable,
            'next': 'Restart the ChatGPT or Codex app, start a new Codex-mode chat, and check that tool calls are '
                    'named mcp__everett_local__*.' + (' ChatGPT desktop ignores the local server in --portable mode.' if portable else '')}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--destination', type=Path, default=Path.home() / '.local/share/everett-direct-plugin')
    parser.add_argument('--portable', action='store_true',
                        help='Also write root plugin.json/mcp.json for other agent-plugins clients. ChatGPT desktop then ignores the local server.')
    args = parser.parse_args()
    print(json.dumps(install(args.destination, args.binary, args.portable), indent=2))
