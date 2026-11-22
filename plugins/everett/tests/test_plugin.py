import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / (name + '.py'))
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


builder, readiness = module('build'), module('check')
RESOURCE = 'https://example.test/mcp/owner'
APP = 'asdk_app_' + 'a' * 32


class HostedReadiness(unittest.TestCase):
    def setUp(self):
        runtime = {'version': '2.2.1', 'revision': 'b' * 40, 'dirty': False}
        self.whoami = {'runtime': runtime, 'gateway_runtime': copy.deepcopy(runtime),
                       'gateway_resource': RESOURCE, 'gateway_auth': {'mode': 'oauth'},
                       'session_id': 'chat-disposable', 'capabilities': {'native_session_start': True}}
        self.catalog = {'tools': [{'name': name} for name in readiness.BASE_TOOLS | {'everett_start'}]}

    def run_check(self, **kwargs):
        return readiness.check(self.whoami, self.catalog, RESOURCE, **kwargs)

    def test_registered_grant_never_claims_dispatch(self):
        result = self.run_check(require_native_start=True)
        self.assertTrue(result['checks_passed'], result)
        self.assertFalse(result['native_dispatch_verified'])
        self.assertFalse(result['destination_directory_registered'])
        self.catalog['tools'].append({'name': 'everett_directory'})
        self.assertTrue(self.run_check()['destination_directory_registered'])

    def test_live_stale_catalog_failure_and_refresh(self):
        self.catalog['tools'] = [t for t in self.catalog['tools'] if t['name'] != 'everett_start']
        self.assertIn('native_start_client_catalog_stale', self.run_check()['errors'])
        self.catalog['tools'].append({'name': 'everett_start'})
        self.assertTrue(self.run_check()['checks_passed'])

    def test_event_pickup_requires_the_registered_inbox_selectors(self):
        inbox = next(tool for tool in self.catalog['tools'] if tool['name'] == 'everett_inbox')
        with tempfile.TemporaryDirectory() as tmp:
            whoami, tools = Path(tmp) / 'whoami.json', Path(tmp) / 'tools.json'
            whoami.write_text(json.dumps(self.whoami))
            command = [sys.executable, str(ROOT / 'check.py'), '--whoami', str(whoami),
                       '--tools', str(tools), '--resource', RESOURCE]
            tools.write_text(json.dumps(self.catalog))
            default = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(default.returncode, 0, default.stderr)

            command.append('--require-event-pickup')
            old = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(old.returncode, 1, old.stderr)
            self.assertIn('event_pickup_client_catalog_stale', json.loads(old.stdout)['errors'])

            inbox['inputSchema'] = {'type': 'object', 'properties': {
                'message_id': {'type': 'string'}, 'identity_handle': {'type': 'string'},
            }}
            tools.write_text(json.dumps(self.catalog))
            refreshed = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(refreshed.returncode, 0, refreshed.stderr)
            self.assertTrue(json.loads(refreshed.stdout)['checks_passed'])

    def test_event_pickup_rejects_missing_or_malformed_selectors(self):
        inbox = next(tool for tool in self.catalog['tools'] if tool['name'] == 'everett_inbox')
        for properties in ({'identity_handle': {'type': 'string'}},
                           {'message_id': {'type': 'string'}},
                           {'message_id': {'type': 'array'}, 'identity_handle': 'string'}):
            with self.subTest(properties=properties):
                inbox['inputSchema'] = {'properties': properties}
                result = self.run_check(require_event_pickup=True)
                self.assertIn('event_pickup_client_catalog_stale', result['errors'])

    def test_inbox_only_requires_explicit_native_grant(self):
        self.whoami['capabilities']['native_session_start'] = False
        self.catalog['tools'] = [t for t in self.catalog['tools'] if t['name'] != 'everett_start']
        self.assertTrue(self.run_check()['checks_passed'])
        self.assertIn('native_start_not_granted', self.run_check(require_native_start=True)['errors'])

    def test_rejects_route_and_build_drift(self):
        self.whoami['gateway_resource'] += '/old'
        self.whoami['runtime']['dirty'] = True
        self.whoami['gateway_runtime']['revision'] = 'c' * 40
        errors = self.run_check()['errors']
        for expected in ('gateway_resource_mismatch', 'runtime_unverified_build', 'backend_gateway_revision_mismatch'):
            self.assertIn(expected, errors)

    def test_rejects_malformed_evidence(self):
        for granted in (None, 1, 'true', {}):
            with self.subTest(granted=granted):
                self.whoami['capabilities']['native_session_start'] = granted
                self.assertIn('native_start_capability_invalid', self.run_check()['errors'])
        for resource in ('https://[', 'http://example.test', 'https://user:secret@example.test', None):
            with self.subTest(resource=resource):
                self.assertIn('invalid_expected_resource', readiness.check(self.whoami, self.catalog, resource)['errors'])
        self.assertFalse(readiness.check([], self.catalog, RESOURCE)['checks_passed'])
        self.assertFalse(readiness.check({'structuredContent': self.whoami, 'isError': True}, self.catalog, RESOURCE)['checks_passed'])
        self.assertIn('invalid_tool_catalog', readiness.check(self.whoami, {'tools': [None]}, RESOURCE)['errors'])


class PluginArtifact(unittest.TestCase):
    def test_reproducible_single_app_package_with_truthful_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            first, second = Path(tmp) / 'first.zip', Path(tmp) / 'second.zip'
            result = builder.build(APP, first)
            builder.build(APP, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            self.assertEqual(result['sha256'], hashlib.sha256(first.read_bytes()).hexdigest())
            with zipfile.ZipFile(first) as archive:
                members = set(archive.namelist())
                self.assertEqual(members, {
                    '.codex-plugin/plugin.json', 'README.md', 'check.py', 'skills/everett/SKILL.md',
                    'plugin.json', '.app.json', 'provenance.json',
                })
                local = json.loads(archive.read('.codex-plugin/plugin.json'))
                portable = json.loads(archive.read('plugin.json'))
                self.assertEqual(local['version'], portable['version'])
                self.assertEqual(local['skills'], './skills')
                # Agent Plugins 1.0.0 rejects undeclared root properties;
                # skills use its conventional skills/ directory instead.
                self.assertLessEqual(set(portable), {'$schema', 'name', 'version', 'description', 'author',
                                                    'homepage', 'repository', 'license', 'keywords', 'extensions'})
                self.assertEqual(json.loads(archive.read('.app.json')), {'apps': {'everett': {'id': APP, 'required': True}}})
                provenance = json.loads(archive.read('provenance.json'))
                self.assertEqual(set(provenance['files']), members - {'provenance.json'})
                for name, expected in provenance['files'].items():
                    self.assertEqual(hashlib.sha256(archive.read(name)).hexdigest(), expected)
                self.assertEqual(result['source_revision'], provenance['source_revision'])
                self.assertEqual(result['source_dirty'], provenance['source_dirty'])
            before = first.read_bytes()
            with self.assertRaises(FileExistsError):
                builder.build(APP, first)
            self.assertEqual(before, first.read_bytes())

    def test_rejects_non_app_references_and_existing_symlinks(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / 'target'
            target.write_text('preserve')
            output = Path(tmp) / 'output.zip'
            for bad in ('token', 'link_' + 'a' * 32, APP + '/path'):
                with self.assertRaises(ValueError):
                    builder.build(bad, output)
            self.assertFalse(output.exists())
            output.symlink_to(target)
            with self.assertRaises(FileExistsError):
                builder.build(APP, output)
            self.assertEqual(target.read_text(), 'preserve')


if __name__ == '__main__':
    unittest.main()
