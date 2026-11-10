#!/usr/bin/env python3
"""Read-only MCP check using the exact command in an installed manifest."""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--manifest', required=True, type=Path)
    args = parser.parse_args()
    server = json.loads(args.manifest.read_text())['mcpServers']['everett_local']
    requests = [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': '2025-06-18', 'capabilities': {}, 'clientInfo': {'name': 'everett-local-verification', 'version': '0.3.0'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'},
        {'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {'name': 'everett_whoami', 'arguments': {}}},
        {'jsonrpc': '2.0', 'id': 4, 'method': 'tools/call', 'params': {'name': 'everett_ls', 'arguments': {'hours': 1}}},
    ]
    result = subprocess.run([server['command'], *server['args']], input=''.join(json.dumps(r) + '\n' for r in requests), capture_output=True, text=True, timeout=40)
    if result.returncode:
        raise RuntimeError('Installed MCP command failed: ' + result.stderr[:500])
    rows = {r['id']: r for line in result.stdout.splitlines() if (r := json.loads(line)).get('id') is not None}
    for request_id in range(1, 5):
        if request_id not in rows or 'error' in rows[request_id] or rows[request_id]['result'].get('isError'):
            raise RuntimeError(f'MCP request {request_id} failed; inspect the local runtime and session binding')
    names = {t['name'] for t in rows[2]['result']['tools']}
    missing = {'everett_whoami', 'everett_ls', 'everett_send', 'everett_inbox'} - names
    if missing:
        raise RuntimeError(f'missing advertised tools: {sorted(missing)}')
    whoami = rows[3]['result']
    identity = whoami.get('structuredContent') or json.loads(whoami['content'][0]['text'])
    session_id = identity.get('session_id') or ''
    if not session_id:
        raise RuntimeError('everett_whoami returned no session_id; the caller session is not bound')
    print(json.dumps({'success': True, 'installed_command': True, 'advertised_tools': len(names), 'identity_call': True, 'session_id': session_id, 'local_listing_call': True, 'messages_sent': 0, 'gui_client_verified': False}, indent=2))


if __name__ == '__main__':
    main()
