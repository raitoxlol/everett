"""`everett onboard`: a friendly first-time-setup TUI.

Seven screens: welcome, detect, hooks, mcp, jev (smarter routing + nightly merge, both optional),
backfill (optional), summary. Nothing is written until the final "Apply" confirmation. `q` quits
at any step with no changes.

Uses stdlib `curses` when stdout/stdin are a TTY; falls back to plain sequential prompts
otherwise (or if curses itself fails to start), and to `--yes` for scripts and tests.
"""
from __future__ import annotations

import argparse
import getpass
import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

from . import cards, config, install, registry
from .doctor import STORES
from .hooks import common
from .route import verify_key
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

JEV_LINES = (
    'Jev (typesafe.ai) picks the right session when many are running, instead of a lexical guess.',
    'Without it, Everett falls back to local matching -- no key required, everything stays on this machine.',
)


# ---- pure layout helpers (no curses import; safe to unit-test directly) -----------------------

def step_indicator(step: int, total: int) -> str:
    """'step 2 of 6'."""
    return f'step {step} of {total}'


def progress_rail(step: int, total: int) -> str:
    """'●●○○○○' -- filled dots for completed/current steps, hollow for the rest."""
    step = max(0, min(step, total))
    return '●' * step + '○' * (total - step)


def use_ascii_glyphs(encoding: str | None) -> bool:
    """Whether to fall back to [x]/[ ] because the terminal encoding isn't UTF-8."""
    return 'utf' not in (encoding or '').lower()


def glyph(checked: bool, ascii_mode: bool = False) -> str:
    """Checkbox glyph: unicode ☑/☐, or ASCII [x]/[ ] when the terminal can't render unicode."""
    if ascii_mode:
        return '[x]' if checked else '[ ]'
    return '☑' if checked else '☐'


def stepper_text(value: int, unit: str = 'day') -> str:
    """Inline stepper label, e.g. '‹ 3 days ›'."""
    plural = '' if value == 1 else 's'
    return f'‹ {value} {unit}{plural} ›'


def animation_disabled() -> bool:
    """Skip the detect-screen reveal animation under NO_COLOR, EVERETT_NO_ANIM, or CI-style envs."""
    return bool(os.environ.get('NO_COLOR') or os.environ.get('EVERETT_NO_ANIM'))


class QuitOnboarding(Exception):
    """Raised when the user quits the TUI or plain flow; caught to exit cleanly with no changes."""


@dataclass
class OnboardConfig:
    detected: dict = field(default_factory=dict)   # harness -> session count
    hooks: dict = field(default_factory=dict)       # harness -> {event: bool}
    mcp: dict = field(default_factory=dict)         # harness -> bool
    backfill_enabled: bool = True
    span_days: int = 3
    jev_choice: str = 'skip'        # 'paste' or 'skip'
    jev_key: str = ''               # only set when jev_choice == 'paste'; never logged or echoed
    jev_found_source: str = ''      # 'env' | 'config' | 'hermes' | '' (not found)
    jev_validated: bool | None = None  # None: not tested (found key, or skipped), else the test route result
    trunk_schedule_enabled: bool = False


@dataclass
class ApplyResult:
    hook_lines: list
    mcp_lines: list
    backfill_written: int = 0
    backfill_skipped: int = 0
    jev_line: str = ''
    trunk_schedule_line: str = ''


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


def find_jev_key_with_source() -> tuple[str, str]:
    """Jev key sources, in order, with which one matched: env, config file, ~/.hermes/.env."""
    key = os.environ.get('TYPESAFE_API_KEY', '').strip()
    if key:
        return key, 'env'
    try:
        key = config.values().get('typesafe_api_key', '').strip()
    except config.ConfigError:
        key = ''
    if key:
        return key, 'config'
    try:
        with (home() / '.hermes' / '.env').open(encoding='utf-8') as env_file:
            for line in env_file:
                name, sep, value = line.partition('=')
                if sep and name.strip() == 'TYPESAFE_API_KEY':
                    value = value.strip().strip('"\'')
                    if value:
                        return value, 'hermes'
    except OSError:
        pass
    return '', ''


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


def apply_jev(cfg: OnboardConfig) -> str:
    """Persist a pasted Jev key (never a found one -- it's already wherever it was found)."""
    if cfg.jev_choice != 'paste' or not cfg.jev_key:
        return 'jev: skipped (using local matching)'
    path = config.set_value('typesafe_api_key', cfg.jev_key)
    status = 'validated' if cfg.jev_validated else 'validation failed; kept anyway'
    return f'jev: key saved to {path} ({status})'


def apply_trunk_schedule(cfg: OnboardConfig) -> str:
    if not cfg.trunk_schedule_enabled:
        return 'trunk schedule: skipped (not selected)'
    from . import trunk_schedule
    result = trunk_schedule.install()
    return f'trunk schedule: installed ({result["path"]}, nightly at 04:00, --llm claude)'


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
            content = common._auto_card(Path(s.path), s.harness, s.cwd, s.id)
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
    jev_line = apply_jev(cfg)
    trunk_schedule_line = apply_trunk_schedule(cfg)
    written = skipped = 0
    if cfg.backfill_enabled:
        written, skipped = generate_backfill_cards(missing_card_sessions(cfg.span_days), backfill_progress)
    return ApplyResult(hook_lines, mcp_lines, written, skipped, jev_line, trunk_schedule_line)


def summary_lines(cfg: OnboardConfig, result: ApplyResult) -> list:
    lines = ['Everett onboarding complete.', '', 'Hooks:']
    lines += [f'  {line}' for line in result.hook_lines] or ['  (none)']
    lines.append('MCP:')
    lines += [f'  {line}' for line in result.mcp_lines] or ['  (none)']
    lines.append(f'Smarter routing: {result.jev_line}')
    lines.append(f'Nightly merge: {result.trunk_schedule_line}')
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
    apply_jev_key_env(cfg, getattr(args, 'jev_key_env', None))
    cfg.trunk_schedule_enabled = bool(getattr(args, 'schedule_merge', False))
    result = run_apply(cfg)
    for line in summary_lines(cfg, result):
        print(line)
    return 0


def apply_jev_key_env(cfg: OnboardConfig, var: str | None) -> None:
    """--yes mode: read a Jev key from the named env var (`--jev-key-env VAR`), validate, and stage
    it for saving. A found key elsewhere (env/config/hermes) needs no action -- it already works."""
    found_key, found_source = find_jev_key_with_source()
    if found_key:
        cfg.jev_found_source = found_source
        cfg.jev_choice = 'skip'
        return
    if not var:
        return
    key = os.environ.get(var, '').strip()
    if not key:
        return
    cfg.jev_validated = verify_key(key)
    cfg.jev_key, cfg.jev_choice = key, 'paste'


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


def _plain_jev_entry(cfg: OnboardConfig) -> None:
    """Prompt for a Jev key with getpass (masked, never echoed or logged), validate it with one
    test route call, and either stage it for saving or ask to keep it anyway on failure."""
    try:
        key = getpass.getpass('  paste key (input hidden, enter to skip): ').strip()
    except (EOFError, KeyboardInterrupt):
        key = ''
    if not key:
        cfg.jev_choice = 'skip'
        return
    print('  validating...')
    ok = verify_key(key)
    print('  ok' if ok else '  failed')
    if ok:
        cfg.jev_key, cfg.jev_choice, cfg.jev_validated = key, 'paste', True
        return
    if _ask_yes('  keep it anyway?', default=False):
        cfg.jev_key, cfg.jev_choice, cfg.jev_validated = key, 'paste', False
    else:
        cfg.jev_choice = 'skip'


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

    print('Smarter routing (optional)')
    for line in JEV_LINES:
        print(f'  {line}')
    found_key, found_source = find_jev_key_with_source()
    if found_key:
        cfg.jev_found_source = found_source
        print(f'  Jev key found ✓ ({found_source})')
        if _ask_yes('  paste a different key instead?', default=False):
            _plain_jev_entry(cfg)
        else:
            cfg.jev_choice = 'skip'
    elif _ask_yes('  paste a Jev key now?', default=False):
        _plain_jev_entry(cfg)
    else:
        cfg.jev_choice = 'skip'
    print()

    cfg.trunk_schedule_enabled = _ask_yes(
        'Merge shared memory nightly? (schedules `everett trunk merge` via launchd, 04:00, --llm claude)',
        default=False)
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
#
# Screens share a frame (title bar + "step N of 6" + progress rail) drawn by _draw_frame, and a
# small palette from _init_colors that degrades to bold/dim attributes when the terminal has no
# color support. All drawing goes through _put, which clips to the screen and never raises even
# at the bottom-right corner or on a terminal too small to hold the frame.

STEP_TITLES = ('Welcome', 'Detect', 'Hooks', 'MCP server', 'Smarter routing', 'Backfill', 'Confirm')
TOTAL_STEPS = len(STEP_TITLES)
MIN_COLS = 54
MIN_ROWS = 14


def _init_colors():
    """Accent / dim / green / amber attributes, degrading gracefully with no color support."""
    import curses
    has_color = False
    try:
        if curses.has_colors() and os.environ.get('NO_COLOR') is None:
            curses.start_color()
            try:
                curses.use_default_colors()
                bg = -1
            except curses.error:
                bg = curses.COLOR_BLACK
            curses.init_pair(1, curses.COLOR_CYAN, bg)
            curses.init_pair(2, curses.COLOR_WHITE, bg)
            curses.init_pair(3, curses.COLOR_GREEN, bg)
            curses.init_pair(4, curses.COLOR_YELLOW, bg)
            has_color = True
    except curses.error:
        has_color = False
    if has_color:
        return {
            'has_color': True,
            'accent': curses.color_pair(1) | curses.A_BOLD,
            'dim': curses.color_pair(2) | curses.A_DIM,
            'green': curses.color_pair(3) | curses.A_BOLD,
            'amber': curses.color_pair(4) | curses.A_BOLD,
        }
    return {'has_color': False, 'accent': curses.A_BOLD, 'dim': curses.A_DIM,
            'green': curses.A_BOLD, 'amber': curses.A_BOLD}


def _put(stdscr, y, x, text, attr=0, w=None):
    """addnstr that clips to the window and swallows the bottom-right-corner curses.error."""
    import curses
    h, maxw = stdscr.getmaxyx()
    if y < 0 or y >= h or x < 0 or x >= maxw:
        return
    width = maxw - x if w is None else min(w, maxw - x)
    if width <= 0:
        return
    try:
        stdscr.addnstr(y, x, text, width, attr)
    except curses.error:
        pass


def _ascii_mode(stdscr) -> bool:
    enc = getattr(stdscr, 'encoding', None) or (sys.stdout.encoding if sys.stdout else None)
    return use_ascii_glyphs(enc)


def _draw_frame(stdscr, colors, step, title):
    """Erases the screen and draws the title bar + step indicator + progress rail + screen title.
    Returns the first free content row, or None if the terminal is too small to draw into."""
    import curses
    stdscr.erase()
    h, w = stdscr.getmaxyx()
    if h < MIN_ROWS or w < MIN_COLS:
        msg = f'terminal too small ({w}x{h}) -- resize to at least {MIN_COLS}x{MIN_ROWS}'
        _put(stdscr, min(h - 1, 1), max(0, (w - len(msg)) // 2), msg, colors['amber'])
        _put(stdscr, min(h - 1, max(0, h - 1)), max(0, (w - 6) // 2), 'q quit')
        stdscr.refresh()
        return None
    bar = ' everett · setup'
    _put(stdscr, 0, 0, bar.ljust(w), colors['accent'] | curses.A_REVERSE, w)
    indicator = step_indicator(step, TOTAL_STEPS)
    _put(stdscr, 0, max(0, w - len(indicator) - 2), indicator, colors['accent'] | curses.A_REVERSE)
    _put(stdscr, 1, 2, progress_rail(step, TOTAL_STEPS), colors['accent'])
    _put(stdscr, 2, 2, title, curses.A_BOLD)
    return 4


def _footer(stdscr, colors, hint):
    h, w = stdscr.getmaxyx()
    _put(stdscr, h - 1, 2, hint, colors['dim'])


def _wait_for_resize_or_key(stdscr):
    """getch that keeps redrawing on KEY_RESIZE instead of treating it as an ordinary key."""
    import curses
    return stdscr.getch()


def _checklist(stdscr, colors, step, title, intro, items):
    """Generic toggle-list screen. items: list of (label, path, getter, setter). Returns 'forward'/'back'."""
    import curses
    ascii_mode = _ascii_mode(stdscr)
    idx = 0
    while True:
        y = _draw_frame(stdscr, colors, step, title)
        if y is not None:
            h, w = stdscr.getmaxyx()
            for line in intro:
                _put(stdscr, y, 2, line, colors['dim'])
                y += 1
            y += 1
            for n, (label, path, getter, _setter) in enumerate(items):
                mark = glyph(getter(), ascii_mode)
                focused = n == idx
                attr = curses.A_REVERSE if focused else 0
                prefix = '›' if focused else ' '
                _put(stdscr, y, 2, f'{prefix} {mark} {label}', attr)
                if path:
                    _put(stdscr, y + 1, 6, str(path), colors['dim'])
                y += 2
            _footer(stdscr, colors, 'up/down move   space toggle   enter continue   b back   q quit')
            stdscr.refresh()
        key = _wait_for_resize_or_key(stdscr)
        if key in (ord('q'), 27):
            raise QuitOnboarding()
        if key in (curses.KEY_UP, ord('k')) and items:
            idx = (idx - 1) % len(items)
        elif key in (curses.KEY_DOWN, ord('j')) and items:
            idx = (idx + 1) % len(items)
        elif key == ord(' ') and items:
            _label, _path, getter, setter = items[idx]
            setter(not getter())
        elif key in (curses.KEY_ENTER, 10, 13):
            return 'forward'
        elif key == ord('b'):
            return 'back'


def _pause(stdscr, colors, step, title, lines, allow_back=True, key_hint=None):
    """A plain information screen with enter/back/quit."""
    import curses
    hint = key_hint or ('enter continue   b back   q quit' if allow_back else 'enter continue   q quit')
    while True:
        y = _draw_frame(stdscr, colors, step, title)
        if y is not None:
            for line in lines:
                _put(stdscr, y, 2, line, colors['dim'] if line.startswith('  ') else 0)
                y += 1
            _footer(stdscr, colors, hint)
            stdscr.refresh()
        key = _wait_for_resize_or_key(stdscr)
        if key in (ord('q'), 27):
            raise QuitOnboarding()
        if key in (curses.KEY_ENTER, 10, 13, ord('a')):
            return 'forward'
        if allow_back and key == ord('b'):
            return 'back'


def _screen_welcome(stdscr, colors):
    lines = list(WELCOME_LINES)
    return _pause(stdscr, colors, 1, 'Welcome to Everett', lines, allow_back=False)


def _screen_detect(stdscr, colors, cfg: OnboardConfig):
    import curses
    if not cfg.detected:
        rows = ['No coding-agent sessions or stores were found on this machine.']
    else:
        rows = [f'{harness}  --  {count} session(s) found' for harness, count in cfg.detected.items()]

    animate = not animation_disabled() and cfg.detected
    if animate:
        y0 = _draw_frame(stdscr, colors, 2, 'What Everett found')
        if y0 is not None:
            delay = min(0.4, 0.4 / max(1, len(rows))) if rows else 0
            for n, row in enumerate(rows):
                mark = glyph(True, _ascii_mode(stdscr))
                _put(stdscr, y0 + n, 2, f'{mark} {row}', colors['green'])
                stdscr.refresh()
                curses.napms(int(delay * 1000))
    lines = [f'{glyph(True, _ascii_mode(stdscr))} {row}' for row in rows] if cfg.detected else rows
    return _pause(stdscr, colors, 2, 'What Everett found', lines)


def _screen_hooks(stdscr, colors, cfg: OnboardConfig):
    items = []
    for harness, events in cfg.hooks.items():
        path = install.omp_extension_path() if harness == 'omp' else install.settings_path(harness)
        for event in events:
            label = f'{harness}: {EVENT_LABELS.get(event, event)}'

            def getter(h=harness, e=event):
                return cfg.hooks[h][e]

            def setter(value, h=harness, e=event):
                cfg.hooks[h][e] = value

            items.append((label, path, getter, setter))
    intro = ['Toggle which hooks to install. A backup is made of any file before it changes.']
    return _checklist(stdscr, colors, 3, 'Hooks', intro, items)


def _screen_mcp(stdscr, colors, cfg: OnboardConfig):
    items = []
    for harness in cfg.mcp:
        label = f'{harness}: register the Everett MCP server'
        path = install.mcp_path(harness)

        def getter(h=harness):
            return cfg.mcp[h]

        def setter(value, h=harness):
            cfg.mcp[h] = value

        items.append((label, path, getter, setter))
    intro = ['Lets each harness call Everett (ls / route / send / learn / card) as a native tool.']
    return _checklist(stdscr, colors, 4, 'MCP server', intro, items)


def _text_entry(stdscr, colors, step, title, intro, mask=True):
    """Minimal masked line editor. Returns the typed text (possibly empty). Enter submits,
    backspace deletes, q/Esc quits onboarding entirely (no partial-entry 'back')."""
    import curses
    buf: list[str] = []
    while True:
        y = _draw_frame(stdscr, colors, step, title)
        if y is not None:
            for line in intro:
                _put(stdscr, y, 2, line, colors['dim'])
                y += 1
            y += 1
            shown = ('*' * len(buf)) if mask else ''.join(buf)
            _put(stdscr, y, 2, '> ' + shown, curses.A_BOLD)
            _footer(stdscr, colors, 'type the key   enter to submit (empty = skip)   q quit')
            stdscr.refresh()
        key = _wait_for_resize_or_key(stdscr)
        if key in (ord('q'), 27):
            raise QuitOnboarding()
        if key in (curses.KEY_ENTER, 10, 13):
            return ''.join(buf)
        if key in (curses.KEY_BACKSPACE, 127, 8):
            if buf:
                buf.pop()
        elif 32 <= key < 127:
            buf.append(chr(key))


def _screen_jev(stdscr, colors, cfg: OnboardConfig):
    import curses
    ascii_mode = _ascii_mode(stdscr)
    found_key, found_source = find_jev_key_with_source()
    if found_key and not cfg.jev_found_source:
        cfg.jev_found_source = found_source
    while True:
        y = _draw_frame(stdscr, colors, 5, 'Smarter routing (optional)')
        if y is not None:
            for line in JEV_LINES:
                _put(stdscr, y, 2, line, colors['dim'])
                y += 1
            y += 1
            if cfg.jev_found_source:
                _put(stdscr, y, 2, f'Jev key found ✓ ({cfg.jev_found_source})', colors['green'])
                y += 2
            paste_mark = glyph(cfg.jev_choice == 'paste', ascii_mode)
            key_note = ' (key entered)' if cfg.jev_choice == 'paste' and cfg.jev_key else ''
            _put(stdscr, y, 2, f'{paste_mark} paste a different key{key_note}'
                 if cfg.jev_found_source else f'{paste_mark} paste a key', 0)
            y += 1
            _put(stdscr, y, 2, f'{glyph(cfg.jev_choice == "skip", ascii_mode)} use local matching', 0)
            y += 2
            merge_mark = glyph(cfg.trunk_schedule_enabled, ascii_mode)
            _put(stdscr, y, 2, f'{merge_mark} merge shared memory nightly (launchd, 04:00, --llm claude)', 0)
            _footer(stdscr, colors, 'p paste key   s skip   m toggle nightly merge   enter continue   b back   q quit')
            stdscr.refresh()
        key = _wait_for_resize_or_key(stdscr)
        if key in (ord('q'), 27):
            raise QuitOnboarding()
        if key == ord('p'):
            entered = _text_entry(stdscr, colors, 5, 'Smarter routing (optional)',
                                   ['Paste the Jev key (typesafe.ai).'])
            if not entered:
                if cfg.jev_choice != 'paste':
                    cfg.jev_choice = 'skip'
                continue
            lines = ['validating…']
            _put(stdscr, 4, 2, lines[0], colors['dim'])
            stdscr.refresh()
            ok = verify_key(entered)
            if ok:
                cfg.jev_key, cfg.jev_choice, cfg.jev_validated = entered, 'paste', True
                _pause(stdscr, colors, 5, 'Smarter routing (optional)', ['ok ✓ -- the key works.'],
                       allow_back=False, key_hint='enter continue')
            else:
                keep = None
                while keep is None:
                    y2 = _draw_frame(stdscr, colors, 5, 'Smarter routing (optional)')
                    if y2 is not None:
                        _put(stdscr, y2, 2, 'failed -- Everett could not confirm this key.', colors['amber'])
                        _footer(stdscr, colors, 'k keep it anyway   s or enter to skip   q quit')
                        stdscr.refresh()
                    k2 = _wait_for_resize_or_key(stdscr)
                    if k2 in (ord('q'), 27):
                        raise QuitOnboarding()
                    if k2 == ord('k'):
                        keep = True
                    elif k2 in (ord('s'), curses.KEY_ENTER, 10, 13):
                        keep = False
                if keep:
                    cfg.jev_key, cfg.jev_choice, cfg.jev_validated = entered, 'paste', False
                else:
                    cfg.jev_choice = 'skip'
        elif key == ord('s'):
            cfg.jev_choice = 'skip'
        elif key == ord('m'):
            cfg.trunk_schedule_enabled = not cfg.trunk_schedule_enabled
        elif key in (curses.KEY_ENTER, 10, 13):
            return 'forward'
        elif key == ord('b'):
            return 'back'


def _screen_backfill(stdscr, colors, cfg: OnboardConfig):
    import curses
    ascii_mode = _ascii_mode(stdscr)
    while True:
        preview = missing_card_sessions(cfg.span_days) if cfg.backfill_enabled else []
        y = _draw_frame(stdscr, colors, 6, 'Backfill cards (optional)')
        if y is not None:
            _put(stdscr, y, 2, 'Make cards for your recent sessions. This step is skippable.', colors['dim'])
            y += 2
            mark = glyph(cfg.backfill_enabled, ascii_mode)
            _put(stdscr, y, 2, f'{mark} enabled', curses.A_BOLD)
            _put(stdscr, y, 20, '(space to toggle)', colors['dim'])
            y += 2
            _put(stdscr, y, 2, 'span:', 0)
            _put(stdscr, y, 8, stepper_text(cfg.span_days), colors['accent'] | curses.A_BOLD)
            _put(stdscr, y, 24, '(left/right or h/l, 1-30)', colors['dim'])
            y += 2
            if cfg.backfill_enabled:
                _put(stdscr, y, 2, f'{len(preview)} session(s) without a card in that span', colors['green'])
                y += 2
            _put(stdscr, y, 2, 'no LLM calls; cards are deterministic and marked <!-- everett:auto -->',
                 colors['dim'])
            _footer(stdscr, colors, 'space toggle   ‹/› or h/l span   enter continue   b back   q quit')
            stdscr.refresh()
        key = _wait_for_resize_or_key(stdscr)
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
    lines.append('Smarter routing (Jev):')
    if cfg.jev_choice == 'paste' and cfg.jev_key:
        lines.append(f'  save key ({"validated" if cfg.jev_validated else "validation failed, kept anyway"})')
    elif cfg.jev_found_source:
        lines.append(f'  skip (using found key: {cfg.jev_found_source})')
    else:
        lines.append('  skip (local matching)')
    lines.append('Nightly merge:')
    lines.append('  schedule `everett trunk merge` at 04:00 via launchd' if cfg.trunk_schedule_enabled else '  skip')
    lines.append('Backfill:')
    lines.append('  generate cards, last {} day(s)'.format(cfg.span_days) if cfg.backfill_enabled else '  skip')
    return lines


def _screen_confirm(stdscr, colors, cfg: OnboardConfig):
    direction = _pause(stdscr, colors, TOTAL_STEPS, 'Apply these changes?', _confirm_lines(cfg),
                        key_hint='enter/a apply   b back   q quit')
    return 'apply' if direction == 'forward' else direction


def _screen_apply(stdscr, colors, cfg: OnboardConfig) -> ApplyResult:
    """Ticks off Hooks / MCP / Backfill as each finishes, with a progress bar for backfill."""
    import curses
    ascii_mode = _ascii_mode(stdscr)
    steps = ['Hooks', 'MCP', 'Jev', 'Merge schedule'] + (['Backfill'] if cfg.backfill_enabled else [])
    done = set()

    def draw(extra_line=None):
        stdscr.erase()
        h, w = stdscr.getmaxyx()
        _put(stdscr, 0, 2, 'Applying...', curses.A_BOLD)
        y = 2
        for name in steps:
            mark = glyph(name in done, ascii_mode)
            attr = colors['green'] if name in done else colors['dim']
            _put(stdscr, y, 2, f'{mark} {name}', attr)
            y += 1
        if extra_line:
            _put(stdscr, y + 1, 2, extra_line, colors['accent'])
        stdscr.refresh()

    draw()
    hook_lines = apply_hooks(cfg)
    done.add('Hooks')
    draw()
    mcp_lines = apply_mcp(cfg)
    done.add('MCP')
    draw()
    jev_line = apply_jev(cfg)
    done.add('Jev')
    draw()
    trunk_schedule_line = apply_trunk_schedule(cfg)
    done.add('Merge schedule')
    draw()

    written = skipped = 0
    if cfg.backfill_enabled:
        def progress(i, total):
            pct = (i / total) if total else 1.0
            bar_w = 30
            filled = int(bar_w * pct)
            bar = '#' * filled + '-' * (bar_w - filled)
            draw(f'[{bar}] {i}/{total}')

        written, skipped = generate_backfill_cards(missing_card_sessions(cfg.span_days), progress)
        done.add('Backfill')
        draw()
    return ApplyResult(hook_lines, mcp_lines, written, skipped, jev_line, trunk_schedule_line)


def _try_this_box(stdscr, colors, y, w):
    """Draws a boxed 'Try this' panel with the exact first commands and a sample agent prompt."""
    import curses
    commands = ['everett ls', 'everett route "what is my codex session doing?"']
    prompt = 'use everett to ask my <name> session what it is doing'
    inner_w = min(w - 6, max(len(c) for c in commands + [prompt]) + 4)
    box_w = inner_w + 4
    x = 2
    _put(stdscr, y, x, '┌' + '─' * (box_w - 2) + '┐', colors['accent'])
    _put(stdscr, y + 1, x, '│ Try this' + ' ' * (box_w - 11) + '│', colors['accent'] | curses.A_BOLD)
    row = y + 2
    for cmd in commands:
        _put(stdscr, row, x, '│', colors['accent'])
        _put(stdscr, row, x + 2, f'$ {cmd}', colors['green'])
        _put(stdscr, row, x + box_w - 1, '│', colors['accent'])
        row += 1
    _put(stdscr, row, x, '│', colors['accent'])
    _put(stdscr, row, x + 2, 'sample agent prompt:', colors['dim'])
    _put(stdscr, row, x + box_w - 1, '│', colors['accent'])
    row += 1
    _put(stdscr, row, x, '│', colors['accent'])
    _put(stdscr, row, x + 2, f'"{prompt}"', 0)
    _put(stdscr, row, x + box_w - 1, '│', colors['accent'])
    row += 1
    _put(stdscr, row, x, '└' + '─' * (box_w - 2) + '┘', colors['accent'])
    return row + 1


def _screen_summary(stdscr, colors, cfg: OnboardConfig, result: ApplyResult):
    import curses
    lines = summary_lines(cfg, result)
    while True:
        y = _draw_frame(stdscr, colors, TOTAL_STEPS, 'Done')
        if y is not None:
            h, w = stdscr.getmaxyx()
            for line in lines:
                _put(stdscr, y, 2, line, colors['dim'] if line.startswith('  ') else 0)
                y += 1
            _try_this_box(stdscr, colors, y + 1, w)
            _footer(stdscr, colors, 'enter/q to exit')
            stdscr.refresh()
        key = _wait_for_resize_or_key(stdscr)
        if key in (ord('q'), 27, curses.KEY_ENTER, 10, 13, ord('a')):
            return


def _tui_main(stdscr, cfg: OnboardConfig):
    import curses
    curses.curs_set(0)
    colors = _init_colors()
    steps = ('welcome', 'detect', 'hooks', 'mcp', 'jev', 'backfill', 'confirm')
    screens = {
        'welcome': lambda: _screen_welcome(stdscr, colors),
        'detect': lambda: _screen_detect(stdscr, colors, cfg),
        'hooks': lambda: _screen_hooks(stdscr, colors, cfg),
        'mcp': lambda: _screen_mcp(stdscr, colors, cfg),
        'jev': lambda: _screen_jev(stdscr, colors, cfg),
        'backfill': lambda: _screen_backfill(stdscr, colors, cfg),
        'confirm': lambda: _screen_confirm(stdscr, colors, cfg),
    }
    i = 0
    while i < len(steps):
        direction = screens[steps[i]]()
        if direction == 'apply':
            result = _screen_apply(stdscr, colors, cfg)
            _screen_summary(stdscr, colors, cfg, result)
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
    p.add_argument('--jev-key-env', metavar='VAR', help='--yes mode: read a Jev key from this env var and save it')
    p.add_argument('--schedule-merge', action='store_true',
                    help='--yes mode: also schedule nightly `everett trunk merge` via launchd')
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
