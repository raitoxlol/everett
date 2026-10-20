#!/bin/bash
set -euo pipefail
DEV_LOGIN_DIR="$(dirname "$(realpath "$0")")"
exec node --input-type=module - "$DEV_LOGIN_DIR" <<'NODE'
import fs from 'node:fs';
import { execFileSync } from 'node:child_process';
const directory = process.argv[2];
const pidFile = `${directory}/.state/stack.pid`;
if (!fs.existsSync(pidFile)) { console.log('Stack is already stopped. Data preserved.'); process.exit(0); }
const pid = Number(fs.readFileSync(pidFile, 'utf8').trim());
if (!Number.isInteger(pid) || pid < 2) throw new Error('Invalid stack.pid');
let command;
try { command = execFileSync('ps', ['-p', String(pid), '-o', 'command='], { encoding: 'utf8' }); }
catch { console.log('Stack is stopped. Stale PID file preserved; up.sh checks it on restart.'); process.exit(0); }
if (!command.includes(`${directory}/stack.mjs`)) throw new Error('PID belongs to another process; refusing to signal it');
process.kill(pid, 'SIGTERM');
console.log('Requested stack shutdown. Keys, tokens, and backend data preserved.');
NODE
