#!/usr/bin/env python3
"""Check saved, nonsecret hosted MCP evidence. Never dispatch a model."""
import argparse
import json
from pathlib import Path
import re
from urllib.parse import urlsplit

BASE_TOOLS = {'everett_whoami', 'everett_identity', 'everett_send', 'everett_inbox', 'everett_card'}


def check(whoami, catalog, resource, require_native_start=False):
    errors = []
    if not isinstance(whoami, dict) or not isinstance(catalog, dict):
        return {'checks_passed': False, 'errors': ['invalid_evidence_object'], 'native_dispatch_verified': False}
    if 'structuredContent' in whoami:
        if whoami.get('isError') is True or not isinstance(whoami['structuredContent'], dict):
            return {'checks_passed': False, 'errors': ['failed_whoami'], 'native_dispatch_verified': False}
        whoami = whoami['structuredContent']
    try:
        target = urlsplit(resource)
        valid_resource = (target.scheme == 'https' and target.hostname and not target.username
                          and not target.password and not target.query and not target.fragment)
    except (ValueError, TypeError, AttributeError):
        valid_resource = False
    if not valid_resource:
        errors.append('invalid_expected_resource')
    if whoami.get('gateway_resource') != resource:
        errors.append('gateway_resource_mismatch')
    revisions = []
    for name in ('runtime', 'gateway_runtime'):
        runtime = whoami.get(name)
        if not isinstance(runtime, dict):
            errors.append(name + '_missing')
            continue
        version = runtime.get('version')
        match = re.fullmatch(r'(\d+)\.(\d+)\.(\d+)', version) if isinstance(version, str) else None
        if not match or tuple(map(int, match.groups())) < (2, 2, 1):
            errors.append(name + '_unsupported_version')
        revision = runtime.get('revision')
        if not isinstance(revision, str) or not re.fullmatch(r'[0-9a-f]{40}', revision) or runtime.get('dirty') is not False:
            errors.append(name + '_unverified_build')
        revisions.append(revision)
    if len(revisions) == 2 and revisions[0] != revisions[1]:
        errors.append('backend_gateway_revision_mismatch')
    if not isinstance(whoami.get('session_id'), str) or not whoami['session_id']:
        errors.append('logical_identity_missing')
    auth = whoami.get('gateway_auth')
    if not isinstance(auth, dict) or auth.get('mode') != 'oauth':
        errors.append('hosted_oauth_missing')
    rows = catalog.get('tools')
    if not isinstance(rows, list) or any(not isinstance(row, dict) or not isinstance(row.get('name'), str) for row in rows):
        names = set()
        errors.append('invalid_tool_catalog')
    else:
        names = {row['name'] for row in rows}
    if not BASE_TOOLS <= names:
        errors.append('base_tools_missing')
    capabilities = whoami.get('capabilities')
    granted = capabilities.get('native_session_start') if isinstance(capabilities, dict) else None
    if type(granted) is not bool:
        errors.append('native_start_capability_invalid')
    elif granted and 'everett_start' not in names:
        errors.append('native_start_client_catalog_stale')
    elif require_native_start and not granted:
        errors.append('native_start_not_granted')
    return {'checks_passed': not errors, 'errors': errors, 'native_start_granted': granted is True,
            'native_start_registered': 'everett_start' in names, 'native_dispatch_verified': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--whoami', required=True, type=Path, help='Nonsecret whoami result or its structuredContent')
    parser.add_argument('--tools', required=True, type=Path, help='Tool catalog for the same app and connection')
    parser.add_argument('--resource', required=True, help='Expected HTTPS MCP resource URL')
    parser.add_argument('--require-native-start', action='store_true')
    args = parser.parse_args()
    result = check(json.loads(args.whoami.read_text()), json.loads(args.tools.read_text()), args.resource, args.require_native_start)
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['checks_passed'] else 1)
