"""Codex Stop hook: a deterministic Everett fallback card, plus the automatic done/blocked/needs-input event."""
from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
        from everett.hooks.common import stop_event, stop_hook
        raw = sys.stdin.read()
        stop_hook(raw, 'codex')
        stop_event(raw, 'codex')
    except BaseException:
        pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
