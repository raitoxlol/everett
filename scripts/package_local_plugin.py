#!/usr/bin/env python3
"""Package the portable installer source without machine-specific runtime files."""
import argparse
import hashlib
import json
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'plugins/everett-local'
FILES = [
    'plugin.json', '.codex-plugin/plugin.json', 'README.md', 'install.py',
    'scripts/launch.py', 'skills/everett-local/SKILL.md',
    'tests/test_launcher.py', 'tests/test_runtime.py', 'tests/smoke.py',
]


def package(output):
    version = json.loads((SOURCE / 'plugin.json').read_text())['version']
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f'everett-local-{version}.zip'
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as bundle:
        for name in FILES:
            bundle.write(SOURCE / name, f'everett-local/{name}')
        bundle.write(ROOT / 'LICENSE', 'everett-local/LICENSE')
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix('.zip.sha256').write_text(f'{digest}  {archive.name}\n')
    return archive


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    print(package(args.output.resolve()))
