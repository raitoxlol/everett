#!/usr/bin/env python3
"""Start Everett stdio without a browser connector or a separate account home."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

MINIMUM = (2, 2, 1)


def runtime(config):
    override = os.environ.get('EVERETT_BIN')
    if override:
        binary = Path(override)
    else:
        binary = Path(json.loads(config.read_text())['binary'])
    if not binary.is_absolute() or not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError('Everett binary must be an absolute executable path. Rerun install.py --binary /path/to/everett.')
    version = subprocess.run([str(binary), '--version'], capture_output=True, text=True, timeout=5, check=True).stdout.strip()
    match = re.fullmatch(r'everett (\d+)\.(\d+)\.(\d+)(?:[-+][\w.-]+)?', version)
    if not match or tuple(map(int, match.groups())) < MINIMUM:
        raise ValueError('Everett 2.2.1 or newer is required; refusing the old runtime. Rerun install.py with the current binary.')
    return binary, version


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--doctor', action='store_true')
    args = parser.parse_args()
    try:
        binary, version = runtime(args.config)
        if args.doctor:
            print(json.dumps({'runtime': str(binary), 'version': version, 'transport': 'stdio', 'browser_oauth': False, 'account_auth': 'existing Everett home; not checked', 'session_identity': 'detected by Everett; never fabricated'}))
            return 0
        os.execv(str(binary), [str(binary), 'mcp'])
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f'Everett local plugin: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
