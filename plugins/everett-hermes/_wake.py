"""Opt-in live Everett inbox watcher using Hermes's ctx.inject_message."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import threading
from typing import Any

from ._inbox import InboxTracker, everett_home, format_inbox_notice, valid_session_id


@dataclass
class _TrackedSession:
    session_id: str
    session_key: str | None
    platform: str
    home: Path | None


class InboxWakeWatcher:
    """Polls tracked active Hermes sessions and injects fixed ID-only inbox nudges."""

    def __init__(
        self,
        ctx: Any,
        *,
        poll_seconds: float = 5.0,
        max_nudged_per_session: int = 10_000,
    ):
        self._ctx = ctx
        self._poll_seconds = max(1.0, min(float(poll_seconds), 60.0))
        self._tracker = InboxTracker(max_seen_per_session=max_nudged_per_session)
        self._lock = threading.Lock()
        self._session_locks: dict[str, threading.Lock] = {}
        self._sessions: dict[str, _TrackedSession] = {}
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def _session_lock(self, session_id: str) -> threading.Lock:
        with self._lock:
            lock = self._session_locks.get(session_id)
            if lock is None:
                lock = threading.Lock()
                self._session_locks[session_id] = lock
            return lock

    @property
    def running(self) -> bool:
        return self._thread is not None and self._thread.is_alive()

    def is_tracked(self, session_id: str) -> bool:
        with self._lock:
            return session_id in self._sessions

    def tracked_session_key(self, session_id: str) -> str | None:
        with self._lock:
            item = self._sessions.get(session_id)
            return item.session_key if item else None

    def track(self, session_id: str, *, session_key: str | None = None, platform: str = "") -> None:
        if not valid_session_id(session_id):
            return
        with self._lock:
            existing = self._sessions.get(session_id)
            key = session_key if session_key else (existing.session_key if existing else None)
            surf = platform.strip().lower() if platform else (existing.platform if existing else "")
            self._sessions[session_id] = _TrackedSession(
                session_id=session_id,
                session_key=key,
                platform=surf,
                home=everett_home(),
            )
            if self._thread is None and not self._stop.is_set():
                self._thread = threading.Thread(
                    target=self._run,
                    name="everett-hermes-wake",
                    daemon=True,
                )
                self._thread.start()

    def untrack(self, session_id: str) -> None:
        # Hold the per-session lock so any in-flight poll finishes or aborts before untrack returns.
        with self._session_lock(session_id):
            with self._lock:
                self._sessions.pop(session_id, None)
            self._tracker.clear_session(session_id)

    def poll_once(self) -> int:
        """Scan every tracked session once and return the number of nudges accepted."""
        if not hasattr(self._ctx, "inject_message"):
            return 0
        with self._lock:
            session_ids = list(self._sessions.keys())
        nudged = 0
        for session_id in session_ids:
            if self._stop.is_set():
                break
            with self._session_lock(session_id):
                with self._lock:
                    item = self._sessions.get(session_id)
                if item is None or self._stop.is_set():
                    continue
                # Shared hosts (TUI, Desktop, messaging gateways) require an explicit route key;
                # keyless injection is allowed only for a single-session classic CLI REPL.
                if not item.session_key and item.platform != "cli":
                    continue
                # Live wake nudges local Everett requests only; remote/relay/peer traffic stays passive.
                fresh_ids = self._tracker.unseen_ids(
                    item.session_id,
                    home=item.home,
                    local_only=True,
                )
                if not fresh_ids:
                    continue
                notice = format_inbox_notice(item.session_id, fresh_ids)
                if not notice:
                    continue
                try:
                    accepted = bool(
                        self._ctx.inject_message(
                            notice,
                            role="user",
                            session_key=item.session_key,
                        )
                    )
                except Exception:
                    # Uncertain delivery after raising: claim to avoid double-nudging.
                    self._tracker.mark_claimed(item.session_id, fresh_ids)
                    continue
                if accepted:
                    self._tracker.mark_claimed(item.session_id, fresh_ids)
                    nudged += 1
        return nudged

    def _run(self) -> None:
        while not self._stop.wait(self._poll_seconds):
            try:
                self.poll_once()
            except Exception:
                pass

    def stop(self) -> None:
        self._stop.set()
        thread = self._thread
        if thread is not None and thread.is_alive() and thread is not threading.current_thread():
            thread.join(timeout=2.0)
        self._thread = None
        with self._lock:
            self._sessions.clear()
            self._session_locks.clear()
