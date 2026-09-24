"""`everett doctor`: one screen of what Everett can see and what is missing."""
from __future__ import annotations

import shutil
import sys

from . import config, install, registry, trunk
from .adapters import t3code
from .route import find_api_key
from .session import home

STORES = {
    'claude': '.claude/projects',
    'codex': '.codex/sessions',
    'omp': '.omp/agent/sessions',
    'pi': '.pi/agent/sessions',
    'hermes': '.hermes',
    'grok': '.grok/sessions',
    't3code': '.t3/userdata/state.sqlite',  # a database; its threads annotate codex/claude/grok sessions
}


WARNINGS = []


def _line(ok: bool | None, text: str) -> None:
    if ok is False:
        WARNINGS.append(text)
    mark = {True: 'ok  ', False: 'warn', None: '--  '}[ok]
    print(f'[{mark}] {text}')


def run(hours: float = 72) -> int:
    from .cli import card_coverage
    problems = 0
    WARNINGS.clear()
    version_ok = sys.version_info >= (3, 10)
    problems += not version_ok
    _line(version_ok, f'python {sys.version.split()[0]} (needs 3.10+)')

    try:
        config.values()
        _line(True if config.config_path().exists() else None, f'config {config.config_path()}'
              + ('' if config.config_path().exists() else ' (none; defaults in use)'))
    except config.ConfigError as e:
        problems += 1
        _line(False, str(e))

    print('stores:')
    for harness, rel in STORES.items():
        path = home() / rel
        found = path.exists()
        state = 'found' if found else 'not found'
        if harness == 't3code':
            extra = f'{len(t3code.threads())} thread(s) with a harness session id' if found else 'desktop app'
        else:
            extra = f'cli: {shutil.which(harness) or "not on PATH"}'
        _line(found or None, f'  {harness:<7} {path} ({state}); {extra}')

    print('hooks:')
    for harness in ('claude', 'codex', 'omp', 'grok'):
        try:
            events = install.installed(harness)
        except (OSError, ValueError) as e:
            problems += 1
            _line(False, f'  {harness:<7} cannot read config: {e}')
            continue
        missing = [event for event, ok in events.items() if not ok]
        if not (home() / STORES[harness]).is_dir():
            _line(None, f'  {harness:<7} harness not used here')
        elif missing:
            _line(False, f'  {harness:<7} missing {", ".join(missing)}; run `everett install-hooks --{harness}`')
        else:
            _line(True, f'  {harness:<7} installed')

    print('mcp:')
    for harness in ('claude', 'codex', 'omp'):
        if not (home() / STORES[harness]).is_dir():
            continue
        ok = install.mcp_installed(harness)
        _line(ok or None, f'  {harness:<7} ' + ('everett MCP server registered' if ok else
                                                f'not registered; `everett install-mcp --{harness}`'))

    sessions = registry.scan(hours, limit=None)
    counts, totals = card_coverage(sessions)
    _line(bool(totals['total']) or None,
          f'cards (last {hours:g} h, interactive): {totals["total"]} sessions, {totals["agent"]} agent, '
          f'{totals["auto"]} auto, {totals["missing"]} missing')

    if find_api_key():
        _line(True, 'router: jev (key found; --router local also available)')
    else:
        _line(True, 'router: local BM25 (no Jev key; everything stays on this machine)')

    folder = trunk.vault_folder()
    _line(True if folder else None, f'vault: {folder}' if folder else 'vault: not configured (optional)')
    warnings = len(WARNINGS) - problems
    print('healthy' if not WARNINGS else f'{problems} problem(s), {warnings} warning(s)')
    return 1 if problems else 0
