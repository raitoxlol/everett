#!/usr/bin/env python3
"""Materialize machine-specific MCP paths outside the distributable source."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import sys

SOURCE = Path(__file__).resolve().parent


def install(destination, binary):
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
    shutil.copyfile(SOURCE / 'plugin.json', plugin / 'plugin.json')
    config = destination / 'runtime.json'
    config.write_text(json.dumps({'binary': str(selected)}, indent=2) + '\n')
    command = {'command': sys.executable, 'args': [str(plugin / 'scripts/launch.py'), '--config', str(config)]}
    (plugin / '.mcp.json').write_text(json.dumps({'mcpServers': {'everett_local': command}}, indent=2) + '\n')
    portable = {'$schema': 'https://agent-plugins.org/schemas/1.0.0/mcp.schema.json',
                'mcpServers': {'everett_local': {'type': 'stdio', **command}}}
    (plugin / 'mcp.json').write_text(json.dumps(portable, indent=2) + '\n')
    shutil.copyfile(SOURCE / 'README.md', plugin / 'README.md')
    market = destination / '.agents/plugins'
    market.mkdir(parents=True, exist_ok=True)
    (market / 'marketplace.json').write_text(json.dumps({'name': 'everett-direct-local', 'interface': {'displayName': 'Everett Local'}, 'plugins': [{'name': 'everett-direct', 'source': {'source': 'local', 'path': './plugins/everett-direct'}, 'policy': {'installation': 'AVAILABLE', 'authentication': 'ON_INSTALL'}, 'category': 'Developer Tools'}]}, indent=2) + '\n')
    return {'marketplace': str(destination), 'plugin': str(plugin), 'runtime': str(selected), 'version': version}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--destination', type=Path, default=Path.home() / '.local/share/everett-direct-plugin')
    args = parser.parse_args()
    print(json.dumps(install(args.destination, args.binary), indent=2))
