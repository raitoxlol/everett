from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import sys
import time
from pathlib import Path

from . import __version__, core, install, registry, trunk
from .route import MIN_CONFIDENCE, RouteError, best_dir, default_harness, route
from .send import (MODES, SPAWNABLE, SendError, command_for, delivery_mode, format_command, hop_env, refuse_self,
                   send, send_inbox, spawn, spawn_command)
from .session import Session, home


def _ago(ts: float) -> str:
    d = int(time.time() - ts)
    return f'{d // 60}m' if d < 3600 else f'{d // 3600}h'


def _where(cwd: str) -> str:
    home = str(Path.home())
    if not cwd or cwd == '/':
        return '-'
    return '~' + cwd[len(home):] if cwd.startswith(home) else cwd


def _width() -> int:
    return shutil.get_terminal_size((100, 20)).columns


def cmd_ls(args) -> int:
    sessions = registry.scan(args.hours, include_auto=args.all, harness=args.harness or '')
    if args.json:
        print(json.dumps([s.to_dict() for s in sessions], indent=2))
        return 0
    if not sessions:
        print('no sessions')
        return 0
    wide = max(len(_where(s.cwd)) for s in sessions)
    wide = min(wide, 28)
    for s in sessions:
        dot = '●' if s.running else '○'
        where = _where(s.cwd)
        where = where if len(where) <= wide else '…' + where[-(wide - 1):]
        work = s.card or s.title or s.first_user or '(no text)'
        if s.source == 't3code':
            work = f'[t3code] {work}'
        if s.state_kind in ('blocked', 'needs-input'):
            work = f'⚠ {s.state} · {work}'
        line = f'{dot} {s.harness:<6} {_ago(s.last_active):>3}  {where:<{wide}}  {work}'
        print(line[:_width()])
    return 0


def card_coverage(sessions) -> tuple[dict, dict]:
    """Per-harness and total counts of agent/auto/missing cards."""
    counts = {harness: {'total': 0, 'agent': 0, 'auto': 0, 'missing': 0}
              for harness in registry.ADAPTERS}
    for session in sessions:
        harness_counts = counts.setdefault(
            session.harness, {'total': 0, 'agent': 0, 'auto': 0, 'missing': 0})
        harness_counts['total'] += 1
        source = session.card_source if session.card_source in ('agent', 'auto') else 'missing'
        harness_counts[source] += 1
    totals = {key: sum(item[key] for item in counts.values())
              for key in ('total', 'agent', 'auto', 'missing')}
    return counts, totals


def cmd_cards(args) -> int:
    sessions = registry.scan(args.hours, include_auto=True, limit=None)
    counts, totals = card_coverage(sessions)
    print(f'Sessions (last {args.hours:g} hours): {totals["total"]}')
    print(f'Agent cards: {totals["agent"]}')
    print(f'Auto cards: {totals["auto"]}')
    print(f'Missing: {totals["missing"]}')
    print('Per harness:')
    for harness, item in counts.items():
        print(f'  {harness}: {item["total"]} sessions, {item["agent"]} agent, '
              f'{item["auto"]} auto, {item["missing"]} missing')
    return 0


def cmd_route(args) -> int:
    sessions = registry.scan(args.hours)
    try:
        r = route(args.text, sessions, router=args.router)
    except RouteError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    if args.json:
        print(json.dumps(r, indent=2))
        return 0
    print(f'{r["decision"]}  (choice={r["choice"]}, confidence={r["confidence"]:.2f}, router={r.get("router", "jev")})')
    if r['decision'] == 'SESSION':
        s = r['session']
        print(f'  → [{s["harness"]}] {s["cwd"]} — {s["title"] or s["first_user"][:80]}')
    if r.get('suggested'):
        print(f'  closest: {r["suggested"]}')
    if r.get('command'):
        print(f'  {r["command"]}')
    return 0


def _emit(args, payload: dict, lines: list[str]) -> None:
    if args.json:
        print(json.dumps(payload, indent=2))
    else:
        print('\n'.join(line for line in lines if line is not None))


def _spawn(args, r: dict, sessions) -> int:
    harness = args.harness or default_harness()
    cwd = os.path.abspath(os.path.expanduser(args.dir)) if args.dir else (best_dir(args.text, sessions) or os.getcwd())
    if args.dry_run:
        command = spawn_command(harness, args.text, '<new-session-id>' if harness == 'claude' else '')
        _emit(args, {**r, 'delivered': False, 'dry_run': True, 'spawn': True, 'harness': harness,
                     'cwd': cwd, 'command': command},
              [f'DRY RUN  spawn NEW [{harness}] {cwd}', f'  {format_command(command)}'])
        return 0
    try:
        result = spawn(harness, args.text, cwd, timeout=args.timeout, env=hop_env())
    except SendError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    _emit(args, {**r, 'delivered': True, 'spawn': True, 'harness': harness, 'cwd': cwd,
                 'session_id': result.session_id, 'command': result.command, 'reply': result.reply},
          [f'SPAWNED  [{harness}] {cwd}  session {result.session_id or "(id not found)"}',
           result.reply or None])
    return 0


def cmd_send(args) -> int:
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        print('everett: --timeout must be a finite number greater than zero.', file=sys.stderr)
        return 2
    try:
        hop_env()
    except SendError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    if args.to:
        if args.spawn:
            print('everett: --to and --spawn are exclusive.', file=sys.stderr)
            return 2
        try:
            session = registry.find(args.to, registry.scan(args.hours, include_auto=True, limit=None))
        except registry.SessionLookupError as e:
            print(f'everett: {e}', file=sys.stderr)
            return 2
        r = {'input': args.text, 'decision': 'SESSION', 'choice': 'to', 'confidence': 1.0, 'router': 'direct',
             'session': session.to_dict()}
    else:
        sessions = registry.scan(args.hours)
        try:
            r = route(args.text, sessions, router=args.router)
        except RouteError as e:
            print(f'everett: {e}', file=sys.stderr)
            return e.code
        if r['decision'] == 'NEW' and args.spawn and r['confidence'] >= MIN_CONFIDENCE:
            return _spawn(args, r, sessions)
        if r['decision'] != 'SESSION':
            r['delivered'] = False
            hint = ('  nothing was sent (add --spawn to start a new session)' if r['decision'] == 'NEW'
                    else '  nothing was sent')
            _emit(args, r, [f'{r["decision"]}  (choice={r["choice"]}, confidence={r["confidence"]:.2f})',
                            f'  closest: {r["suggested"]}' if r.get('suggested') else None, hint])
            return 0

    session = Session(**r['session'])
    label = f'[{session.harness}] {session.cwd} — {session.card or session.title or session.first_user[:80]}'
    try:
        refuse_self(session)
        mode = delivery_mode(session, args.mode)
        if mode == 'inbox':
            return _send_inbox(args, r, session, label)
        command = command_for(session, args.text)
    except SendError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    if args.dry_run:
        blocked_running = session.running or registry.session_running(session)
        _emit(args, {**r, 'manual_command': r.get('command'), 'delivered': False, 'dry_run': True,
                     'command': command, 'blocked_running': blocked_running},
              [f'DRY RUN  {label}', f'  {format_command(command)}',
               '  blocked: target session is marked running' if blocked_running else None])
        return 0

    try:
        result = send(session, args.text, timeout=args.timeout, env=hop_env())
    except SendError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    _emit(args, {**r, 'manual_command': r.get('command'), 'delivered': True,
                 'command': result.command, 'reply': result.reply},
          [f'SENT  {label}', result.reply or None])
    return 0


def _send_inbox(args, r: dict, session: Session, label: str) -> int:
    """Live delivery: queue the request in the running session's inbox (its hooks inject it)."""
    if args.dry_run:
        _emit(args, {**r, 'delivered': False, 'dry_run': True, 'mode': 'inbox'},
              [f'DRY RUN  {label}', '  would queue in its inbox; delivered at its next turn or tool call'])
        return 0
    if not math.isfinite(args.wait) or args.wait < 0:
        print('everett: --wait must be a finite number of seconds, 0 or more.', file=sys.stderr)
        return 2
    result = send_inbox(session, args.text, wait=args.wait)
    lines = [f'QUEUED  {label}',
             f'  message {result["message_id"]}: delivered at its next turn or tool call'
             + ('' if result['hooked'] else f' (no {session.harness} inbox hook; it must run `everett inbox`)')]
    if args.wait > 0:
        lines.append(result['reply'] if result['reply'] is not None else
                     f'  no reply within {args.wait:g}s; it will arrive in inbox {result["reply_inbox"]} '
                     '(`everett inbox`)')
    else:
        lines.append(f'  replies go to inbox {result["reply_inbox"]} (`everett inbox`); add --wait S to wait')
    _emit(args, {**r, 'delivered': True, **result}, lines)
    return 0


def cmd_reply(args) -> int:
    from .send import reply
    try:
        result = reply(args.id, args.text)
    except SendError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    print(f'REPLIED  to {result["reply_to"]} (inbox {result["to"]}), message {result["message_id"]}')
    return 0


def cmd_inbox(args) -> int:
    from . import inbox
    from .send import caller_session_id
    sid = args.session or caller_session_id() or inbox.HUMAN
    items = inbox.pending(sid)
    if not args.peek:
        inbox.mark_done(sid, [m['id'] for m in items])
    if args.json:
        print(json.dumps({'session': sid, 'messages': items}, indent=2, ensure_ascii=False))
        return 0
    if not items:
        print(f'inbox {sid}: empty')
        return 0
    for m in items:
        kind = {'reply': f'reply to {m.get("reply_to")}', 'event': 'event'}.get(m.get('kind'), 'message')
        print(f'{m["id"]}  {_ago(float(m.get("ts") or 0))} ago  from {m.get("from")}  ({kind})')
        print('  ' + m['text'].replace('\n', '\n  '))
    return 0


def cmd_event(args) -> int:
    from . import events
    try:
        event = events.record(args.kind, args.message, session=args.session, project=args.project)
    except events.EventError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    print(f'event {event["id"]}: {event["kind"]} [{event["project"] or "-"}] {event["text"]}'
          + ('' if event['session'] else '  (no session detected; not attached to a card)'))
    return 0


def cmd_events(args) -> int:
    from . import events
    try:
        window = events.parse_since(args.since)
    except events.EventError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    if args.check:
        for data in events.check_escalations():
            print(f'escalated: {events.describe(data)} [{data.get("project") or data.get("session", "")[:8]}]')
    items = [e for e in events.read(time.time() - window)
             if not args.session or (e.get('session') or '').startswith(args.session)]
    if args.json:
        print(json.dumps(items, indent=2, ensure_ascii=False))
        return 0
    if not items:
        print(f'no events in the last {args.since}')
        return 0
    for e in items:
        where = e.get('project') or '-'
        who = f'{e.get("harness") or "?"} {(e.get("session") or "-")[:8]}'
        print(f'{time.strftime("%m-%d %H:%M", time.localtime(e["ts"]))}  {e["kind"]:<11} {who:<16} {where:<14} '
              f'{e["text"]}'[:_width() + 40] + ('  (auto)' if e.get('source') == 'auto' else ''))
    return 0


def cmd_subscribe(args) -> int:
    from . import events
    from .send import caller_session_id
    subscriber = args.as_ or caller_session_id()
    try:
        target = events.resolve_target(args.target)
        current = events.subscribe(subscriber, target, remove=args.remove)
    except events.EventError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    verb = 'unsubscribed from' if args.remove else 'subscribed to'
    print(f'{subscriber} {verb} {target}; now following: {", ".join(current) or "nothing"}')
    return 0


def cmd_trunk(args) -> int:
    if args.action == 'merge':
        return cmd_merge(args)
    sessions = registry.scan(args.hours)
    if args.dry_run:
        print(trunk.render(sessions))
        return 0
    try:
        print(f'wrote {trunk.write(sessions)}')
    except trunk.VaultNotConfiguredError as e:
        print(f'everett: {e}', file=sys.stderr)
        return 2
    return 0


def cmd_merge(args) -> int:
    llm = args.llm or core.default_llm()
    try:
        result = core.merge(llm, dry_run=args.dry_run)
    except core.CoreError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    if not result['merged']:
        print('inbox is empty; nothing to merge')
        return 0
    verb = 'would write' if args.dry_run else 'wrote'
    print(f'merged {result["merged"]} learning(s) with --llm {llm}')
    for path, text in result['files'].items():
        print(f'{verb} {path} ({core.words(text)} words)')
        if args.dry_run:
            print(text)
    if result.get('history'):
        print(f'previous core and inbox saved to {result["history"]}')
    if result.get('mirror'):
        print(f'mirrored to {result["mirror"]}')
    return 0


def cmd_learn(args) -> int:
    try:
        entry = core.learn(args.fact, project=args.project, scope=args.scope)
    except core.CoreError as e:
        print(f'everett: {e}', file=sys.stderr)
        return e.code
    where = f'project {entry["project"]}' if entry['scope'] == 'project' else 'global'
    print(f'learned ({where}); it joins the shared core at the next `everett trunk merge`')
    return 0


def cmd_core(args) -> int:
    project = core.slug(args.project) if args.project else core.project_for(os.getcwd())
    if args.action == 'edit-path':
        print(core.project_path(project) if args.project else core.global_path())
        return 0
    if args.action == 'history':
        snaps = sorted(core.history_dir().glob('*')) if core.history_dir().is_dir() else []
        for snap in snaps:
            print(snap)
        if not snaps:
            print('no merges yet')
        return 0
    print(core.context(os.getcwd()) if not args.project else
          (core._read(core.global_path()).strip() + '\n\n' + core._read(core.project_path(project)).strip()).strip()
          or '(empty)')
    pending = len(core.read_inbox())
    if pending:
        print(f'\n({pending} learning(s) waiting in the inbox; run `everett trunk merge`)')
    return 0


def _harnesses(args, choices=('claude', 'codex', 'omp')) -> list[str]:
    picked = [h for h in choices if getattr(args, h, False)]
    return picked or list(choices)


def cmd_install_hooks(args) -> int:
    choices = ('claude', 'codex', 'omp') + (('grok',) if (home() / '.grok').is_dir() or getattr(args, 'grok', False) else ())
    for harness in _harnesses(args, choices):
        if args.apply:
            try:
                print(install.apply(harness))
            except (OSError, ValueError) as e:
                print(f'everett: {harness}: {e}', file=sys.stderr)
                return 1
        else:
            print(f'## {harness}')
            print(install.snippet(harness))
    if not args.apply:
        print('# run again with --apply to merge these (a backup is written first)')
    return 0


def cmd_mcp(args) -> int:
    from . import mcp
    return mcp.serve()


def cmd_install_mcp(args) -> int:
    choices = ('claude', 'codex', 'omp') + (('grok',) if (home() / '.grok').is_dir() or getattr(args, 'grok', False) else ())
    for harness in _harnesses(args, choices):
        if args.apply:
            try:
                print(install.apply_mcp(harness))
            except (OSError, ValueError) as e:
                print(f'everett: {harness}: {e}', file=sys.stderr)
                return 1
        else:
            print(f'## {harness}')
            print(install.mcp_snippet(harness))
    if not args.apply:
        print('# run again with --apply to register it (a backup is written first)')
    return 0


def cmd_doctor(args) -> int:
    from . import doctor
    return doctor.run(args.hours)


def cmd_onboard(args) -> int:
    from . import onboard
    return onboard.run(args)


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog='everett', description='See and reach parallel coding-agent sessions.')
    p.add_argument('--version', action='version', version=f'everett {__version__}')
    p.add_argument('--hours', type=float, default=72, help='look-back window (default 72)')
    sub = p.add_subparsers(dest='cmd', required=True)
    a = sub.add_parser('ls'); a.add_argument('--json', action='store_true'); a.add_argument('--all', action='store_true', help='include automated runs')
    a.add_argument('--harness', choices=tuple(registry.ADAPTERS)); a.set_defaults(fn=cmd_ls)
    c = sub.add_parser('cards', help='show card coverage across recent sessions'); c.set_defaults(fn=cmd_cards)
    routers = ('local', 'jev')
    r = sub.add_parser('route', help='pick the session a request belongs to (prints, never sends)')
    r.add_argument('text'); r.add_argument('--json', action='store_true')
    r.add_argument('--router', choices=routers, help='default: jev when a key is configured, else local')
    r.set_defaults(fn=cmd_route)
    s = sub.add_parser('send', help='deliver a request to the right session (or --to one) and print its reply')
    s.add_argument('text')
    s.add_argument('--json', action='store_true')
    s.add_argument('--dry-run', action='store_true', help='route and print the command without resuming a session')
    s.add_argument('--timeout', type=float, default=120, help='maximum wait for the session reply in seconds')
    s.add_argument('--router', choices=routers, help='default: jev when a key is configured, else local')
    s.add_argument('--to', metavar='SESSION', help='skip routing: session id prefix, card name, or project folder')
    s.add_argument('--spawn', action='store_true', help='when routing says NEW, start a new headless session')
    s.add_argument('--dir', help='directory for --spawn (default: the best-matching session\'s folder, else here)')
    s.add_argument('--harness', choices=SPAWNABLE, help='harness for --spawn (default: config default_harness, else claude)')
    s.add_argument('--mode', choices=MODES, default='auto',
                   help='auto (default): inbox for a session with a live harness process, else headless resume')
    s.add_argument('--wait', type=float, default=0, metavar='S',
                   help='inbox mode: wait up to S seconds for the reply (default 0: it arrives in your inbox)')
    s.set_defaults(fn=cmd_send)
    ev = sub.add_parser('event', help='record what this session is doing: done, blocked, needs-input, info')
    ev.add_argument('kind', choices=('done', 'blocked', 'needs-input', 'info')); ev.add_argument('message')
    ev.add_argument('--project'); ev.add_argument('--session', help='default: the calling session')
    ev.set_defaults(fn=cmd_event)
    es = sub.add_parser('events', help='recent events across sessions')
    es.add_argument('--since', default='24h', help='window like 30m, 24h, 7d (default 24h)')
    es.add_argument('--session', help='only this session (id prefix)'); es.add_argument('--json', action='store_true')
    es.add_argument('--check', action='store_true', help='also run the blocked-too-long escalation now')
    es.set_defaults(fn=cmd_events)
    sb = sub.add_parser('subscribe', help='get events from a session or project in your inbox')
    sb.add_argument('target', help='session id prefix or name, project name, project:<name>, or *')
    sb.add_argument('--as', dest='as_', metavar='SESSION', help='subscriber session (default: the calling session)')
    sb.add_argument('--remove', action='store_true', help='unsubscribe'); sb.set_defaults(fn=cmd_subscribe)
    rp = sub.add_parser('reply', help='answer an Everett inbox message (goes to the sender\'s inbox)')
    rp.add_argument('id'); rp.add_argument('text'); rp.set_defaults(fn=cmd_reply)
    ib = sub.add_parser('inbox', help='show (and mark delivered) the messages waiting for a session')
    ib.add_argument('--session', help='default: the calling session, else the human inbox')
    ib.add_argument('--peek', action='store_true', help='do not mark them delivered')
    ib.add_argument('--json', action='store_true'); ib.set_defaults(fn=cmd_inbox)
    t = sub.add_parser('trunk', help='view: write the session list to your vault; merge: distill learnings into the core')
    t.add_argument('action', nargs='?', choices=('view', 'merge'), default='view')
    t.add_argument('--dry-run', action='store_true', help='print instead of writing')
    t.add_argument('--llm', choices=('claude', 'codex', 'none'),
                   help='merge engine (default: config merge_llm, else claude); none = deterministic append+dedupe')
    t.set_defaults(fn=cmd_trunk)
    le = sub.add_parser('learn', help='push a fact to the shared core inbox (secrets are rejected)')
    le.add_argument('fact'); le.add_argument('--project'); le.add_argument('--scope', choices=('global', 'project'))
    le.set_defaults(fn=cmd_learn)
    co = sub.add_parser('core', help='show the shared core, its file path, or merge history')
    co.add_argument('action', nargs='?', choices=('show', 'edit-path', 'history'), default='show')
    co.add_argument('--project'); co.set_defaults(fn=cmd_core)
    ih = sub.add_parser('install-hooks', help='print (or --apply) the harness hook registrations')
    for h in ('claude', 'codex', 'omp', 'grok'):
        ih.add_argument(f'--{h}', action='store_true')
    ih.add_argument('--apply', action='store_true', help='back up, then merge into the harness config')
    ih.set_defaults(fn=cmd_install_hooks)
    m = sub.add_parser('mcp', help='run the stdio MCP server (for harnesses; see install-mcp)'); m.set_defaults(fn=cmd_mcp)
    im = sub.add_parser('install-mcp', help='print (or --apply) the MCP server registration')
    for h in ('claude', 'codex', 'omp', 'grok'):
        im.add_argument(f'--{h}', action='store_true')
    im.add_argument('--apply', action='store_true', help='back up, then register')
    im.set_defaults(fn=cmd_install_mcp)
    d = sub.add_parser('doctor', help='check stores, hooks, cards, and router'); d.set_defaults(fn=cmd_doctor)
    ob = sub.add_parser('onboard', help='friendly first-time setup (TUI, or --yes for scripts)')
    ob.add_argument('--yes', action='store_true', help='non-interactive: apply the defaults without prompting')
    ob.add_argument('--span-days', type=int, default=3, help='backfill span in days, 1-30 (default 3)')
    ob.add_argument('--no-backfill', action='store_true', help='skip generating backfill cards')
    ob.add_argument('--no-mcp', action='store_true', help='skip registering the MCP server')
    ob.set_defaults(fn=cmd_onboard)
    args = p.parse_args(argv)
    return args.fn(args)


if __name__ == '__main__':
    sys.exit(main())
