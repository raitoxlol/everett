#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
binary="$repo/everett-rs/target/debug/everett"
if [ ! -x "$binary" ]; then
    printf '%s\n' 'Build first: cargo build --locked --manifest-path everett-rs/Cargo.toml' >&2
    exit 1
fi

demo_home=$(mktemp -d "${TMPDIR:-/tmp}/everett-onboarding-demo.XXXXXX")
trap 'rm -rf "$demo_home"' EXIT HUP INT TERM

case "${1:-agents}" in
    agents)
        mkdir -p "$demo_home/.claude/projects/demo" "$demo_home/.codex/sessions/2026/09/20" "$demo_home/.hermes"
        cp "$repo/everett-rs/tests/fixtures/claude.jsonl" "$demo_home/.claude/projects/demo/session.jsonl"
        cp "$repo/everett-rs/tests/fixtures/codex.jsonl" "$demo_home/.codex/sessions/2026/09/20/rollout-demo.jsonl"
        ;;
    empty) ;;
    *) printf '%s\n' 'Usage: sh scripts/demo-onboarding.sh [agents|empty]' >&2; exit 2 ;;
esac

set --
if [ "${EVERETT_NO_ANIM+x}" = x ]; then
    set -- "$@" "EVERETT_NO_ANIM=$EVERETT_NO_ANIM"
fi
if [ "${NO_COLOR+x}" = x ]; then
    set -- "$@" "NO_COLOR=$NO_COLOR"
fi

printf '%s\n' 'Everett onboarding demo. Session data is synthetic; setup writes only to a temporary home.'
env -i HOME="$demo_home" EVERETT_HOME="$demo_home" PATH=/nonexistent \
    TERM="${TERM:-xterm-256color}" LANG="${LANG:-en_US.UTF-8}" EVERETT_NOTIFY=none \
    "$@" "$binary" onboard --no-backfill
