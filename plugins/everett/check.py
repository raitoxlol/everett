#!/usr/bin/env python3
"""Check saved, nonsecret hosted MCP evidence. Never dispatch a model."""
import argparse
import json
from pathlib import Path
import re
from urllib.parse import urlsplit

BASE_TOOLS = {'everett_whoami', 'everett_identity', 'everett_send', 'everett_inbox', 'everett_card'}


def check(whoami, catalog, resource, require_native_start=False, *, require_event_pickup=False):
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
        rows = []
        errors.append('invalid_tool_catalog')
    else:
        names = {row['name'] for row in rows}
        if require_event_pickup and 'everett_inbox' in names:
            schema = next(row.get('inputSchema') for row in rows if row['name'] == 'everett_inbox')
            properties = schema.get('properties') if isinstance(schema, dict) else None
            if not isinstance(properties, dict) or any(
                not isinstance(properties.get(key), dict) or properties[key].get('type') != 'string'
                for key in ('message_id', 'identity_handle')
            ):
                errors.append('event_pickup_client_catalog_stale')
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
    scope = whoami.get('native_start_scope')
    if scope is not None:
        projects = scope.get('projects') if isinstance(scope, dict) else None
        if not isinstance(projects, dict) or 'default' not in projects:
            errors.append('native_start_scope_invalid')
        else:
            start = next((row for row in rows if row['name'] == 'everett_start'), {})
            schema = start.get('inputSchema', {})
            props = schema.get('properties', {}) if isinstance(schema, dict) else {}
            selector = props.get('project', {}) if isinstance(props, dict) else {}
            if len(projects) > 1 and (not isinstance(selector, dict)
                    or selector.get('type') != 'string'
                    or not isinstance(selector.get('enum'), list)
                    or any(not isinstance(alias, str) for alias in selector['enum'])
                    or set(selector['enum']) != set(projects)):
                errors.append('native_project_client_catalog_stale')
            status = next((row for row in rows if row['name'] == 'everett_start_status'), {})
            schema = status.get('inputSchema', {})
            props = schema.get('properties', {}) if isinstance(schema, dict) else {}
            if not isinstance(props, dict) or any(
                    not isinstance(props.get(key), dict) or props[key].get('type') != 'string'
                    for key in ('request_id', 'identity_handle')):
                errors.append('native_status_client_catalog_stale')
    return {'checks_passed': not errors, 'errors': errors, 'native_start_granted': granted is True,
            'native_start_registered': 'everett_start' in names,
            'destination_directory_registered': 'everett_directory' in names,
            'native_dispatch_verified': False}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--whoami', required=True, type=Path, help='Nonsecret whoami result or its structuredContent')
    parser.add_argument('--tools', required=True, type=Path, help='Tool catalog for the same app and connection')
    parser.add_argument('--resource', required=True, help='Expected HTTPS MCP resource URL')
    parser.add_argument('--require-native-start', action='store_true')
    parser.add_argument('--require-event-pickup', action='store_true',
                        help='Require message_id and identity_handle in the registered everett_inbox schema')
    args = parser.parse_args()
    result = check(json.loads(args.whoami.read_text()), json.loads(args.tools.read_text()), args.resource,
                   args.require_native_start, require_event_pickup=args.require_event_pickup)
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['checks_passed'] else 1)
