"""Print (and mark delivered) pending Everett messages for an OMP session; used by the OMP extension.

Usage: omp_inbox.py <session-id>. Prints nothing when there is nothing to deliver. Always exits 0.
"""
from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    try:
        if len(sys.argv) != 2:
            return 0
        sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
        from everett import inbox
        session_id = sys.argv[1]
        if not inbox.valid_id(session_id):
            return 0
        inbox.touch_live(session_id, 'omp', 'turn')
        text = inbox.take(session_id)
        if text:
            sys.stdout.write(text)
    except BaseException:  # noqa: BLE001
        pass
    return 0


if __name__ == '__main__':
    sys.exit(main())
