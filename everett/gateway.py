"""Optional owner-bound MCP gateway. Rust remains the tool backend."""
from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import math
import os
import re
import sys
from dataclasses import dataclass
from datetime import timedelta
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from .inbox import HUMAN

AGENT_ID = re.compile(r"[A-Za-z0-9_-]{1,100}\Z")
DESTINATION_ID = re.compile(r"[A-Za-z0-9._-]{1,128}\Z")
PROJECT = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
MAX_BODY = 64 * 1024
MAX_RESULT = 256 * 1024
MAX_INBOX = 8 * 1024 * 1024
TOOLS = frozenset({"everett_core", "everett_inbox", "everett_send", "everett_card", "everett_whoami"})
EXACT_DELIVERY_CAPABILITY = "everettExactDestinationIds"


class BoundaryError(ValueError):
    pass


@dataclass(frozen=True)
class Binding:
    agent_id: str
    harness: str
    cwd: Path
    project: str
    destinations: frozenset[str]
    allow_shared_core: bool
    backend_command: tuple[str, ...]
    home: Path
    max_workers: int = 2
    timeout: float = 20
    allowed_hosts: tuple[str, ...] = ("127.0.0.1:8788",)
    allowed_origins: tuple[str, ...] = ()

    def __post_init__(self):
        if not isinstance(self.agent_id, str) or not AGENT_ID.fullmatch(self.agent_id):
            raise BoundaryError("agent_id must match [A-Za-z0-9_-]{1,100}")
        if self.agent_id.lower() in (HUMAN.lower(), "live"):
            raise BoundaryError("agent_id cannot bind a reserved human or live inbox")
        if self.harness not in ("openai-dot", "grok-bot"):
            raise BoundaryError("harness must be openai-dot or grok-bot")
        if not isinstance(self.project, str) or not PROJECT.fullmatch(self.project):
            raise BoundaryError("project must be a fixed lowercase project slug")
        if not self.cwd.is_absolute() or not self.cwd.is_dir() or not self.home.is_absolute():
            raise BoundaryError("cwd must be an existing absolute directory; home must be absolute")
        if type(self.allow_shared_core) is not bool:
            raise BoundaryError("allow_shared_core must explicitly grant or deny global plus project core")
        if len(self.destinations) > 64 or any(
            not isinstance(sid, str) or not DESTINATION_ID.fullmatch(sid) or sid in (".", "..", self.agent_id)
            for sid in self.destinations
        ):
            raise BoundaryError("destinations must contain full valid IDs, excluding this agent")
        if not self.backend_command:
            raise BoundaryError("backend_command must not be empty")
        command = next(iter(self.backend_command))
        if (len(self.backend_command) > 8
                or any(not isinstance(arg, str) or not arg or "\x00" in arg for arg in self.backend_command)
                or not Path(command).is_absolute()):
            raise BoundaryError("backend_command must be a trusted absolute executable and arguments")
        if type(self.max_workers) is not int or not 1 <= self.max_workers <= 8:
            raise BoundaryError("max_workers must be between 1 and 8")
        if (isinstance(self.timeout, bool) or not isinstance(self.timeout, (int, float))
                or not math.isfinite(self.timeout) or not 1 <= self.timeout <= 30):
            raise BoundaryError("timeout must be between 1 and 30 seconds")
        if not self.allowed_hosts:
            raise BoundaryError("at least one exact HTTP Host is required")
        for host in self.allowed_hosts:
            parsed = urlsplit("https://" + host)
            if (not parsed.hostname or parsed.netloc != host or parsed.path or parsed.query or parsed.fragment
                    or parsed.username or parsed.password or "*" in host or not host.isascii()
                    or any(char.isspace() or ord(char) < 32 for char in host)):
                raise BoundaryError("allowed_hosts must be exact authorities without wildcards")
            try:
                parsed.port
            except ValueError as exc:
                raise BoundaryError("invalid Host port") from exc
        for origin in self.allowed_origins:
            parsed = urlsplit(origin)
            if (parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.path
                    or parsed.query or parsed.fragment or parsed.username or parsed.password or "*" in origin
                    or not origin.isascii() or any(char.isspace() or ord(char) < 32 for char in origin)):
                raise BoundaryError("allowed_origins must be exact http(s) origins")

    @classmethod
    def load(cls, path: Path) -> Binding:
        with path.open("rb") as handle:
            raw = handle.read(16 * 1024 + 1)
        if len(raw) > 16 * 1024:
            raise BoundaryError("binding configuration exceeds 16 KiB")
        data = json.loads(raw)
        required = {"agent_id", "harness", "cwd", "project", "destinations", "allow_shared_core", "backend_command"}
        optional = {"max_workers", "timeout", "allowed_hosts", "allowed_origins"}
        if not isinstance(data, dict) or not required <= data.keys() or data.keys() - required - optional:
            raise BoundaryError("binding configuration has missing or unknown fields")
        for key in ("destinations", "backend_command", "allowed_hosts", "allowed_origins"):
            if key in data and (not isinstance(data[key], list) or any(not isinstance(s, str) for s in data[key])):
                raise BoundaryError(f"{key} must be an array of strings")
        if not isinstance(data["cwd"], str):
            raise BoundaryError("cwd must be a string")
        data["cwd"] = Path(data["cwd"])
        data["destinations"] = frozenset(data["destinations"])
        data["backend_command"] = tuple(data["backend_command"])
        for key in ("allowed_hosts", "allowed_origins"):
            if key in data:
                data[key] = tuple(data[key])
        home = Path(os.environ.get("EVERETT_HOME") or Path.home()).resolve()
        return cls(home=home, **data)

    def environment(self) -> dict[str, str]:
        return {
            "HOME": str(self.home), "EVERETT_HOME": str(self.home),
            "EVERETT_SESSION_ID": self.agent_id, "EVERETT_HARNESS_NAME": self.harness,
            "EVERETT_NOTIFY": "none", "EVERETT_ROUTER": "local", "EVERETT_SEND": "inbox",
            "EVERETT_HOPS": "0", "EVERETT_GATEWAY_EXACT_IDS": "1",
        }

    def reply_destination(self, message_id: str) -> str:
        if not isinstance(message_id, str) or not message_id or len(message_id) > 128:
            raise BoundaryError("invalid reply message ID")
        path = self.home / ".everett" / "inbox" / (self.agent_id + ".jsonl")
        try:
            with path.open("rb") as handle:
                raw = handle.read(MAX_INBOX + 1)
        except OSError as exc:
            raise BoundaryError("reply message is not in this agent's inbox") from exc
        if len(raw) > MAX_INBOX:
            raise BoundaryError("inbox exceeds reply authorization scan limit")
        matches = []
        for line in raw.splitlines():
            if not line:
                continue
            try:
                message = json.loads(line)
            except (ValueError, UnicodeError) as exc:
                raise BoundaryError("cannot authorize reply from malformed inbox") from exc
            if isinstance(message, dict) and message.get("id") == message_id:
                matches.append(message)
        if len(matches) != 1 or matches[0].get("to") != self.agent_id:
            raise BoundaryError("reply message is not uniquely addressed to this agent")
        sender = matches[0].get("from")
        if not isinstance(sender, str) or sender not in self.destinations:
            raise BoundaryError("reply destination is not authorized")
        return sender

    def authorize(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        if name not in TOOLS or (name == "everett_core" and not self.allow_shared_core):
            raise BoundaryError("tool is not allowed")
        if not isinstance(arguments, dict) or len(json.dumps(arguments).encode()) > MAX_BODY:
            raise BoundaryError("tool arguments exceed boundary limits")
        if "session_id" in arguments and arguments["session_id"] != self.agent_id:
            raise BoundaryError("session_id differs from the immutable binding")
        allowed = {
            "everett_core": {"project"}, "everett_whoami": set(),
            "everett_inbox": {"session_id", "peek"},
            "everett_card": {"session_id", "what", "state", "next"},
            "everett_send": {"session_id", "text", "to", "reply_to"},
        }[name]
        if arguments.keys() - allowed:
            raise BoundaryError("unexpected argument; spawning, resuming, routing and cwd changes are not allowed")
        args = dict(arguments)
        if name == "everett_core":
            if args.get("project", self.project) != self.project:
                raise BoundaryError("project differs from the fixed binding")
            args["project"] = self.project
        if name in ("everett_inbox", "everett_card", "everett_send"):
            args["session_id"] = self.agent_id
        if name == "everett_inbox":
            if "peek" in args and type(args["peek"]) is not bool:
                raise BoundaryError("peek must be a boolean")
        if name == "everett_card":
            for key in ("what", "state", "next"):
                if not isinstance(args.get(key), str) or not args[key].strip() or len(args[key]) > 2000:
                    raise BoundaryError("card fields must be nonempty strings of at most 2000 characters")
        if name == "everett_send":
            if not isinstance(args.get("text"), str) or not args["text"].strip() or len(args["text"]) > 16000:
                raise BoundaryError("text must be a nonempty string of at most 16000 characters")
            if ("to" in args) == ("reply_to" in args):
                raise BoundaryError("provide exactly one authorized full destination ID or owned reply_to")
            if "reply_to" in args:
                self.reply_destination(args["reply_to"])
            elif not isinstance(args["to"], str) or args["to"] not in self.destinations:
                raise BoundaryError("destination is not an authorized full ID")
            args.update(mode="inbox", wait=0, spawn=False)
        return args


def tool_definitions(binding: Binding) -> list[dict[str, Any]]:
    own = {"type": "string", "enum": [binding.agent_id]}
    text = {"type": "string", "minLength": 1, "maxLength": 16000}
    specs = {
        "everett_whoami": ("Read this connection's logical owner-scoped identity, not vendor-attested Bot identity.", {}, []),
        "everett_core": ("Read owner-wide GLOBAL shared core plus this connection's fixed project core.",
                         {"project": {"type": "string", "enum": [binding.project]}}, []),
        "everett_inbox": ("Read only this agent's inbox. By default mark messages delivered; use peek=true to retain them.",
                          {"session_id": own, "peek": {"type": "boolean"}}, []),
        "everett_send": ("Queue an inbox-only message to an authorized full ID, or reply to an owned message. No automatic bot wake-up.",
                         {"session_id": own, "text": text, "to": {"type": "string", "enum": sorted(binding.destinations)},
                          "reply_to": {"type": "string", "minLength": 1, "maxLength": 128}}, ["text"]),
        "everett_card": ("Update only this agent's session card.",
                         {"session_id": own, **{key: {"type": "string", "minLength": 1, "maxLength": 2000}
                                                for key in ("what", "state", "next")}}, ["what", "state", "next"]),
    }
    return [
        {"name": name, "description": description,
         "inputSchema": {"type": "object", "properties": properties, "required": required, "additionalProperties": False},
         "annotations": {"readOnlyHint": name in ("everett_core", "everett_whoami"), "openWorldHint": False}}
        for name, (description, properties, required) in specs.items()
        if name != "everett_core" or binding.allow_shared_core
    ]


def _data(result) -> dict[str, Any]:
    if result.isError:
        raise BoundaryError("backend rejected the tool call")
    if len(result.model_dump_json().encode()) > MAX_RESULT:
        raise BoundaryError("backend result exceeds 256 KiB")
    if not isinstance(result.structuredContent, dict):
        raise BoundaryError("backend must return structured tool results")
    return result.structuredContent


class Gateway:
    def __init__(self, binding: Binding):
        import anyio

        self.binding = binding
        self.workers = anyio.CapacityLimiter(binding.max_workers)

    async def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        import anyio
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client

        args = self.binding.authorize(name, arguments)
        try:
            self.workers.acquire_nowait()
        except anyio.WouldBlock as exc:
            raise BoundaryError("gateway workers busy; retry later") from exc
        try:
            with anyio.fail_after(self.binding.timeout):
                parameters = StdioServerParameters(
                    command=self.binding.backend_command[0], args=list(self.binding.backend_command[1:]),
                    env=self.binding.environment(), cwd=str(self.binding.cwd),
                )
                with open(os.devnull, "w") as errlog:
                    async with stdio_client(parameters, errlog=errlog) as (read, write):
                        async with ClientSession(read, write, read_timeout_seconds=timedelta(seconds=self.binding.timeout)) as session:
                            initialized = await session.initialize()
                            if name == "everett_send" and "to" in args:
                                capabilities = initialized.capabilities.experimental or {}
                                exact_ids = capabilities.get(EXACT_DELIVERY_CAPABILITY)
                                if not isinstance(exact_ids, dict) or exact_ids.get("enforced") is not True:
                                    raise BoundaryError("backend lacks strict exact-ID delivery; owned-message replies remain available")
                            if name == "everett_send" and "reply_to" in args:
                                self.binding.reply_destination(args["reply_to"])
                            data = _data(await session.call_tool(name, args))
                            if name == "everett_core":
                                return {"core": data["core"]}
                            if name == "everett_whoami" and data.get("session_id") != self.binding.agent_id:
                                raise BoundaryError("backend identity differs from binding")
                            return data
        except BoundaryError:
            raise
        except Exception as exc:
            raise BoundaryError("backend unavailable, incompatible, or request timed out") from exc
        finally:
            self.workers.release()


def build_server(binding: Binding):
    from mcp.server.lowlevel import Server
    from mcp.types import CallToolResult, TextContent, Tool

    gateway = Gateway(binding)
    server = Server("everett-owner-gateway", instructions=(
        "This connection is bound to one logical agent. Read its inbox for handoffs; reply using reply_to. "
        "Send only to authorized full IDs. No spawn/resume, automatic routing, or automatic bot wake-up. "
        "Shared core, when enabled, includes the owner's global core."
    ))

    @server.list_tools()
    async def list_tools():
        return [Tool.model_validate(tool) for tool in tool_definitions(binding)]

    @server.call_tool()
    async def call_tool(name: str, arguments: dict[str, Any]):
        try:
            return await gateway.call(name, arguments)
        except BoundaryError as exc:
            return CallToolResult(isError=True, content=[TextContent(type="text", text=str(exc))])

    return server


def build_http_app(binding: Binding, token: str):
    import anyio
    from mcp.server.auth.middleware.bearer_auth import BearerAuthBackend, RequireAuthMiddleware
    from mcp.server.auth.provider import AccessToken
    from mcp.server.streamable_http_manager import StreamableHTTPSessionManager
    from mcp.server.transport_security import TransportSecurityMiddleware, TransportSecuritySettings
    from starlette.applications import Starlette
    from starlette.middleware.authentication import AuthenticationMiddleware
    from starlette.requests import Request
    from starlette.responses import Response
    from starlette.routing import Route

    if not isinstance(token, str) or len(token) < 32 or len(token) > 512 or not token.isascii() or any(c.isspace() for c in token):
        raise BoundaryError("HTTP requires a bearer token of 32–512 ASCII characters without whitespace")

    class FixedTokenVerifier:
        def __init__(self):
            self.digest = hashlib.sha256(token.encode()).digest()

        async def verify_token(self, token: str):
            if len(token) > 512 or not hmac.compare_digest(hashlib.sha256(token.encode()).digest(), self.digest):
                return None
            return AccessToken(token=token, client_id=binding.agent_id, scopes=["everett"])

    security = TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=list(binding.allowed_hosts), allowed_origins=list(binding.allowed_origins),
    )
    manager = StreamableHTTPSessionManager(
        build_server(binding), stateless=True, json_response=True, security_settings=security,
    )
    limiter = anyio.CapacityLimiter(binding.max_workers * 2)

    class LimitedEndpoint:
        async def __call__(self, scope, receive, send):
            headers = [name.lower() for name, _ in scope["headers"]]
            if any(headers.count(key) > 1 for key in (b"host", b"origin", b"authorization", b"content-length", b"transfer-encoding")):
                await Response("Duplicate security header", 400)(scope, receive, send)
                return
            invalid = await TransportSecurityMiddleware(security).validate_request(Request(scope), scope["method"] == "POST")
            if invalid is not None:
                await invalid(scope, receive, send)
                return
            try:
                limiter.acquire_nowait()
            except anyio.WouldBlock:
                await Response("Gateway busy", 503)(scope, receive, send)
                return
            started = False

            async def tracked_send(message):
                nonlocal started
                if message["type"] == "http.response.start":
                    started = True
                await send(message)

            try:
                length = Request(scope).headers.get("content-length")
                if length is not None and (len(length) > 10 or not length.isdecimal() or int(length) > MAX_BODY):
                    await Response("Invalid or oversized body", 413)(scope, receive, send)
                    return
                chunks = []
                size = 0
                with anyio.fail_after(5):
                    while True:
                        message = await receive()
                        if message["type"] == "http.disconnect":
                            return
                        chunk = message.get("body", b"")
                        size += len(chunk)
                        if size > MAX_BODY:
                            await Response("Body exceeds 64 KiB", 413)(scope, receive, send)
                            return
                        chunks.append(chunk)
                        if not message.get("more_body", False):
                            break
                body = b"".join(chunks)
                consumed = False

                async def replay():
                    nonlocal consumed
                    if not consumed:
                        consumed = True
                        return {"type": "http.request", "body": body, "more_body": False}
                    return await receive()

                with anyio.fail_after(binding.timeout + 5):
                    await manager.handle_request(scope, replay, tracked_send)
            except TimeoutError:
                if not started:
                    await Response("Request timed out", 504)(scope, receive, send)
            finally:
                limiter.release()

    protected = AuthenticationMiddleware(
        RequireAuthMiddleware(LimitedEndpoint(), required_scopes=["everett"]),
        backend=BearerAuthBackend(FixedTokenVerifier()),
    )

    @asynccontextmanager
    async def lifespan(app):
        async with manager.run():
            yield

    return Starlette(routes=[Route("/mcp", protected, methods=["POST", "GET", "DELETE"])], lifespan=lifespan)


async def run_stdio(binding: Binding):
    from mcp.server.stdio import stdio_server

    server = build_server(binding)
    async with stdio_server() as (read, write):
        await server.run(read, write, server.create_initialization_options())


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("--transport", choices=("stdio", "http"), default="stdio")
    parser.add_argument("--port", type=int, default=8788)
    args = parser.parse_args(argv)
    try:
        binding = Binding.load(args.config)
        if args.transport == "stdio":
            import anyio

            anyio.run(run_stdio, binding)
        else:
            import uvicorn

            if not 1 <= args.port <= 65535:
                raise BoundaryError("port must be between 1 and 65535")
            app = build_http_app(binding, os.environ.get("EVERETT_GATEWAY_TOKEN", ""))
            uvicorn.run(app, host="127.0.0.1", port=args.port, proxy_headers=False,
                        access_log=False, limit_concurrency=binding.max_workers * 2 + 1,
                        timeout_keep_alive=5, timeout_graceful_shutdown=5)
    except ImportError:
        print("Gateway requires the optional extra: pip install 'everett-sessions[gateway]'", file=sys.stderr)
        return 2
    except (BoundaryError, OSError, ValueError, TypeError):
        print("Invalid gateway configuration or unavailable backend; see gateway documentation.", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
