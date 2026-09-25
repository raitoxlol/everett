"""`everett onboard`: a friendly first-time-setup TUI.

Six screens: welcome, detect, hooks, mcp, backfill (optional), summary. Nothing is written
until the final "Apply" confirmation. `q` quits at any step with no changes.

Uses stdlib `curses` when stdout/stdin are a TTY; falls back to plain sequential prompts
otherwise (or if curses itself fails to start), and to `--yes` for scripts and tests.
"""
from __future__ import annotations

import argparse
import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

from . import cards, install, registry
from .doctor import STORES
from .hooks import common
from .session import home

# "since forever": detection counts every session Everett can see, not just a recent window.
ALL_HOURS = 24 * 365 * 5

# Harnesses whose hooks live in a settings file Everett can toggle event-by-event (see install.HOOKS).
HOOK_HARNESSES = ('claude', 'codex', 'grok')
MCP_HARNESSES = ('claude', 'codex', 'omp')
# everett/hooks/common._auto_card only knows how to build a fallback card for these harnesses.
BACKFILL_HARNESSES = ('claude', 'codex', 'grok')

EVENT_LABELS = {
    'SessionStart': 'session cards + shared core (SessionStart)',
    'Stop': 'automatic fallback cards + done/blocked/needs-input events (Stop)',
    'UserPromptSubmit': 'live delivery of Everett messages at each new prompt (UserPromptSubmit)',
    'PostToolUse': 'live delivery of Everett messages mid-task, after tool calls (PostToolUse)',
    'extension': 'session cards + shared core + live delivery (extension)',
}

WELCOME_LINES = (
    'Everett sees every Claude Code, Codex, OMP, Pi, Hermes, and Grok session on this machine.',
    'It lets those sessions hand work to each other instead of you copy-pasting between them.',
    'It also gives every session a shared memory: one fact learned in one session reaches the rest.',
)


class QuitOnboarding(Exception):
    """Raised when the user quits the TUI or plain flow; caught to exit cleanly with no changes."""


@dataclass
class OnboardConfig:
    detected: dict = field(default_factory=dict)   # harness -> session count
    hooks: dict = field(default_factory=dict)       # harness -> {event: bool}
    mcp: dict = field(default_factory=dict)         # harness -> bool
    backfill_enabled: bool = True
    span_days: int = 3


@dataclass
class ApplyResult:
    hook_lines: list
    mcp_lines: list
    backfill_written: int = 0
    backfill_skipped: int = 0


# ---- detection --------------------------------------------------------------------------------

def store_exists(harness: str) -> bool:
    rel = STORES.get(harness)
    return bool(rel) and (home() / rel).exists()


def detect_harnesses(hours: float = ALL_HOURS) -> dict:
    """harness -> session count, for every harness whose session store exists."""
    found = {}
    for harness in registry.ADAPTERS:  # claude, codex, omp, pi, hermes, grok
        if store_exists(harness):
            found[harness] = len(registry.scan(hours, include_auto=True, limit=None, harness=harness))
    return found


def default_config() -> OnboardConfig:
    cfg = OnboardConfig(detected=detect_harnesses())
    for harness in HOOK_HARNESSES:
        if harness in cfg.detected:
            cfg.hooks[harness] = {event: True for event, _, _ in install.HOOKS[harness]}
    if 'omp' in cfg.detected:
        cfg.hooks['omp'] = {'extension': True}
    for harness in MCP_HARNESSES:
        if harness in cfg.detected:
            cfg.mcp[harness] = True
    return cfg


def hook_change_lines(cfg: OnboardConfig) -> list:
    """(harness, file that will change, whether anything is selected there)."""
    lines = []
    for harness, events in cfg.hooks.items():
        if harness == 'omp':
            lines.append((harness, str(install.omp_extension_path()), bool(events.get('extension'))))
        else:
            lines.append((harness, str(install.settings_path(harness)), any(events.values())))
    return lines


# ---- applying (only called after the final confirm) --------------------------------------------

def apply_hooks(cfg: OnboardConfig) -> list:
    report = []
    for harness, events in cfg.hooks.items():
        if harness == 'omp':
            report.append(install.apply('omp') if events.get('extension')
                           else 'omp: skipped (not selected)')
            continue
        selected = [event for event, on in events.items() if on]
        report.append(install.apply(harness, events=selected) if selected
                       else f'{harness}: skipped (not selected)')
    return report


def apply_mcp(cfg: OnboardConfig) -> list:
    return [install.apply_mcp(harness) if on else f'{harness}: skipped (not selected)'
            for harness, on in cfg.mcp.items()]


def missing_card_sessions(span_days: int) -> list:
    sessions = registry.scan(span_days * 24, include_auto=False, limit=None)
    return [s for s in sessions if not s.card_source and s.harness in BACKFILL_HARNESSES]


def generate_backfill_cards(sessions: list, progress=None) -> tuple:
    """Write AUTO cards (never overwriting an existing card, agent or auto). Returns (written, skipped)."""
    written = skipped = 0
    total = len(sessions)
    for i, s in enumerate(sessions, 1):
        content = ''
        try:
            content = common._auto_card(Path(s.path), s.harness, s.cwd)
        except OSError:
            content = ''
        if content:
            target = cards.card_path(s.id)
            target.parent.mkdir(parents=True, exist_ok=True)
            try:
                fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            except FileExistsError:
                skipped += 1
            else:
                with os.fdopen(fd, 'w', encoding='utf-8') as f:
                    f.write(content)
                written += 1
        else:
            skipped += 1
        if progress:
            progress(i, total)
    return written, skipped


def run_apply(cfg: OnboardConfig, backfill_progress=None) -> ApplyResult:
    hook_lines = apply_hooks(cfg)
    mcp_lines = apply_mcp(cfg)
    written = skipped = 0
    if cfg.backfill_enabled:
        written, skipped = generate_backfill_cards(missing_card_sessions(cfg.span_days), backfill_progress)
    return ApplyResult(hook_lines, mcp_lines, written, skipped)


def summary_lines(cfg: OnboardConfig, result: ApplyResult) -> list:
    lines = ['Everett onboarding complete.', '', 'Hooks:']
    lines += [f'  {line}' for line in result.hook_lines] or ['  (none)']
    lines.append('MCP:')
    lines += [f'  {line}' for line in result.mcp_lines] or ['  (none)']
    if cfg.backfill_enabled:
        lines.append(f'Backfill: {result.backfill_written} card(s) written, {result.backfill_skipped} skipped '
                     f'(last {cfg.span_days} day(s))')
    else:
        lines.append('Backfill: skipped')
    lines += ['', 'Try next:', '  everett ls',
              '  (from inside an agent) use everett to ask my <name> session what it is doing']
    return lines


# ---- non-interactive (--yes) --------------------------------------------------------------------

def run_yes(args) -> int:
    cfg = default_config()
    cfg.span_days = args.span_days
    cfg.backfill_enabled = not args.no_backfill
    if args.no_mcp:
        cfg.mcp = {harness: False for harness in cfg.mcp}
    result = run_apply(cfg)
    for line in summary_lines(cfg, result):
        print(line)
    return 0


# ---- plain-prompt fallback (no TTY, or curses failed) --------------------------------------------

def _ask_yes(prompt: str, default: bool = True) -> bool:
    suffix = ' [Y/n] ' if default else ' [y/N] '
    while True:
        try:
            raw = input(prompt + suffix).strip().lower()
        except EOFError:
            return default
        if raw in ('q', 'quit'):
            raise QuitOnboarding()
        if not raw:
            return default
        if raw in ('y', 'yes'):
            return True
        if raw in ('n', 'no'):
            return False


def run_plain(args) -> int:
    print('Everett -- first-time setup')
    for line in WELCOME_LINES:
        print(line)
    print()
    cfg = default_config()
    cfg.span_days = args.span_days
    if args.no_backfill:
        cfg.backfill_enabled = False
    if args.no_mcp:
        cfg.mcp = {harness: False for harness in cfg.mcp}

    if not cfg.detected:
        print('No coding-agent sessions or stores were found on this machine.')
    else:
        print('Found:')
        for harness, count in cfg.detected.items():
            print(f'  [x] {harness}: {count} session(s)')
    print()

    print('Hooks -- a backup is made of any file before it changes.')
    for harness, events in cfg.hooks.items():
        if harness == 'omp':
            path = install.omp_extension_path()
            on = _ask_yes(f'  {harness}: install {EVENT_LABELS["extension"]} at {path}?')
            cfg.hooks['omp']['extension'] = on
            continue
        path = install.settings_path(harness)
        for event in list(events):
            on = _ask_yes(f'  {harness}: install {EVENT_LABELS.get(event, event)} in {path}?')
            cfg.hooks[harness][event] = on
    print()

    print('MCP -- lets each harness call Everett as a tool.')
    for harness in list(cfg.mcp):
        on = _ask_yes(f'  register the Everett MCP server for {harness} in {install.mcp_path(harness)}?',
                       default=cfg.mcp[harness])
        cfg.mcp[harness] = on
    print()

    cfg.backfill_enabled = _ask_yes('Make cards for your recent sessions without one? (optional, skippable)',
                                     default=cfg.backfill_enabled)
    if cfg.backfill_enabled:
        raw = input(f'  how many days back? [1-30, default {cfg.span_days}] ').strip()
        if raw:
            if raw.isdigit() and 1 <= int(raw) <= 30:
                cfg.span_days = int(raw)
            else:
                print('  not 1-30; keeping the default.')
        preview = missing_card_sessions(cfg.span_days)
        print(f'  {len(preview)} session(s) without a card in the last {cfg.span_days} day(s).')
        if not preview:
            cfg.backfill_enabled = False
        elif not _ask_yes(f'  generate {len(preview)} automatic card(s) now?'):
            cfg.backfill_enabled = False
    print()

    if not _ask_yes('Apply these changes now?'):
        print('Nothing was changed.')
        return 0

    def progress(i, total):
        print(f'\r  generating cards {i}/{total}', end='', flush=True)

    result = run_apply(cfg, backfill_progress=progress if cfg.backfill_enabled else None)
    if cfg.backfill_enabled:
        print()
    print()
    for line in summary_lines(cfg, result):
        print(line)
    return 0


# ---- curses TUI -----------------------------------------------------------------------------

def _checklist(stdscr, title, intro, items):
    """Generic toggle-list screen. items: list of (label, getter, setter). Returns 'forward'/'back'."""
    import curses
    idx = 0
    while True:
        stdscr.erase()
        h, w = stdscr.getmaxyx()
        y = 0
        stdscr.addnstr(y, 2, title, max(1, w - 4), curses.A_BOLD)
        y += 2
        for line in intro:
            stdscr.addnstr(y, 2, line, max(1, w - 4))
            y += 1
        y += 1
        for n, (label, getter, _setter) in enumerate(items):
            mark = 'x' if getter() else ' '
            prefix = '>' if n == idx else ' '
            attr = curses.A_REVERSE if n == idx else 0
            stdscr.addnstr(y, 2, f'{prefix} [{mark}] {label}', max(1, w - 4), attr)
            y += 1
        y += 1
        stdscr.addnstr(y, 2, 'up/down or j/k move   space toggle   enter continue   b back   q quit',
                       max(1, w - 4))
        stdscr.refresh()
        key = stdscr.getch()
        if key in (ord('q'), 27):
            raise QuitOnboarding()
        if key in (curses.KEY_UP, ord('k')) and items:
            idx = (idx - 1) % len(items)
        elif key in (curses.KEY_DOWN, ord('j')) and items:
            idx = (idx + 1) % len(items)
        elif key == ord(' ') and items:
            label, getter, setter = items[idx]
            setter(not getter())
        elif key in (curses.KEY_ENTER, 10, 13):
            return 'forward'
        elif key == ord('b'):
            return 'back'


def _pause(stdscr, title, lines, allow_back=True, key_hint=None):
    """A plain information screen with enter/back/quit."""
    import curses
    hint = key_hint or ('enter continue   b back   q quit' if allow_back else 'enter continue   q quit')
    while True:
        stdscr.erase()
        h, w = stdscr.getmaxyx()
        stdscr.addnstr(0, 2, title, max(1, w - 4), curses.A_BOLD)
        for n, line in enumerate(lines):
            stdscr.addnstr(2 + n, 2, line, max(1, w - 4))
        stdscr.addnstr(2 + len(lines) + 1, 2, hint, max(1, w - 4))
        stdscr.refresh()
        key = stdscr.getch()
        if key in (ord('q'), 27):
            raise QuitOnboarding()
        if key in (curses.KEY_ENTER, 10, 13, ord('a')):
            return 'forward'
        if allow_back and key == ord('b'):
            return 'back'


def _screen_welcome(stdscr):
    return _pause(stdscr, 'Welcome to Everett', list(WELCOME_LINES), allow_back=False)


def _screen_detect(stdscr, cfg: OnboardConfig):
    if not cfg.detected:
        lines = ['No coding-agent sessions or stores were found on this machine.']
    else:
        lines = [f'v {harness}  --  {count} session(s)' for harness, count in cfg.detected.items()]
    return _pause(stdscr, 'What Everett found', lines)


def _screen_hooks(stdscr, cfg: OnboardConfig):
    items = []
    for harness, events in cfg.hooks.items():
        path = install.omp_extension_path() if harness == 'omp' else install.settings_path(harness)
        for event in events:
            label = f'{harness}: {EVENT_LABELS.get(event, event)} -> {path}'

            def getter(h=harness, e=event):
                return cfg.hooks[h][e]

            def setter(value, h=harness, e=event):
                cfg.hooks[h][e] = value

            items.append((label, getter, setter))
    intro = ['Toggle which hooks to install. A backup is made of any file before it changes.']
    return _checklist(stdscr, 'Hooks', intro, items)


def _screen_mcp(stdscr, cfg: OnboardConfig):
    items = []
    for harness in cfg.mcp:
        label = f'{harness}: register the Everett MCP server -> {install.mcp_path(harness)}'

        def getter(h=harness):
            return cfg.mcp[h]

        def setter(value, h=harness):
            cfg.mcp[h] = value

        items.append((label, getter, setter))
    intro = ['Lets each harness call Everett (ls / route / send / learn / card) as a native tool.']
    return _checklist(stdscr, 'MCP server', intro, items)


def _screen_backfill(stdscr, cfg: OnboardConfig):
    import curses
    while True:
        preview = missing_card_sessions(cfg.span_days) if cfg.backfill_enabled else []
        stdscr.erase()
        h, w = stdscr.getmaxyx()
        y = 0
        stdscr.addnstr(y, 2, 'Backfill cards (optional)', max(1, w - 4), curses.A_BOLD)
        y += 2
        stdscr.addnstr(y, 2, 'Make cards for your recent sessions? This step is skippable.', max(1, w - 4))
        y += 2
        mark = 'x' if cfg.backfill_enabled else ' '
        stdscr.addnstr(y, 2, f'[{mark}] enabled  (space to toggle)', max(1, w - 4))
        y += 2
        stdscr.addnstr(y, 2, f'span: {cfg.span_days} day(s)  (left/right or h/l to change, 1-30)', max(1, w - 4))
        y += 2
        if cfg.backfill_enabled:
            stdscr.addnstr(y, 2, f'{len(preview)} session(s) without a card in that span', max(1, w - 4))
            y += 2
        stdscr.addnstr(y, 2, 'no LLM calls; cards are deterministic and marked <!-- everett:auto -->',
                       max(1, w - 4))
        y += 2
        stdscr.addnstr(y, 2, 'enter continue   b back   q quit', max(1, w - 4))
        stdscr.refresh()
        key = stdscr.getch()
        if key in (ord('q'), 27):
            raise QuitOnboarding()
        if key == ord(' '):
            cfg.backfill_enabled = not cfg.backfill_enabled
        elif key in (curses.KEY_LEFT, ord('h')):
            cfg.span_days = max(1, cfg.span_days - 1)
        elif key in (curses.KEY_RIGHT, ord('l')):
            cfg.span_days = min(30, cfg.span_days + 1)
        elif key in (curses.KEY_ENTER, 10, 13):
            return 'forward'
        elif key == ord('b'):
            return 'back'


def _confirm_lines(cfg: OnboardConfig) -> list:
    lines = ['Nothing is written until you apply.', '', 'Hooks:']
    for harness, path, selected in hook_change_lines(cfg):
        lines.append(f'  {"install" if selected else "skip":<7} {harness:<7} {path}')
    lines.append('MCP:')
    for harness, on in cfg.mcp.items():
        lines.append(f'  {"register" if on else "skip":<8} {harness:<7} {install.mcp_path(harness)}')
    lines.append('Backfill:')
    lines.append('  generate cards, last {} day(s)'.format(cfg.span_days) if cfg.backfill_enabled else '  skip')
    return lines


def _screen_confirm(stdscr, cfg: OnboardConfig):
    direction = _pause(stdscr, 'Apply these changes?', _confirm_lines(cfg),
                        key_hint='enter/a apply   b back   q quit')
    return 'apply' if direction == 'forward' else direction


def _screen_apply(stdscr, cfg: OnboardConfig) -> ApplyResult:
    import curses
    h, w = stdscr.getmaxyx()

    def progress(i, total):
        stdscr.erase()
        stdscr.addnstr(0, 2, 'Generating backfill cards...', max(1, w - 4), curses.A_BOLD)
        pct = (i / total) if total else 1.0
        bar_w = max(10, w - 24)
        filled = int(bar_w * pct)
        stdscr.addnstr(2, 2, '[' + '#' * filled + '-' * (bar_w - filled) + f'] {i}/{total}', max(1, w - 4))
        stdscr.refresh()

    stdscr.erase()
    stdscr.addnstr(0, 2, 'Applying...', max(1, w - 4), curses.A_BOLD)
    stdscr.refresh()
    return run_apply(cfg, backfill_progress=progress)


def _screen_summary(stdscr, cfg: OnboardConfig, result: ApplyResult):
    _pause(stdscr, 'Done', summary_lines(cfg, result), allow_back=False, key_hint='enter/q to exit')


def _tui_main(stdscr, cfg: OnboardConfig):
    import curses
    curses.curs_set(0)
    steps = ('welcome', 'detect', 'hooks', 'mcp', 'backfill', 'confirm')
    screens = {
        'welcome': lambda: _screen_welcome(stdscr),
        'detect': lambda: _screen_detect(stdscr, cfg),
        'hooks': lambda: _screen_hooks(stdscr, cfg),
        'mcp': lambda: _screen_mcp(stdscr, cfg),
        'backfill': lambda: _screen_backfill(stdscr, cfg),
        'confirm': lambda: _screen_confirm(stdscr, cfg),
    }
    i = 0
    while i < len(steps):
        direction = screens[steps[i]]()
        if direction == 'apply':
            result = _screen_apply(stdscr, cfg)
            _screen_summary(stdscr, cfg, result)
            return
        i = max(0, i - 1) if direction == 'back' else i + 1


def run_tui(args) -> int:
    import curses
    cfg = default_config()
    cfg.span_days = args.span_days
    if args.no_backfill:
        cfg.backfill_enabled = False
    if args.no_mcp:
        cfg.mcp = {harness: False for harness in cfg.mcp}
    curses.wrapper(_tui_main, cfg)
    return 0


# ---- entry point ----------------------------------------------------------------------------

def build_argparser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog='everett onboard', description='Friendly first-time setup for Everett.')
    p.add_argument('--yes', action='store_true', help='non-interactive: apply the defaults without prompting')
    p.add_argument('--span-days', type=int, default=3, help='backfill span in days, 1-30 (default 3)')
    p.add_argument('--no-backfill', action='store_true', help='skip generating backfill cards')
    p.add_argument('--no-mcp', action='store_true', help='skip registering the MCP server')
    return p


def run(args) -> int:
    if not (1 <= args.span_days <= 30):
        print('everett onboard: --span-days must be between 1 and 30.', file=sys.stderr)
        return 2
    if args.yes:
        return run_yes(args)
    if sys.stdin.isatty() and sys.stdout.isatty():
        try:
            return run_tui(args)
        except QuitOnboarding:
            print('Cancelled -- no changes were made.')
            return 0
        except Exception:  # noqa: BLE001  curses unavailable or failed; fall back to plain prompts
            pass
    try:
        return run_plain(args)
    except QuitOnboarding:
        print('Cancelled -- no changes were made.')
        return 0


def main(argv=None) -> int:
    return run(build_argparser().parse_args(argv))


if __name__ == '__main__':
    sys.exit(main())
