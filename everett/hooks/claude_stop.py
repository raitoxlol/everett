"""Claude Code Stop hook: write a deterministic Everett fallback card."""
from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    try:
        sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
        from everett.hooks.common import stop_hook
        stop_hook(sys.stdin.read(), 'claude')
    except BaseException:
        pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
