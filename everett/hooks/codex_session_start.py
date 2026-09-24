"""Codex SessionStart hook: card instruction + Everett shared core as additionalContext.

Codex (>= 0.155) runs command hooks from ~/.codex/hooks.json with the Claude-compatible
contract: JSON on stdin (`session_id` = the rollout's session_meta id, `cwd`, `source` =
startup|resume|clear|compact|fork) and `hookSpecificOutput.additionalContext` on stdout.
Always exits 0.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path


def main() -> int:
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
        from everett.hooks.common import session_start_output
        out = session_start_output(sys.stdin.read())
        if out:
            print(out)
    except BaseException:  # a hook must never break or slow the session over Everett
        pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
