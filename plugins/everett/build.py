#!/usr/bin/env python3
"""Build one reproducible hosted-app plugin without installing or authenticating."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import zipfile

SOURCE = Path(__file__).resolve().parent
FILES = ('.codex-plugin/plugin.json', 'README.md', 'check.py', 'skills/everett/SKILL.md')


def encoded(value):
    return (json.dumps(value, indent=2, sort_keys=True) + '\n').encode()


def build(app_id, output):
    if not re.fullmatch(r'asdk_app_[0-9a-f]{32}', app_id):
        raise ValueError('app-id must identify one existing hosted App SDK app')
    contents = {name: (SOURCE / name).read_bytes() for name in FILES}
    manifest = json.loads(contents['.codex-plugin/plugin.json'])
    if not re.fullmatch(r'\d+\.\d+\.\d+', manifest['version']):
        raise ValueError('plugin version must be a release version')
    # Both supported manifest layouts derive from one versioned source.
    portable = {key: manifest[key] for key in ('name', 'version', 'description', 'author', 'repository', 'license')}
    portable['$schema'] = 'https://agent-plugins.org/schemas/1.0.0/plugin.schema.json'
    portable['extensions'] = {'com.openai': {'interface': manifest['interface'], 'apps': './.app.json'}}
    contents['plugin.json'] = encoded(portable)
    contents['.app.json'] = encoded({'apps': {'everett': {'id': app_id, 'required': True}}})
    revision, dirty = None, True
    try:
        revision = subprocess.check_output(['git', '-C', str(SOURCE), 'rev-parse', 'HEAD'], stderr=subprocess.DEVNULL, text=True).strip()
        dirty = bool(subprocess.check_output(['git', '-C', str(SOURCE), 'status', '--porcelain', '--', '.'], stderr=subprocess.DEVNULL, text=True).strip())
    except (OSError, subprocess.CalledProcessError):
        pass
    provenance = {'plugin_version': manifest['version'], 'source_revision': revision, 'source_dirty': dirty,
                  'files': {name: hashlib.sha256(raw).hexdigest() for name, raw in sorted(contents.items())}}
    contents['provenance.json'] = encoded(provenance)
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation also refuses an existing symlink. Never replace a package.
    with output.open('xb') as stream:
        with zipfile.ZipFile(stream, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name, raw in sorted(contents.items()):
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o100644 << 16
                archive.writestr(info, raw, compresslevel=9)
    return {'archive': str(output.absolute()), 'version': manifest['version'], 'source_revision': revision,
            'source_dirty': dirty, 'sha256': hashlib.sha256(output.read_bytes()).hexdigest(), 'app_dependencies': 1}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-id', required=True, help='Existing owner app id, not a token or connection id')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(build(args.app_id, args.output), indent=2))
