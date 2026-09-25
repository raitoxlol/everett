"""Grok CLI UserPromptSubmit / PostToolUse hook: deliver pending Everett messages to this live session.

Always exits 0 and prints nothing unless there is something to deliver.
"""
from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
        from everett.hooks.deliver import run
        return run('grok')
    except BaseException:  # noqa: BLE001
        return 0


if __name__ == '__main__':
    sys.exit(main())
