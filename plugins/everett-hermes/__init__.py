"""Native Hermes Agent plugin for Everett."""

from __future__ import annotations

import atexit
from collections import OrderedDict
from contextlib import contextmanager
import json
import os
from pathlib import Path
import threading
import time
from typing import Any, Mapping

from ._commands import make_everett_command
from ._everett import (
    McpPool,
    bind_live_session,
    discover_tools,
    find_binary,
    release_live_session,
    session_start_prompt_context,
    write_fallback_auto_card,
)
from ._inbox import InboxTracker, format_inbox_notice, valid_session_id
from ._wake import InboxWakeWatcher


TOOLSET_NAME = "everett"
PLUGIN_DIR = Path(__file__).resolve().parent
SKILL_PATH = PLUGIN_DIR / "skills" / "everett-hermes" / "SKILL.md"


def _setting(ctx: Any, key: str, default: Any) -> Any:
    getter = getattr(ctx, "get_config", None)
    if not callable(getter):
        return default
    try:
        value = getter(key, default)
    except Exception:
        return default
    if value is None:
        return default
    if isinstance(default, bool):
        return value if isinstance(value, bool) else default
    if isinstance(default, int):
        return value if type(value) is int else default
    if isinstance(default, float):
        return float(value) if isinstance(value, (int, float)) and not isinstance(value, bool) else default
    if isinstance(default, str):
        return value if isinstance(value, str) else default
    return value


def _in_everett_send() -> bool:
    return bool(os.environ.get("EVERETT_SEND"))


def _session_cwd(ctx: Any) -> str:
    fn = getattr(ctx, "session_cwd", None)
    if callable(fn):
        try:
            value = fn()
            if isinstance(value, str) and value.strip():
                return value
        except Exception:
            pass
    return ""


def _session_route_key(ctx: Any, session_id: str, explicit: str = "") -> str | None:
    """Resolve a task-local route key verified to belong to *session_id* (never borrow os.environ)."""
    if isinstance(explicit, str) and explicit.strip():
        return explicit.strip()
    fn = getattr(ctx, "session_key", None)
    if callable(fn):
        try:
            value = fn()
            if isinstance(value, str) and value.strip():
                return value.strip()
        except Exception:
            pass
    try:
        from gateway.session_context import get_session_env
        mod = getattr(get_session_env, "__globals__", {})
        if not isinstance(mod, dict):
            return None
        # Require task-local proof for BOTH HERMES_SESSION_ID and HERMES_SESSION_KEY so ambient
        # process env values (e.g. during scoped_session_key()) can never cross-bind sessions.
        unset = mod.get("_UNSET")
        sid_var = mod.get("_SESSION_ID")
        bound_key_fn = mod.get("bound_session_key")
        if sid_var is not None and callable(getattr(sid_var, "get", None)):
            raw_sid = sid_var.get()
            if raw_sid is unset or raw_sid != session_id:
                return None
        else:
            sentinel = object()
            raw_sid = get_session_env("HERMES_SESSION_ID", sentinel)
            if raw_sid is sentinel or raw_sid != session_id or raw_sid == os.environ.get("HERMES_SESSION_ID"):
                return None
        if callable(bound_key_fn):
            value = bound_key_fn()
        else:
            sentinel = object()
            raw = get_session_env("HERMES_SESSION_KEY", sentinel)
            if raw is sentinel or (
                isinstance(raw, str) and raw == os.environ.get("HERMES_SESSION_KEY")
            ):
                return None
            value = raw
        if isinstance(value, str) and value.strip():
            return value.strip()
    except Exception:
        pass
    return None


def _normalize_parameters(raw_schema: Any) -> dict[str, Any]:
    if not isinstance(raw_schema, Mapping):
        return {"type": "object", "properties": {}}
    schema = dict(raw_schema)
    schema.setdefault("type", "object")
    if not isinstance(schema.get("properties"), Mapping):
        schema["properties"] = {}
    return schema


def register(ctx: Any) -> None:
    """Register Everett tools, lifecycle hooks, prompt context, slash command, and skill."""

    def resolve_binary() -> str | None:
        configured = _setting(ctx, "binary", "")
        return find_binary(configured)

    max_clients = _setting(ctx, "max_cached_clients", 16)
    pool = McpPool(resolve_binary, max_clients=max_clients)
    inbox_tracker = InboxTracker()
    session_meta_lock = threading.Lock()
    session_meta: dict[str, dict[str, str]] = {}
    last_live_bind: dict[str, float] = {}
    max_tracked_generations = 512
    session_generations: OrderedDict[str, int] = OrderedDict()
    session_gen_pins: dict[str, int] = {}
    deleted_sessions: OrderedDict[str, None] = OrderedDict()
    # Short-lived verified route handoff for gateway `/new` (Hermes finalizes old_sid before reset).
    recent_routes: OrderedDict[str, tuple[float, str, str, str]] = OrderedDict()
    live_bind_locks: dict[str, list[Any]] = {}

    @contextmanager
    def _live_bind_lock(session_id: str):
        with session_meta_lock:
            entry = live_bind_locks.get(session_id)
            if entry is None:
                entry = [threading.Lock(), 0]
                live_bind_locks[session_id] = entry
            entry[1] += 1
        entry[0].acquire()
        try:
            yield
        finally:
            entry[0].release()
            with session_meta_lock:
                entry[1] -= 1
                if (
                    entry[1] == 0
                    and session_id not in session_meta
                    and live_bind_locks.get(session_id) is entry
                ):
                    live_bind_locks.pop(session_id, None)

    def _current_session_gen_locked(session_id: str) -> int:
        return session_generations.get(session_id, 0)

    def _evict_session_generations_locked(keep: str = "") -> None:
        if len(session_generations) <= max_tracked_generations:
            return
        for sid in list(session_generations.keys()):
            if len(session_generations) <= max_tracked_generations:
                break
            if (
                sid != keep
                and sid not in session_meta
                and sid not in live_bind_locks
                and sid not in session_gen_pins
            ):
                session_generations.pop(sid, None)

    def _bump_session_generation_locked(session_id: str) -> None:
        session_generations[session_id] = session_generations.get(session_id, 0) + 1
        session_generations.move_to_end(session_id)
        _evict_session_generations_locked(keep=session_id)

    @contextmanager
    def _pin_session_generation(session_id: str):
        """Pin this session's generation for the duration of a callback so eviction never resets it."""
        with session_meta_lock:
            session_gen_pins[session_id] = session_gen_pins.get(session_id, 0) + 1
            gen = _current_session_gen_locked(session_id)
        try:
            yield gen
        finally:
            with session_meta_lock:
                remaining = session_gen_pins.get(session_id, 1) - 1
                if remaining <= 0:
                    session_gen_pins.pop(session_id, None)
                    _evict_session_generations_locked()
                else:
                    session_gen_pins[session_id] = remaining

    def _session_generation(session_id: str) -> int:
        with session_meta_lock:
            return _current_session_gen_locked(session_id)

    wake_watcher: InboxWakeWatcher | None = None
    if _setting(ctx, "live_wake", False) and not _in_everett_send():
        wake_watcher = InboxWakeWatcher(
            ctx,
            poll_seconds=_setting(ctx, "wake_poll_seconds", 5.0),
        )

    def _remember_session(
        session_id: str,
        *,
        cwd: str = "",
        platform: str = "",
        session_key: str | None = None,
        expected_gen: int | None = None,
    ) -> bool:
        resolved_key = _session_route_key(ctx, session_id, session_key or "")
        with _live_bind_lock(session_id):
            with session_meta_lock:
                if expected_gen is not None and _current_session_gen_locked(session_id) != expected_gen:
                    return False
                meta = session_meta.setdefault(session_id, {})
                if cwd:
                    meta["cwd"] = cwd
                if platform:
                    meta["platform"] = platform
                if resolved_key:
                    meta["session_key"] = resolved_key
                effective_key = meta.get("session_key") or None
                effective_platform = meta.get("platform") or ""
            if wake_watcher is not None:
                wake_watcher.track(session_id, session_key=effective_key, platform=effective_platform)
            return True

    def _ensure_live_bound(
        session_id: str,
        *,
        cwd: str = "",
        force: bool = False,
        expected_gen: int | None = None,
    ) -> None:
        # Serialize bind and release per session and guard with lifecycle generation so a
        # paused pre-turn/start callback cannot resurrect live state after finalization.
        with _live_bind_lock(session_id):
            now = time.monotonic()
            with session_meta_lock:
                if expected_gen is not None and _current_session_gen_locked(session_id) != expected_gen:
                    return
                last = last_live_bind.get(session_id)
                if not force and last is not None and (now - last) < 30.0:
                    return
                last_live_bind[session_id] = now
            bind_live_session(resolve_binary(), session_id, cwd=cwd or None)

    # 1. Register Everett MCP tools as first-class native Hermes tools with per-session identity.
    tools = discover_tools(resolve_binary())
    for tool in tools:
        tool_name = str(tool["name"])
        description = str(tool.get("description") or f"Everett tool {tool_name}.")
        parameters = _normalize_parameters(tool.get("inputSchema"))

        def _make_handler(name: str):
            def _handler(args: Mapping[str, Any] | None = None, session_id: str | None = None, **_kw: Any) -> str:
                clean_args = dict(args) if isinstance(args, Mapping) else {}
                if not valid_session_id(session_id):
                    return pool.call_tool(name, clean_args, session_id=session_id)
                sid = str(session_id)
                with _pin_session_generation(sid) as gen, pool.pin_generation(sid) as ticket:
                    if ticket is None or _session_generation(sid) != gen:
                        return json.dumps({"error": "Everett MCP session was closed.", "isError": True})
                    cwd = _session_cwd(ctx)
                    if not _remember_session(sid, cwd=cwd, expected_gen=gen):
                        return json.dumps({"error": "Everett MCP session was closed.", "isError": True})
                    return pool.call_tool(
                        name,
                        clean_args,
                        session_id=sid,
                        cwd=cwd or None,
                        generation_ticket=ticket,
                    )

            return _handler

        ctx.register_tool(
            name=tool_name,
            toolset=TOOLSET_NAME,
            schema={"name": tool_name, "description": description, "parameters": parameters},
            handler=_make_handler(tool_name),
            description=description,
        )

    # 2. Prompt-cache-safe system prompt section (rendered once at new session creation).
    if hasattr(ctx, "register_system_prompt_section"):
        def _prompt_section(session_info: Mapping[str, Any]) -> str:
            if _in_everett_send() or not _setting(ctx, "inject_core", True):
                return ""
            sid = str(session_info.get("session_id") or "")
            cwd = str(session_info.get("cwd") or "") or _session_cwd(ctx)
            return session_start_prompt_context(resolve_binary(), sid, cwd=cwd or None)

        ctx.register_system_prompt_section(
            "everett",
            _prompt_section,
            position="after_memory",
            max_chars=3500,
        )

    # 3. Lifecycle hooks:
    # - on_session_start binds liveness and registers the session for live wake.
    # - on_session_end fires after every turn; ensure fallback auto-card exists while keeping
    #   live wake tracking and session liveness active while the session sits idle.
    # - on_session_finalize fires on close/eviction/teardown; untrack, release liveness, close MCP.
    def _on_session_start(session_id: str = "", platform: str = "", session_key: str = "", **_kw: Any) -> None:
        if _in_everett_send() or not valid_session_id(session_id):
            return
        with session_meta_lock:
            deleted_sessions.pop(session_id, None)
        with _pin_session_generation(session_id) as gen:
            cwd = _session_cwd(ctx)
            _remember_session(session_id, cwd=cwd, platform=str(platform or ""), session_key=session_key, expected_gen=gen)
            _ensure_live_bound(session_id, cwd=cwd, force=True, expected_gen=gen)

    def _turn_state(completed: bool | None, failed: bool | None, interrupted: bool | None) -> str:
        if failed:
            return "Session ended with error"
        if interrupted:
            return "Session interrupted"
        if completed:
            return "Session completed"
        return "Session ended"

    def _on_turn_end(
        session_id: str = "",
        platform: str = "",
        completed: bool | None = None,
        failed: bool | None = None,
        interrupted: bool | None = None,
        **_kw: Any,
    ) -> None:
        if not valid_session_id(session_id):
            return
        with _pin_session_generation(session_id) as gen:
            cwd = _session_cwd(ctx)
            _remember_session(session_id, cwd=cwd, platform=str(platform or ""), expected_gen=gen)
            if _in_everett_send() or not _setting(ctx, "auto_cards", True):
                return
            with _live_bind_lock(session_id):
                with session_meta_lock:
                    if _current_session_gen_locked(session_id) != gen or session_id in deleted_sessions:
                        return
                    meta = dict(session_meta.get(session_id, {}))
                write_fallback_auto_card(
                    session_id,
                    cwd=meta.get("cwd") or cwd,
                    platform=str(platform or meta.get("platform") or ""),
                    state=_turn_state(completed, failed, interrupted),
                )

    def _teardown_session(
        session_id: str,
        *,
        platform: str = "",
        completed: bool | None = None,
        failed: bool | None = None,
        interrupted: bool | None = None,
        write_card: bool = True,
        write_card_only_if_active: bool = False,
        retain_route: bool = False,
        mark_deleted: bool = False,
    ) -> None:
        if not valid_session_id(session_id):
            return
        with _live_bind_lock(session_id):
            # Retire the MCP pool session BEFORE bumping session_generations so a concurrent tool
            # handler (which pins session_generations first, then pool.pin_generation) can never
            # pair the post-teardown plugin generation with a pre-teardown MCP ticket.
            pool.close_session(session_id)
            with session_meta_lock:
                _bump_session_generation_locked(session_id)
                was_active = session_id in session_meta
                meta = session_meta.pop(session_id, {})
                last_live_bind.pop(session_id, None)
                if mark_deleted:
                    deleted_sessions[session_id] = None
                    deleted_sessions.move_to_end(session_id)
                    while len(deleted_sessions) > max_tracked_generations:
                        deleted_sessions.popitem(last=False)
                was_deleted = session_id in deleted_sessions
                if retain_route and not was_deleted and meta.get("session_key"):
                    recent_routes[session_id] = (
                        time.monotonic(),
                        meta["session_key"],
                        meta.get("cwd", ""),
                        meta.get("platform", ""),
                    )
                    recent_routes.move_to_end(session_id)
                    while len(recent_routes) > 64:
                        recent_routes.popitem(last=False)
                elif not retain_route or was_deleted:
                    recent_routes.pop(session_id, None)
            inbox_tracker.clear_session(session_id)
            if wake_watcher is not None:
                wake_watcher.untrack(session_id)
            cwd = meta.get("cwd") or _session_cwd(ctx)
            should_write_card = (
                write_card
                and not was_deleted
                and (not write_card_only_if_active or was_active)
                and _setting(ctx, "auto_cards", True)
            )
            if not _in_everett_send():
                release_live_session(resolve_binary(), session_id, cwd=cwd or None)
                if should_write_card:
                    write_fallback_auto_card(
                        session_id,
                        cwd=cwd,
                        platform=str(platform or meta.get("platform") or ""),
                        state=_turn_state(completed, failed, interrupted),
                    )

    def _on_session_finalize(
        session_id: str = "",
        platform: str = "",
        completed: bool | None = None,
        failed: bool | None = None,
        interrupted: bool | None = None,
        **_kw: Any,
    ) -> None:
        _teardown_session(
            session_id,
            platform=platform,
            completed=completed,
            failed=failed,
            interrupted=interrupted,
            write_card=True,
            retain_route=True,
        )

    def _on_session_reset(
        session_id: str = "",
        old_session_id: str = "",
        new_session_id: str = "",
        platform: str = "",
        session_key: str = "",
        **_kw: Any,
    ) -> None:
        old_sid = str(old_session_id or "")
        new_sid = str(new_session_id or session_id or "")
        carried_key = str(session_key or "").strip()
        carried_cwd = ""
        carried_platform = str(platform or "")
        if valid_session_id(old_sid):
            with session_meta_lock:
                old_meta = dict(session_meta.get(old_sid, {}))
                stashed = recent_routes.pop(old_sid, None)
            if not carried_key:
                if old_meta.get("session_key"):
                    carried_key = old_meta["session_key"]
                elif stashed is not None and (time.monotonic() - stashed[0]) <= 60.0:
                    carried_key = stashed[1]
            if not carried_cwd:
                carried_cwd = old_meta.get("cwd") or (stashed[2] if stashed is not None else "")
            if not carried_platform:
                carried_platform = old_meta.get("platform") or (stashed[3] if stashed is not None else "")
        if valid_session_id(old_sid) and old_sid != new_sid:
            _teardown_session(
                old_sid,
                platform=carried_platform,
                write_card=True,
                write_card_only_if_active=True,
                retain_route=False,
            )
        if not _in_everett_send() and valid_session_id(new_sid) and new_sid != old_sid:
            with session_meta_lock:
                deleted_sessions.pop(new_sid, None)
            with _pin_session_generation(new_sid) as gen:
                cwd = _session_cwd(ctx) or carried_cwd
                _remember_session(
                    new_sid,
                    cwd=cwd,
                    platform=carried_platform,
                    session_key=carried_key or None,
                    expected_gen=gen,
                )
                _ensure_live_bound(new_sid, cwd=cwd, force=True, expected_gen=gen)

    def _on_session_delete(
        session_id: str = "",
        deleted_ids: Any = None,
        **_kw: Any,
    ) -> None:
        targets: list[str] = []
        if valid_session_id(session_id):
            targets.append(session_id)
        if isinstance(deleted_ids, (list, tuple, set)):
            for item in deleted_ids:
                if valid_session_id(item) and item not in targets:
                    targets.append(str(item))
        for sid in targets:
            _teardown_session(sid, write_card=False, retain_route=False, mark_deleted=True)

    # 4. Safe inbox pickup hooks (constant-template ID-only nudges; zero subprocesses).
    def _pre_llm_call(session_id: str = "", platform: str = "", session_key: str = "", **_kw: Any) -> Any:
        if _in_everett_send() or not valid_session_id(session_id):
            return None
        with _pin_session_generation(session_id) as gen:
            cwd = _session_cwd(ctx)
            _remember_session(session_id, cwd=cwd, platform=str(platform or ""), session_key=session_key, expected_gen=gen)
            # Resumed Hermes sessions reuse their saved prompt and skip on_session_start,
            # so restore/refresh Everett live binding here as well.
            _ensure_live_bound(session_id, cwd=cwd, expected_gen=gen)
            if not _setting(ctx, "inbox_hooks", True):
                return None
            with _live_bind_lock(session_id):
                if _session_generation(session_id) != gen:
                    return None
                fresh = inbox_tracker.claim_new_ids(session_id)
            if not fresh:
                return None
            notice = format_inbox_notice(session_id, fresh)
            return {"context": notice} if notice else None

    def _transform_tool_result(
        tool_name: str = "",
        result: Any = None,
        session_id: str = "",
        **_kw: Any,
    ) -> Any:
        if _in_everett_send() or not isinstance(result, str) or tool_name == "everett_inbox":
            return None
        if not valid_session_id(session_id) or not _setting(ctx, "inbox_hooks", True):
            return None
        # Decide how to attach the notice before claiming IDs so we never break JSON tool results,
        # overwrite an existing `everett_inbox_notice` field, or burn IDs on non-object JSON.
        parsed_obj: dict[str, Any] | None = None
        is_plain_text = False
        try:
            parsed = json.loads(result)
            if isinstance(parsed, dict) and "everett_inbox_notice" not in parsed:
                parsed_obj = parsed
            else:
                return None
        except json.JSONDecodeError:
            is_plain_text = True

        fresh = inbox_tracker.claim_new_ids(session_id)
        if not fresh:
            return None
        notice = format_inbox_notice(session_id, fresh)
        if not notice:
            return None
        if parsed_obj is not None:
            updated = dict(parsed_obj)
            updated["everett_inbox_notice"] = notice
            return json.dumps(updated)
        if is_plain_text:
            return f"{result}\n\n{notice}"
        return None

    def _pre_verify(session_id: str = "", **_kw: Any) -> Any:
        if _in_everett_send() or not valid_session_id(session_id) or not _setting(ctx, "inbox_hooks", True):
            return None
        fresh = inbox_tracker.claim_new_ids(session_id)
        if not fresh:
            return None
        notice = format_inbox_notice(session_id, fresh)
        return {"action": "continue", "message": notice} if notice else None

    ctx.register_hook("on_session_start", _on_session_start)
    ctx.register_hook("on_session_end", _on_turn_end)
    ctx.register_hook("on_session_finalize", _on_session_finalize)
    ctx.register_hook("on_session_reset", _on_session_reset)
    ctx.register_hook("on_session_delete", _on_session_delete)
    ctx.register_hook("pre_llm_call", _pre_llm_call)
    ctx.register_hook("transform_tool_result", _transform_tool_result)
    ctx.register_hook("pre_verify", _pre_verify)

    # 5. Native /everett slash command across CLI, TUI, Desktop, and gateway.
    if hasattr(ctx, "register_command"):
        ctx.register_command(
            "everett",
            make_everett_command(
                binary_resolver=resolve_binary,
                cwd_resolver=lambda: _session_cwd(ctx),
                live_wake_enabled=lambda: bool(_setting(ctx, "live_wake", False)),
                inbox_tracker=inbox_tracker,
                on_session_key=lambda sid, key: _remember_session(sid, session_key=key),
            ),
            description="Inspect Everett status, local sessions, inbox, card, sent requests, and shared core.",
            args_hint="[status|ls|inbox|card|sent|core|help]",
        )

    # 6. Bundled skill registered directly in Hermes's skill registry.
    if hasattr(ctx, "register_skill") and SKILL_PATH.is_file():
        ctx.register_skill(
            "everett-hermes",
            SKILL_PATH,
            description="Use Everett's native Hermes tools, /everett command, inbox pickup, and shared core.",
        )

    # 7. Clean teardown on plugin unload or interpreter exit.
    def _cleanup() -> None:
        if wake_watcher is not None:
            wake_watcher.stop()
        pool.close_all()

    if hasattr(ctx, "on_unload"):
        ctx.on_unload(_cleanup)
    atexit.register(_cleanup)
