"""Grok CLI Stop hook: write a deterministic Everett fallback card.

Grok sends camelCase JSON on stdin (`sessionId`, `cwd`) and no transcript path, so the session's
chat_history.jsonl is looked up under ~/.grok/sessions. Prints nothing (a Stop hook's stdout could
keep the agent working) and always exits 0.
"""
from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
        from everett.hooks.common import grok_stop_hook
        grok_stop_hook(sys.stdin.read())
    except BaseException:
        pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
