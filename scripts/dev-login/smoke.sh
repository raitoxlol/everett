#!/bin/bash
set -euo pipefail
DEV_LOGIN_DIR="$(dirname "$(realpath "$0")")"
exec node "$DEV_LOGIN_DIR/smoke.mjs" "$@"
