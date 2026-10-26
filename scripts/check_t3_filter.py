#!/usr/bin/env python3
"""Compare current T3 lifecycle metadata with an Everett dry-run lookup."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys


DRIVERS = {"codex": "codex", "claudeAgent": "claude", "grok": "grok"}


class CheckError(RuntimeError):
    pass


def links_for(home, native_id):
    store = home / ".t3/userdata/statev2.sqlite"
    with sqlite3.connect(store.as_uri() + "?mode=ro", uri=True, timeout=2) as con:
        con.execute("PRAGMA query_only=ON")
        rows = con.execute(
            "SELECT t.thread_id,t.archived_at,t.deleted_at,j.deleted_at,p.driver,p.payload_json "
            "FROM orchestration_v2_projection_threads t "
            "LEFT JOIN projection_projects j ON j.project_id=t.project_id "
            "JOIN orchestration_v2_projection_provider_threads p "
            "ON p.provider_thread_id=t.active_provider_thread_id OR "
            "(p.thread_id=t.thread_id AND (t.archived_at IS NOT NULL "
            "OR t.deleted_at IS NOT NULL OR j.deleted_at IS NOT NULL))"
        )
        links = []
        for thread, archived, deleted, project_deleted, driver, payload in rows:
            if driver not in DRIVERS:
                continue
            try:
                data = json.loads(payload)
                reference = data.get("nativeThreadRef") if isinstance(data, dict) else None
            except (TypeError, ValueError):
                continue
            if not isinstance(reference, dict):
                continue
            if reference.get("driver") != driver or reference.get("nativeId") != native_id:
                continue
            state = "deleted" if deleted else "archived" if archived else "project-deleted" if project_deleted else "active"
            links.append({"thread_sha256": hashlib.sha256(thread.encode()).hexdigest(),
                          "harness": DRIVERS[driver], "state": state})
        return links


def resolves(binary, home, native_id):
    env = os.environ.copy()
    env.update(HOME=str(home), EVERETT_HOME=str(home), EVERETT_NOTIFY="none", EVERETT_SESSION_ID="")
    for name in ("CLAUDE_CODE_SESSION_ID", "CLAUDE_SESSION_ID", "CODEX_THREAD_ID", "CODEX_SESSION_ID", "GROK_SESSION_ID", "EVERETT_HOPS", "EVERETT_SEND"):
        env.pop(name, None)
    result = subprocess.run(
        [str(binary), "--hours", "876000", "send", "--to", native_id,
         "--mode", "inbox", "--dry-run", "--json", "Everett T3 read-only acceptance probe"],
        env=env, text=True, capture_output=True, timeout=30, check=False,
    )
    if result.returncode == 2 and "no session matches" in result.stderr:
        return False
    if result.returncode:
        raise CheckError("Everett dry-run lookup failed. Run doctor separately for diagnostics.")
    payload = json.loads(result.stdout)
    if payload.get("dry_run") is not True or payload.get("delivered") is not False:
        raise CheckError("Everett did not return a read-only dry-run receipt.")
    if payload.get("session", {}).get("id") != native_id:
        raise CheckError("Everett matched a different session. Use the full native provider ID.")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--home", type=Path, default=Path(os.environ.get("EVERETT_HOME") or Path.home()))
    parser.add_argument("--native-id", required=True)
    parser.add_argument("--expect", choices=("visible", "hidden"), required=True)
    args = parser.parse_args()
    try:
        home = args.home.expanduser().resolve(strict=True)
        if not args.binary.is_absolute() or not args.binary.is_file():
            raise CheckError("--binary must name an existing absolute Everett binary.")
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,199}", args.native_id):
            raise CheckError("--native-id must be a local native provider ID, without a device prefix.")
        links = links_for(home, args.native_id)
        if not links:
            raise CheckError("No supported T3 link has this native ID. The check is inconclusive.")
        visible = any(link["state"] == "active" for link in links)
        resolved = resolves(args.binary, home, args.native_id)
        passed = visible == resolved == (args.expect == "visible")
        print(json.dumps({
            "scope": "local-t3-store-and-everett-cli",
            "checked_at": datetime.now(timezone.utc).isoformat(),
            "native_id": args.native_id,
            "links": links,
            "expected": args.expect,
            "resolved": resolved,
            "passed": passed,
            "native_client_mcp_binding": "not_checked",
            "live_request_reply": "not_checked",
            "owner_authorized_shared_core": "not_checked",
        }, indent=2))
        return 0 if passed else 1
    except CheckError as error:
        print(str(error), file=sys.stderr)
        return 2
    except (OSError, ValueError, sqlite3.Error, subprocess.TimeoutExpired):
        print("T3 filter check is inconclusive. Verify the current v2 store, full native ID, and Everett binary.", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
