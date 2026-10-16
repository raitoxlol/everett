#!/bin/bash
set -euo pipefail
DEV_LOGIN_DIR="$(dirname "$(realpath "$0")")"
DEV_LOGIN_ROOT="$(dirname "$(dirname "$DEV_LOGIN_DIR")")"
umask 077
mkdir -p "$DEV_LOGIN_DIR/.state"
node - "$DEV_LOGIN_DIR" <<'NODE'
const fs = require('node:fs');
if (Number(process.versions.node.split('.')[0]) < 24) throw new Error('Use Node 24 or later');
const disk = fs.statfsSync(process.argv[2]);
if (disk.bavail * disk.bsize < 500 * 1024 * 1024) throw new Error('Local stack needs at least 500 MB of free disk space');
NODE
npm ci --prefix "$DEV_LOGIN_ROOT" --no-audit --no-fund
npm ci --prefix "$DEV_LOGIN_DIR" --ignore-scripts --no-audit --no-fund
exec node "$DEV_LOGIN_DIR/stack.mjs"
