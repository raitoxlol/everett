"""`everett trunk schedule`: a macOS LaunchAgent that runs `everett trunk merge` nightly.

Writes ~/Library/LaunchAgents/dev.everett.core-merge.plist (default 04:00, default --llm claude)
and, with --apply, bootstraps it with launchctl. `everett trunk merge` already skips cleanly when
the inbox is empty. Logs go to ~/.everett/logs/merge.log.

Every path here is injectable (`path=`) and every launchctl call goes through `runner=`, so tests
never touch the real ~/Library or launchctl.
"""
from __future__ import annotations

import os
import plistlib
import subprocess
import sys
from pathlib import Path

from .session import home

LABEL = 'dev.everett.core-merge'


def plist_path() -> Path:
    return home() / 'Library' / 'LaunchAgents' / f'{LABEL}.plist'


def log_path() -> Path:
    return home() / '.everett' / 'logs' / 'merge.log'


def everett_command() -> list[str]:
    """The `python -m everett` invocation for this install (matches install.mcp_launch)."""
    return [sys.executable, '-m', 'everett']


def _parse_at(at: str) -> tuple[int, int]:
    hour_s, _, minute_s = (at or '04:00').partition(':')
    try:
        hour, minute = int(hour_s), int(minute_s or 0)
    except ValueError as exc:
        raise ValueError(f'--at must be HH:MM, got {at!r}') from exc
    if not (0 <= hour <= 23 and 0 <= minute <= 59):
        raise ValueError(f'--at must be HH:MM (00-23:00-59), got {at!r}')
    return hour, minute


def plist_data(at: str = '04:00', llm: str = 'claude') -> dict:
    hour, minute = _parse_at(at)
    log = str(log_path())
    return {
        'Label': LABEL,
        'ProgramArguments': everett_command() + ['trunk', 'merge', '--llm', llm],
        'StartCalendarInterval': {'Hour': hour, 'Minute': minute},
        'StandardOutPath': log,
        'StandardErrorPath': log,
        'RunAtLoad': False,
    }


def render_plist(at: str = '04:00', llm: str = 'claude') -> bytes:
    return plistlib.dumps(plist_data(at, llm))


def is_scheduled(path: Path | None = None) -> bool:
    return (path or plist_path()).exists()


def _domain() -> str:
    uid = os.getuid() if hasattr(os, 'getuid') else 501
    return f'gui/{uid}'


def install(at: str = '04:00', llm: str = 'claude', path: Path | None = None, log: Path | None = None,
            runner=None) -> dict:
    """Write the plist (idempotent) and `launchctl bootstrap` it. Returns a report dict.

    `runner` defaults to `subprocess.run`, looked up at call time (not bound at import) so tests
    can `mock.patch('everett.trunk_schedule.subprocess.run')` and never touch real launchctl.
    """
    runner = runner or subprocess.run
    target = path or plist_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    (log or log_path()).parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(render_plist(at, llm))
    target.chmod(0o644)
    result = runner(['launchctl', 'bootstrap', _domain(), str(target)],
                     capture_output=True, text=True, check=False)
    ok = getattr(result, 'returncode', 1) == 0
    return {'path': str(target), 'launchctl_ok': ok, 'stderr': (getattr(result, 'stderr', '') or '').strip()}


def remove(path: Path | None = None, runner=None) -> dict:
    """Bootout and delete the plist. Safe to call when nothing is scheduled."""
    runner = runner or subprocess.run
    target = path or plist_path()
    existed = target.exists()
    if existed:
        runner(['launchctl', 'bootout', _domain(), str(target)], capture_output=True, text=True, check=False)
        target.unlink()
    return {'path': str(target), 'removed': existed}
