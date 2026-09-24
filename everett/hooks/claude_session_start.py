"""Claude Code SessionStart hook: card instruction + Everett shared core as additionalContext.

Stdin: the hook JSON (session_id, cwd). Stdout: hookSpecificOutput JSON, or nothing. Always exits 0.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path


def main() -> int:
    try:
        if os.environ.get('CLAUDE_CODE_ENTRYPOINT') == 'sdk-cli':
            return 0  # scripted runs don't get cards
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
