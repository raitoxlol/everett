"""Print the shared Everett card instruction for an OMP extension."""
from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from everett.hooks.common import session_start_context  # noqa: E402


def main() -> int:
    if len(sys.argv) != 2:
        return 2
    try:
        context = session_start_context(sys.argv[1], os.getcwd())
    except Exception:  # noqa: BLE001
        return 0
    if context:
        sys.stdout.write(context)
    return 0


if __name__ == '__main__':
    sys.exit(main())
