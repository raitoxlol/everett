#!/usr/bin/env python3
"""Opt-in Hermes client smoke using only loopback model and account fixtures."""

from __future__ import annotations

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import time


PLUGIN_ROOT = Path(__file__).resolve().parents[1]
SIGNED_OUT = "EVERETT_HERMES_SIGNED_OUT"
SIGNED_IN = "EVERETT_HERMES_SIGNED_IN"
WHOAMI = "everett_whoami"
LIST = "everett_ls"
TOOL_SEARCH = "tool_search"
TOOL_DESCRIBE = "tool_describe"
TOOL_CALL = "tool_call"
FIXTURE_TOKEN = "evt_" + "a" * 64
ACCOUNT_ERROR = "Local Everett MCP requires an account."


class SmokeFailure(RuntimeError):
    pass


class ModelState:
    def __init__(self):
        self.lock = threading.Lock()
        self.requests = {SIGNED_OUT: [], SIGNED_IN: []}
        self.failures = []
        self.call_number = 0

    def record(self, marker: str, payload: dict) -> None:
        with self.lock:
            self.requests[marker].append(payload)

    def next_call_id(self) -> str:
        with self.lock:
            self.call_number += 1
            return f"call_{self.call_number}"

    def snapshot(self, marker: str) -> list[dict]:
        with self.lock:
            return list(self.requests[marker])


def _content_text(value) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return " ".join(_content_text(part) for part in value)
    if isinstance(value, dict):
        return _content_text(value.get("text", ""))
    return ""


def _marker(payload: dict) -> str | None:
    if not payload.get("tools"):
        return None  # Hermes also sends this conversation to tool-free auxiliary model calls.
    for message in payload.get("messages", []):
        if message.get("role") == "user":
            content = _content_text(message.get("content"))
            for marker in (SIGNED_OUT, SIGNED_IN):
                if content.startswith(marker + ":"):
                    return marker
    return None


def _arguments(value) -> dict:
    if isinstance(value, dict):
        return value
    if isinstance(value, str):
        try:
            parsed = json.loads(value or "{}")
        except json.JSONDecodeError as exc:
            raise SmokeFailure(f"model bridge arguments were not JSON: {exc}") from exc
        if isinstance(parsed, dict):
            return parsed
    raise SmokeFailure("model bridge arguments were not an object")


def _history_calls(messages: list[dict]) -> list[tuple[str, dict]]:
    calls = []
    for message in messages:
        if message.get("role") != "assistant":
            continue
        for call in message.get("tool_calls", []):
            function = call.get("function", {})
            if isinstance(function, dict) and function.get("name"):
                calls.append((function["name"], _arguments(function.get("arguments", {}))))
    return calls


def _targets(marker: str) -> tuple[str, ...]:
    return (WHOAMI,) if marker == SIGNED_OUT else (WHOAMI, LIST)


def _next_call(marker: str, messages: list[dict], available: set[str] | None = None) -> tuple[str, dict] | None:
    history = _history_calls(messages)
    names = _targets(marker)
    direct = available is not None and set(names).issubset(available)
    if direct:
        called = {name for name, _ in history}
        if WHOAMI not in called:
            return WHOAMI, {}
        if marker == SIGNED_IN and LIST not in called:
            return LIST, {"harness": "hermes", "all": True}
        return None

    if not any(name == TOOL_SEARCH for name, _ in history):
        return TOOL_SEARCH, {"queries": ["Everett whoami account identity", "Everett list recent Hermes sessions"]}

    described = set()
    underlying = set()
    for name, arguments in history:
        if name == TOOL_DESCRIBE:
            raw_names = arguments.get("names", [])
            described.update([raw_names] if isinstance(raw_names, str) else raw_names)
        elif name == TOOL_CALL:
            for call in arguments.get("calls", []):
                if isinstance(call, dict) and isinstance(call.get("name"), str):
                    underlying.add(call["name"])

    if not set(names).issubset(described):
        return TOOL_DESCRIBE, {"names": list(names)}
    if WHOAMI not in underlying:
        return TOOL_CALL, {"calls": [{"name": WHOAMI, "arguments": {}}]}
    if marker == SIGNED_IN and LIST not in underlying:
        return TOOL_CALL, {"calls": [{"name": LIST, "arguments": {"harness": "hermes", "all": True}}]}
    return None


class LoopbackHTTPServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


class ModelHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        body = json.dumps({
            "object": "list",
            "data": [{"id": "everett-hermes-smoke", "object": "model", "created": 0, "owned_by": "local"}],
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:
        try:
            payload = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            messages = payload.get("messages", [])
            marker = _marker(payload)
            if marker is None:
                self._send_completion(payload, content="local model auxiliary completion")
                return
            self.server.state.record(marker, payload)
            available = {
                tool.get("function", {}).get("name")
                for tool in payload.get("tools", [])
                if isinstance(tool, dict) and isinstance(tool.get("function"), dict)
            }
            targets = set(_targets(marker))
            has_direct = targets.issubset(available)
            has_bridge = {TOOL_SEARCH, TOOL_DESCRIBE, TOOL_CALL}.issubset(available)
            if not has_direct and not has_bridge:
                raise SmokeFailure(
                    f"Hermes exposed neither native Everett tools nor the tool_search/describe/call bridge (available={sorted(n for n in available if n)})"
                )
            action = _next_call(marker, messages, available=available)
            if action is None:
                self._send_completion(payload, content="local model completed scripted tool flow")
            else:
                self._send_completion(payload, function=action)
        except Exception as exc:
            self.server.state.failures.append(f"{type(exc).__name__}: {exc}")
            body = str(exc).encode()
            self.send_response(500)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    def _send_completion(self, payload: dict, *, content: str | None = None,
                         function: tuple[str, dict] | None = None) -> None:
        completion_id = f"chatcmpl-{self.server.state.next_call_id()}"
        created = int(time.time())
        model = payload.get("model", "everett-hermes-smoke")
        if function is None:
            message = {"role": "assistant", "content": content}
            finish_reason = "stop"
        else:
            name, arguments = function
            if name not in {tool.get("function", {}).get("name") for tool in payload.get("tools", [])}:
                raise SmokeFailure(f"Hermes did not expose bridge tool {name}")
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [{
                    "id": self.server.state.next_call_id(),
                    "type": "function",
                    "function": {"name": name, "arguments": json.dumps(arguments)},
                }],
            }
            finish_reason = "tool_calls"
        if payload.get("stream"):
            self._send_stream(completion_id, created, model, message, finish_reason)
            return
        body = json.dumps({
            "id": completion_id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_stream(self, completion_id: str, created: int, model: str, message: dict,
                     finish_reason: str) -> None:
        chunks = []
        if message.get("tool_calls"):
            call = message["tool_calls"][0]
            function = call["function"]
            chunks.extend([
                {"role": "assistant", "tool_calls": [{
                    "index": 0, "id": call["id"], "type": "function",
                    "function": {"name": function["name"], "arguments": ""},
                }]},
                {"tool_calls": [{
                    "index": 0, "function": {"arguments": function["arguments"]},
                }]},
            ])
        else:
            chunks.append({"role": "assistant", "content": message.get("content", "")})
        events = [
            {"id": completion_id, "object": "chat.completion.chunk", "created": created,
             "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]}
            for delta in chunks
        ]
        events.append({
            "id": completion_id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}],
        })
        body = "".join(f"data: {json.dumps(event)}\n\n" for event in events) + "data: [DONE]\n\n"
        encoded = body.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, *_args) -> None:
        pass


class IssuerHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path != "/v2/relay/whoami" or self.headers.get("Authorization") != f"Bearer {FIXTURE_TOKEN}":
            self.send_error(401)
            return
        body = json.dumps({"result": {"owner": "fixture-owner", "deviceId": "fixture-device", "role": "member"}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args) -> None:
        pass


def _start_server(handler, state=None) -> tuple[LoopbackHTTPServer, threading.Thread]:
    server = LoopbackHTTPServer(("127.0.0.1", 0), handler)
    if state is not None:
        server.state = state
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread


def _stop_server(server: LoopbackHTTPServer, thread: threading.Thread) -> None:
    try:
        server.shutdown()
    finally:
        try:
            server.server_close()
        finally:
            thread.join(timeout=5)


def _cleanup_profile(
    hermes: Path,
    env: dict[str, str] | None,
    home: Path | None,
    installed: bool,
    servers: tuple[tuple[LoopbackHTTPServer, threading.Thread], ...],
) -> None:
    try:
        if env is not None and home is not None:
            try:
                _run(hermes, ["gateway", "stop"], env, home, check=False, timeout=30)
            finally:
                if installed:
                    _run(hermes, ["plugins", "remove", "everett-hermes"], env, home,
                         check=False, timeout=60)
    finally:
        try:
            _stop_server(*servers[0])
        finally:
            _stop_server(*servers[1])


def _make_install_source(root: Path) -> str:
    git = shutil.which("git")
    if not git:
        raise SmokeFailure("git is required to exercise Hermes's file:// plugin installer")
    repository = root / "source"
    package = repository / "plugins" / "everett-hermes"
    package.mkdir(parents=True)
    files = (
        "plugin.yaml",
        "__init__.py",
        "_everett.py",
        "_inbox.py",
        "_wake.py",
        "_commands.py",
        "plugin.json",
        "mcp.json",
        "README.md",
        "skills/everett-hermes/SKILL.md",
    )
    for name in files:
        destination = package / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(PLUGIN_ROOT / name, destination)
    subprocess.run([git, "-C", str(repository), "init", "--quiet"], check=True, capture_output=True, text=True)
    subprocess.run([git, "-C", str(repository), "add", "--", *[f"plugins/everett-hermes/{name}" for name in files]],
                   check=True, capture_output=True, text=True)
    subprocess.run([git, "-C", str(repository), "-c", "user.name=Everett fixture", "-c", "user.email=fixture@example.invalid",
                    "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "smoke fixture"],
                   check=True, capture_output=True, text=True)
    return repository.as_uri() + "#plugins/everett-hermes"


def _clean_environment(home: Path, hermes_home: Path, temp: Path,
                       hermes: Path, everett: Path, issuer_url: str) -> dict[str, str]:
    # The manifest starts "everett" by name. Pin that name to the requested
    # artifact even when Hermes has a different Everett installed beside it.
    bin_dir = temp / "fixture-bin"
    bin_dir.mkdir()
    (bin_dir / "everett").symlink_to(everett)
    path_parts = [str(bin_dir), str(hermes.parent), os.environ.get("PATH", os.defpath)]
    environment = {
        "PATH": os.pathsep.join(path_parts),
        "HOME": str(home),
        "HERMES_HOME": str(hermes_home),
        "EVERETT_HOME": str(home),
        "EVERETT_AUTH_PROVIDER": "cloudflare",
        "EVERETT_AUTH_BASE_URL": issuer_url,
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "TMPDIR": str(temp),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TZ": "UTC",
        "CI": "1",
        "NO_COLOR": "1",
        "NO_PROXY": "localhost,127.0.0.1",
        "no_proxy": "localhost,127.0.0.1",
    }
    for key in ("PYTHONPATH", "HERMES_RUNTIME_DIR"):
        if key in os.environ:
            environment[key] = os.environ[key]
    return environment


def _run(hermes: Path, args: list[str], env: dict[str, str], cwd: Path,
         *, check: bool = True, timeout: int = 120) -> subprocess.CompletedProcess:
    result = subprocess.run([str(hermes), *args], env=env, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    if check and result.returncode:
        command = " ".join([str(hermes), *args])
        raise SmokeFailure(f"{command} exited {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


def _plugin_rows(hermes: Path, env: dict[str, str], home: Path) -> list[dict]:
    result = _run(hermes, ["plugins", "list", "--json", "--no-bundled"], env, home)
    try:
        rows = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise SmokeFailure(f"Hermes plugin list did not return JSON: {exc}") from exc
    if not isinstance(rows, list):
        raise SmokeFailure("Hermes plugin list did not return a plugin array")
    return rows


def _tail(text: str, limit: int = 4000) -> str:
    return text[-limit:].strip()


def _failure_with_diagnostics(exc: SmokeFailure, state: ModelState, chat_results: list[tuple[str, subprocess.CompletedProcess]],
                              hermes_home: Path) -> SmokeFailure:
    diagnostics = []
    for label, result in chat_results:
        for channel in ("stdout", "stderr"):
            if tail := _tail(getattr(result, channel)):
                diagnostics.append(f"{label} {channel}:\n{tail}")
    mcp_log = hermes_home / "logs" / "mcp-stderr.log"
    if mcp_log.is_file():
        diagnostics.append(
            f"Hermes MCP stderr log:\n{_tail(mcp_log.read_text(encoding='utf-8', errors='replace'))}")
    traces = json.dumps({
        "signed_out_requests": _request_trace(state.snapshot(SIGNED_OUT)),
        "signed_in_requests": _request_trace(state.snapshot(SIGNED_IN)),
    }, indent=2)
    details = "\n".join(diagnostics)
    details = f"{details}\nmodel request trace:\n{traces}" if details else traces
    return SmokeFailure(f"{exc}\n{details}")


def _write_fixture_account(home: Path, issuer_url: str) -> None:
    credentials = home / ".everett" / "cloudflare-auth.json"
    credentials.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(credentials, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
        json.dump({
            "issuer": issuer_url,
            "owner": "fixture-owner",
            "agent_id": "fixture-device",
            "device_name": "Hermes smoke fixture",
            "token": FIXTURE_TOKEN,
            "expires_at": int(time.time() * 1000) + 120_000,
        }, stream)


def _tool_calls(requests: list[dict]) -> list[tuple[str, dict]]:
    calls = []
    for payload in requests:
        calls.extend(_history_calls(payload.get("messages", [])))
    return calls


def _tool_results(requests: list[dict]) -> dict[str, str]:
    results = {}
    for payload in requests:
        messages = payload.get("messages", [])
        for index, message in enumerate(messages):
            if message.get("role") != "assistant":
                continue
            for call in message.get("tool_calls", []):
                function = call.get("function", {})
                if not isinstance(function, dict):
                    continue
                fname = function.get("name")
                output = []
                for following in messages[index + 1:]:
                    if following.get("role") == "assistant":
                        break
                    if following.get("role") == "tool":
                        output.append(_content_text(following.get("content")))
                if not output:
                    continue
                if fname == TOOL_CALL:
                    arguments = _arguments(function.get("arguments", {}))
                    entries = arguments.get("calls", [])
                    if isinstance(entries, list) and len(entries) == 1 and isinstance(entries[0], dict):
                        results[entries[0].get("name", "")] = "\n".join(output)
                elif fname in {WHOAMI, LIST}:
                    results[fname] = "\n".join(output)
    return results


def _request_trace(requests: list[dict]) -> list[dict]:
    trace = []
    for payload in requests:
        calls = []
        outputs = []
        for message in payload.get("messages", []):
            if message.get("role") == "assistant":
                for call in message.get("tool_calls", []):
                    function = call.get("function", {})
                    if isinstance(function, dict) and function.get("name") in {
                        TOOL_SEARCH, TOOL_DESCRIBE, TOOL_CALL,
                    }:
                        calls.append({
                            "name": function["name"],
                            "arguments": _arguments(function.get("arguments", {})),
                        })
            elif message.get("role") in {"tool", "function"}:
                outputs.append({
                    "role": message.get("role"),
                    "name": message.get("name"),
                    "tool_call_id": message.get("tool_call_id"),
                    "content": _content_text(message.get("content"))[:1000],
                })
        trace.append({"bridge_calls": calls, "tool_outputs": outputs})
    return trace


def _embedded_json(text: str):
    decoder = json.JSONDecoder()
    index = 0
    while index < len(text):
        if text[index] not in "[{":
            index += 1
            continue
        try:
            value, length = decoder.raw_decode(text, index)
        except json.JSONDecodeError:
            index += 1
            continue
        yield value
        index += length


def _objects(value):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from _objects(child)
    elif isinstance(value, list):
        for child in value:
            yield from _objects(child)


def _response_values(text: str, label: str) -> list:
    values = list(_embedded_json(text))
    if not values:
        raise SmokeFailure(f"{label} did not return a JSON result")
    return values


def _error_messages(value) -> list[str]:
    messages = []
    for item in _objects(value):
        message = item.get("error")
        if isinstance(message, str) and message:
            messages.append(message)
    return messages


def _contains_tool_error(value) -> bool:
    if isinstance(value, dict):
        return value.get("isError") is True or any(
            (key == "error" and child not in (None, "", False)) or _contains_tool_error(child)
            for key, child in value.items()
        )
    if isinstance(value, list):
        return any(_contains_tool_error(child) for child in value)
    return False


def _require_signed_out_account_error(text: str) -> str:
    values = _response_values(text, "signed-out whoami")
    messages = [message for value in values for message in _error_messages(value)]
    if not any(ACCOUNT_ERROR in message for message in messages):
        raise SmokeFailure("signed-out whoami did not return Everett's account-required error")
    if any(ACCOUNT_ERROR not in message for message in messages):
        raise SmokeFailure("signed-out whoami returned an unexpected tool error")
    return next(message for message in messages if ACCOUNT_ERROR in message)


def _require_verified_whoami(text: str) -> tuple[dict, dict]:
    values = _response_values(text, "loopback-account whoami")
    if any(_contains_tool_error(value) for value in values):
        raise SmokeFailure("loopback-account whoami returned a tool error")
    if any(item.get("verified") is False for value in values for item in _objects(value)):
        raise SmokeFailure("loopback-account whoami explicitly reported an unverified identity")
    for value in values:
        for response in _objects(value):
            identity = response.get("identity")
            if not isinstance(identity, dict):
                continue
            account = identity.get("account")
            if not isinstance(account, dict) or account.get("status") != "verified":
                continue
            return response, identity
    raise SmokeFailure("loopback-account whoami did not verify the synthetic fixture account")


def _require_hermes_sessions(text: str, expected_session_id: str) -> tuple[list[dict], list[dict]]:
    values = _response_values(text, "loopback-account everett_ls")
    if any(_contains_tool_error(value) for value in values):
        raise SmokeFailure("loopback-account everett_ls returned a tool error")
    sessions = next((
        response["sessions"]
        for value in values
        for response in _objects(value)
        if "sessions" in response
    ), None)
    if not isinstance(sessions, list):
        raise SmokeFailure("loopback-account everett_ls did not return a sessions array")
    hermes_sessions = [
        session for session in sessions
        if isinstance(session, dict)
        and session.get("harness") == "hermes"
        and isinstance(session.get("id"), str)
        and session["id"]
    ]
    if not any(session["id"] == expected_session_id for session in hermes_sessions):
        raise SmokeFailure("everett_ls did not include the actual Hermes oneshot session from state.db")
    return sessions, hermes_sessions


def _create_clean_profile(temp: Path, hermes: Path, everett: Path, issuer_url: str) -> tuple[Path, Path, dict[str, str]]:
    home = temp / "home"
    home.mkdir()
    hermes_home = temp / "hermes-profile"
    hermes_home.mkdir()
    env = _clean_environment(home, hermes_home, temp, hermes, everett, issuer_url)
    _run(hermes, ["pm", "install"], env, hermes.parent, timeout=300)
    print("Hermes PM: isolated runtime dependencies installed")
    return home, hermes_home, env


def _configure_model(hermes: Path, env: dict[str, str], home: Path, base_url: str) -> None:
    for key, value in (
        ("model.provider", "custom"),
        ("model.base_url", base_url),
        ("model.default", "everett-hermes-smoke"),
    ):
        _run(hermes, ["config", "set", key, value], env, home)


def _chat(hermes: Path, env: dict[str, str], home: Path, base_url: str, marker: str,
          request: str) -> subprocess.CompletedProcess:
    return _run(hermes, [
        "chat", "-Q", "-q", f"{marker}: {request}",
        "--provider", "custom", "--model", "everett-hermes-smoke",
        "--base-url", base_url, "--api-key", "smoke-only-no-secret",
    ], env, home, timeout=180)


def _read_oneshot_session_id(hermes_home: Path, chat: subprocess.CompletedProcess) -> str:
    session_ids = [
        line.strip().partition(":")[2].strip()
        for output in (chat.stdout, chat.stderr)
        for line in output.splitlines()
        if line.strip().startswith("session_id:")
    ]
    if not session_ids or not session_ids[-1]:
        raise SmokeFailure("Hermes chat did not report its actual session id")
    session_id = session_ids[-1]
    database = hermes_home / "state.db"
    try:
        connection = sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)
        try:
            row = connection.execute(
                "SELECT source FROM sessions WHERE id = ?", (session_id,)
            ).fetchone()
        finally:
            connection.close()
    except sqlite3.Error as exc:
        raise SmokeFailure(f"could not read Hermes state.db: {exc}") from exc
    if row is None or row[0] != "oneshot":
        raise SmokeFailure("Hermes chat session id is not an actual oneshot row in state.db")
    return session_id


def _read_hermes_tool_result(hermes_home: Path, session_id: str, tool_name: str,
                             expected_arguments: dict | None = None) -> str:
    database = hermes_home / "state.db"
    try:
        connection = sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)
        try:
            rows = connection.execute(
                "SELECT role, tool_calls, tool_name, content FROM messages "
                "WHERE session_id = ? AND (tool_calls IS NOT NULL OR tool_name = ?) ORDER BY id",
                (session_id, tool_name),
            ).fetchall()
        finally:
            connection.close()
    except sqlite3.Error as exc:
        raise SmokeFailure(f"could not read Hermes tool results from state.db: {exc}") from exc

    arguments = []
    result = None
    for role, raw_calls, stored_tool_name, content in rows:
        if role == "assistant" and raw_calls:
            try:
                calls = json.loads(raw_calls)
            except json.JSONDecodeError as exc:
                raise SmokeFailure("Hermes state.db stored malformed tool calls") from exc
            for call in calls:
                function = call.get("function", {})
                fname = function.get("name")
                if fname == TOOL_CALL:
                    bridge_arguments = _arguments(function.get("arguments", {}))
                    arguments.extend(
                        target.get("arguments", {})
                        for target in bridge_arguments.get("calls", [])
                        if isinstance(target, dict) and target.get("name") == tool_name
                    )
                elif fname == tool_name:
                    arguments.append(_arguments(function.get("arguments", {})))
        if role == "tool" and stored_tool_name == tool_name and isinstance(content, str):
            result = content

    if not arguments:
        raise SmokeFailure(f"Hermes did not call {tool_name} in its state.db session")
    if expected_arguments is not None and expected_arguments not in arguments:
        raise SmokeFailure(f"Hermes called {tool_name} with unexpected arguments")
    if not result:
        raise SmokeFailure(f"Hermes did not store the {tool_name} result in state.db")
    for value in _embedded_json(result):
        if isinstance(value, dict) and "result" in value:
            nested = value["result"]
            return nested if isinstance(nested, str) else json.dumps(nested)
    return result


def _assert_tool_flow(state: ModelState, marker: str, expected: set[str]) -> list[dict]:
    if state.failures:
        raise SmokeFailure("local model fixture failed: " + "; ".join(state.failures))
    requests = state.snapshot(marker)
    calls = _tool_calls(requests)
    names = {name for name, _ in calls}
    if expected.issubset(names):
        return requests
    described = {
        tool_name
        for name, arguments in calls if name == TOOL_DESCRIBE
        for tool_name in ([arguments.get("names")] if isinstance(arguments.get("names"), str)
                          else arguments.get("names", []))
    }
    invoked = {
        entry.get("name")
        for name, arguments in calls if name == TOOL_CALL
        for entry in arguments.get("calls", []) if isinstance(entry, dict)
    }
    if not {TOOL_SEARCH, TOOL_DESCRIBE, TOOL_CALL}.issubset(names) or not expected.issubset(described & invoked):
        raise SmokeFailure(f"Hermes did not invoke expected Everett tools for {marker}")
    return requests


def _assert_signed_out_result(state: ModelState) -> None:
    requests = _assert_tool_flow(state, SIGNED_OUT, {WHOAMI})
    signed_out_error = _tool_results(requests).get(WHOAMI, "")
    if not signed_out_error:
        raise SmokeFailure("signed-out whoami result was missing")
    _require_signed_out_account_error(signed_out_error)


def _assert_client_results(state: ModelState, expected_session_id: str, hermes_home: Path,
                           chat: subprocess.CompletedProcess) -> tuple[int, bool, str]:
    _assert_tool_flow(state, SIGNED_IN, {WHOAMI})
    session_id = _read_oneshot_session_id(hermes_home, chat)
    whoami_text = _read_hermes_tool_result(hermes_home, session_id, WHOAMI, {})
    list_text = _read_hermes_tool_result(
        hermes_home, session_id, LIST, {"harness": "hermes", "all": True})
    whoami, identity = _require_verified_whoami(whoami_text)
    _, sessions = _require_hermes_sessions(list_text, expected_session_id)
    caller = identity.get("session")
    caller_id = caller.get("id") if isinstance(caller, dict) else None
    if not isinstance(caller_id, str):
        caller_id = whoami.get("session_id")
    if caller_id != session_id:
        raise SmokeFailure(f"Everett caller identity {caller_id!r} did not match Hermes session {session_id!r}")
    return len(sessions), True, session_id


def _resolve_executable(value: str, label: str) -> Path:
    candidate = Path(value).expanduser()
    if candidate.is_file():
        return candidate.resolve(strict=True)
    found = shutil.which(value)
    if found:
        return Path(found).resolve(strict=True)
    raise SmokeFailure(f"{label} executable was not found: {value}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--everett-binary", required=True, help="Path to the Everett executable to test")
    parser.add_argument("--hermes-executable", required=True, help="Path to the Hermes Agent CLI executable")
    parser.add_argument("--upstream", action="store_true", help="Install the published repository subdirectory from GitHub")
    arguments = parser.parse_args()
    everett = _resolve_executable(arguments.everett_binary, "Everett")
    hermes = _resolve_executable(arguments.hermes_executable, "Hermes")
    with tempfile.TemporaryDirectory(prefix="everett-hermes-smoke-") as directory:
        temp = Path(directory)
        state = ModelState()
        model_server, model_thread = _start_server(ModelHandler, state)
        issuer_server, issuer_thread = _start_server(IssuerHandler)
        issuer_url = f"http://127.0.0.1:{issuer_server.server_port}"
        home = hermes_home = None
        env = None
        installed = False
        try:
            home, hermes_home, env = _create_clean_profile(temp, hermes, everett, issuer_url)
            source = "raitoxlol/everett#plugins/everett-hermes" if arguments.upstream else _make_install_source(temp)
            _run(hermes, ["plugins", "doctor", str(PLUGIN_ROOT), "--ci"], env, home)
            print("Hermes plugin doctor: local checkout passed")
            _run(hermes, ["plugins", "validate", str(PLUGIN_ROOT)], env, home)
            print("Hermes plugin validate: local checkout passed (host-ready & safe)")
            _run(hermes, ["plugins", "install", source, "--no-enable"], env, home)
            installed = True
            _run(hermes, ["plugins", "enable", "everett-hermes"], env, home)
            rows = _plugin_rows(hermes, env, home)
            if not any(row.get("name") == "everett-hermes" and row.get("status") == "enabled"
                       for row in rows if isinstance(row, dict)):
                raise SmokeFailure("Hermes plugin manager did not report everett-hermes enabled")
            model_url = f"http://127.0.0.1:{model_server.server_port}/v1"
            _configure_model(hermes, env, home, model_url)
            signed_out_chat = _chat(
                hermes, env, home, model_url, SIGNED_OUT,
                "Use tool_search, tool_describe, and tool_call to check Everett whoami.")
            try:
                _assert_signed_out_result(state)
            except SmokeFailure as exc:
                raise _failure_with_diagnostics(
                    exc, state, [("signed-out Hermes", signed_out_chat)], hermes_home) from exc
            expected_session_id = _read_oneshot_session_id(hermes_home, signed_out_chat)
            print(f"Hermes state.db oneshot session: {expected_session_id}")
            _write_fixture_account(home, issuer_url)
            signed_in_chat = _chat(
                hermes, env, home, model_url, SIGNED_IN,
                "Use tool_search, tool_describe, and tool_call for Everett whoami and a Hermes-only session list.")
            try:
                session_count, identity_bound, signed_in_session_id = _assert_client_results(
                    state, expected_session_id, hermes_home, signed_in_chat)
            except SmokeFailure as exc:
                raise _failure_with_diagnostics(
                    exc, state,
                    [("signed-out Hermes", signed_out_chat), ("signed-in Hermes", signed_in_chat)],
                    hermes_home) from exc
            auto_card = home / ".everett" / "cards" / f"{signed_in_session_id}.md"
            if not auto_card.is_file() or not auto_card.read_text(encoding="utf-8").startswith("<!-- everett:auto -->\n"):
                raise SmokeFailure(f"native Hermes plugin did not write fallback auto-card at {auto_card}")
            _run(hermes, ["plugins", "remove", "everett-hermes"], env, home)
            installed = False
            if any(row.get("name") == "everett-hermes" for row in _plugin_rows(hermes, env, home)
                   if isinstance(row, dict)):
                raise SmokeFailure("Hermes plugin manager still listed everett-hermes after removal")
            print(f"Hermes plugin manager: {source} installed, enabled, and removed")
            print("Hermes client: native Everett tools executed end-to-end")
            print(f"Signed-out whoami: {ACCOUNT_ERROR} Run `everett login` in this Everett profile, then retry.")
            print("Loopback fixture: whoami account verified; everett_ls returned sessions")
            print(f"Hermes sessions listed: {session_count}")
            print(f"Hermes fallback card: {auto_card.name} written on session end")
            if identity_bound:
                print("Hermes caller identity: bound ID matches the calling chat and its state.db row")
        finally:
            _cleanup_profile(
                hermes, env, home, installed,
                ((model_server, model_thread), (issuer_server, issuer_thread)),
            )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SmokeFailure as exc:
        raise SystemExit(str(exc))
