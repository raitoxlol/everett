import sandbox  # noqa: F401  (must be first: isolates HOME)

import asyncio
import importlib.util
import json
import os
import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock

from everett.gateway import Binding, BoundaryError, Gateway, MAX_BODY, build_http_app, tool_definitions
from everett.inbox import HUMAN

SDK_AVAILABLE = importlib.util.find_spec("mcp") is not None
NATIVE_BACKEND = os.environ.get("EVERETT_GATEWAY_TEST_BINARY", "")
FIXTURE = Path(__file__).parent / "fixtures" / "gateway_backend.py"
TOKEN = "synthetic-gateway-test-token-not-a-real-secret"


class GatewaySandbox(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="everett-gateway-test-")
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        patch = mock.patch.dict(os.environ, {"HOME": str(self.home), "EVERETT_HOME": str(self.home)})
        patch.start()
        self.addCleanup(patch.stop)
        self.binding = Binding(
            agent_id="dot-one", harness="openai-dot", cwd=self.home, project="everett",
            destinations=frozenset({"agent-two"}), allow_shared_core=True,
            backend_command=(sys.executable, str(FIXTURE)), home=self.home,
        )

    def inbox(self, agent_id="dot-one", sender="agent-two", message_id="message-one", addressed_to=None):
        directory = self.home / ".everett" / "inbox"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / (agent_id + ".jsonl")
        path.write_text(json.dumps({"id": message_id, "to": addressed_to or agent_id,
                                    "from": sender, "text": "Synthetic handoff"}) + "\n")
        return path

    def records(self):
        path = self.home / "worker-calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


class GatewayPolicy(GatewaySandbox):
    def test_fixed_project_global_core_grant_and_tool_allowlist(self):
        self.assertEqual(self.binding.authorize("everett_core", {}), {"project": "everett"})
        with self.assertRaises(BoundaryError):
            self.binding.authorize("everett_core", {"project": "../other"})
        disabled = replace(self.binding, allow_shared_core=False)
        with self.assertRaises(BoundaryError):
            disabled.authorize("everett_core", {})
        self.assertNotIn("everett_core", {tool["name"] for tool in tool_definitions(disabled)})
        for name in ("everett_ls", "everett_route", "everett_learn", "everett_subscribe", "everett_event"):
            with self.subTest(name=name), self.assertRaises(BoundaryError):
                self.binding.authorize(name, {})

    def test_identity_is_immutable_on_inbox_card_send(self):
        for name in ("everett_inbox", "everett_card", "everett_send"):
            with self.subTest(name=name), self.assertRaises(BoundaryError):
                self.binding.authorize(name, {"session_id": "agent-two"})
        self.assertEqual(self.binding.authorize("everett_inbox", {})["session_id"], "dot-one")

    def test_delivery_must_be_explicit_allowlisted_full_id_and_inbox_only(self):
        safe = self.binding.authorize("everett_send", {"text": "Synthetic task", "to": "agent-two"})
        self.assertEqual((safe["mode"], safe["spawn"], safe["wait"]), ("inbox", False, 0))
        for args in ({"text": "task"}, {"text": "task", "to": "agent"},
                     {"text": "task", "to": "dot-one"}, {"text": "task", "to": "agent-three"},
                     {"text": "task", "to": "agent-two", "reply_to": "message-one"}):
            with self.subTest(args=args), self.assertRaises(BoundaryError):
                self.binding.authorize("everett_send", args)
        for key, value in (("spawn", True), ("spawn", False), ("dir", "/tmp"), ("harness", "codex"),
                           ("mode", "headless"), ("mode", "inbox"), ("wait", 99), ("timeout", 300), ("router", "jev")):
            with self.subTest(key=key, value=value), self.assertRaises(BoundaryError):
                self.binding.authorize("everett_send", {"text": "task", "to": "agent-two", key: value})

    def test_reply_must_be_addressed_to_binding_and_sender_allowed(self):
        own = self.inbox()
        self.assertEqual(self.binding.reply_destination("message-one"), "agent-two")
        own.unlink()
        self.inbox(agent_id="agent-two", message_id="foreign-message")
        with self.assertRaises(BoundaryError):
            self.binding.reply_destination("foreign-message")
        self.inbox(sender="agent-three")
        with self.assertRaises(BoundaryError):
            self.binding.reply_destination("message-one")
        self.inbox(addressed_to="agent-two")
        with self.assertRaises(BoundaryError):
            self.binding.reply_destination("message-one")

    def test_duplicate_and_malformed_inbox_records_fail_closed(self):
        path = self.inbox()
        path.write_text(path.read_text() * 2)
        with self.assertRaises(BoundaryError):
            self.binding.reply_destination("message-one")
        path.write_text("not json\n")
        with self.assertRaises(BoundaryError):
            self.binding.reply_destination("message-one")

    def test_binding_limits_and_exact_hosts(self):
        for fields in ({"agent_id": "../dot"}, {"harness": "grok"}, {"project": "../elsewhere"},
                       {"project": ""}, {"max_workers": 0}, {"max_workers": 9}, {"timeout": 31},
                       {"timeout": float("nan")}, {"allow_shared_core": "yes"},
                       {"allowed_hosts": ("*.example.com",)}, {"allowed_origins": ("https://example.com/path",)}):
            with self.subTest(fields=fields), self.assertRaises(BoundaryError):
                replace(self.binding, **fields)
        with self.assertRaises(BoundaryError):
            self.binding.authorize("everett_send", {"text": "x" * MAX_BODY, "to": "agent-two"})

    def test_reserved_agent_ids_are_rejected_case_insensitively(self):
        for agent_id in (HUMAN, HUMAN.upper(), "HuMaN", "live", "LIVE", "LiVe"):
            with self.subTest(agent_id=agent_id), self.assertRaises(BoundaryError):
                replace(self.binding, agent_id=agent_id)
        for agent_id in (HUMAN + "-one", "live-one"):
            self.assertEqual(replace(self.binding, agent_id=agent_id).agent_id, agent_id)

    def test_backend_environment_does_not_mutate_or_forward_credentials(self):
        with mock.patch.dict(os.environ, {"OPENAI_API_KEY": "synthetic", "EVERETT_GATEWAY_TOKEN": TOKEN}):
            before = dict(os.environ)
            env = self.binding.environment()
            self.assertEqual(dict(os.environ), before)
            self.assertNotIn("OPENAI_API_KEY", env)
            self.assertNotIn("EVERETT_GATEWAY_TOKEN", env)
            self.assertEqual(env["EVERETT_SESSION_ID"], "dot-one")

    def test_config_rejects_unknown_fields_and_requires_explicit_core_grant(self):
        config = {"agent_id": "dot-one", "harness": "openai-dot", "cwd": str(self.home), "project": "everett",
                  "destinations": ["agent-two"], "allow_shared_core": True,
                  "backend_command": [sys.executable, str(FIXTURE)]}
        path = self.home / "gateway.json"
        path.write_text(json.dumps(config))
        self.assertEqual(Binding.load(path).agent_id, "dot-one")
        path.write_text(json.dumps({**config, "extra": True}))
        with self.assertRaises(BoundaryError):
            Binding.load(path)
        del config["allow_shared_core"]
        path.write_text(json.dumps(config))
        with self.assertRaises(BoundaryError):
            Binding.load(path)


@unittest.skipUnless(SDK_AVAILABLE, "optional gateway MCP SDK not installed")
class GatewaySDK(GatewaySandbox):
    @unittest.skipUnless(NATIVE_BACKEND, "set EVERETT_GATEWAY_TEST_BINARY to a built Rust binary")
    def test_native_backend_poll_reply_card_core_and_exact_destination_guard(self):
        from everett import cards, core, external, inbox

        external.register("agent-two", "grok-bot", cwd=str(self.home))
        core.learn("Synthetic global convention for native peers", scope="global")
        core.learn("Synthetic project convention for native peers", project="everett")
        core.merge(llm="none")

        async def journey():
            for harness in ("openai-dot", "grok-bot"):
                agent_id = "ext-native-" + harness
                external.register(agent_id, harness, cwd=str(self.home))
                binding = replace(self.binding, agent_id=agent_id, harness=harness,
                                  backend_command=(NATIVE_BACKEND, "mcp"))
                gateway = Gateway(binding)
                incoming = inbox.post(agent_id, "Synthetic native handoff", sender="agent-two")
                who = await gateway.call("everett_whoami", {})
                self.assertEqual(who["session_id"], agent_id)
                self.assertEqual(who["harness"], harness)
                shared = await gateway.call("everett_core", {})
                self.assertIn("Synthetic global convention", shared["core"])
                self.assertIn("Synthetic project convention", shared["core"])
                await gateway.call("everett_card", {"what": "Synthetic work", "state": "done", "next": "Poll"})
                self.assertIn("Synthetic work", cards.card_path(agent_id).read_text())
                self.assertFalse(cards.card_path("agent-two").exists())
                peek = await gateway.call("everett_inbox", {"peek": True})
                self.assertEqual(peek["messages"][0]["id"], incoming["id"])
                consumed = await gateway.call("everett_inbox", {})
                self.assertEqual(consumed["messages"][0]["id"], incoming["id"])
                self.assertEqual((await gateway.call("everett_inbox", {}))["messages"], [])
                answer = await gateway.call("everett_send", {"reply_to": incoming["id"], "text": "Synthetic native reply"})
                self.assertEqual(answer["to"], "agent-two")
                reply = inbox.pending("agent-two")[-1]
                self.assertEqual(reply["from"], agent_id)
                self.assertEqual(reply["from_harness"], harness)
                self.assertEqual(reply["reply_to"], incoming["id"])
                queued = await gateway.call("everett_send", {"to": "agent-two", "text": "Synthetic direct task"})
                self.assertTrue(queued["queued"])
                self.assertFalse(queued["hooked"])

            external.remove("agent-two")
            external.register("agent-two-suffix", "grok-bot", title="agent-two")
            with self.assertRaises(BoundaryError):
                await gateway.call("everett_send", {"to": "agent-two", "text": "Must not route to a suffix/title"})
            self.assertEqual(inbox.pending("agent-two-suffix"), [])

        asyncio.run(journey())

    def test_separate_worker_per_binding_and_no_secret_inheritance(self):
        async def journey():
            dot = Gateway(self.binding)
            grok = Gateway(replace(self.binding, agent_id="grok-one", harness="grok-bot"))
            with mock.patch.dict(os.environ, {"EVERETT_GATEWAY_TOKEN": TOKEN, "OPENAI_API_KEY": "synthetic"}):
                return await asyncio.gather(dot.call("everett_whoami", {}), grok.call("everett_whoami", {}))

        results = asyncio.run(journey())
        self.assertEqual([r["session_id"] for r in results], ["dot-one", "grok-one"])
        records = self.records()
        self.assertEqual(len({r["pid"] for r in records}), 2)
        self.assertTrue(all(not r["has_secret"] for r in records))
        self.assertTrue(all(Path(r["cwd"]).resolve() == self.home.resolve() for r in records))

    def test_worker_resolves_symlinked_working_directory(self):
        alias = self.home / "alias"
        alias.symlink_to(self.home, target_is_directory=True)
        binding = replace(self.binding, cwd=alias)
        asyncio.run(Gateway(binding).call("everett_whoami", {}))
        self.assertEqual(len(self.records()), 1)
        self.assertEqual(Path(self.records()[0]["cwd"]).resolve(), alias.resolve())

    def test_backend_without_enforced_exact_id_capability_refuses_direct_send(self):
        async def journey():
            for flags in (("--legacy",), ("--capability", "true"), ("--capability", "{}"),
                          ("--capability", '{"enforced": false}'), ("--capability", '{"enforced": 1}')):
                with self.subTest(flags=flags), self.assertRaises(BoundaryError):
                    binding = replace(self.binding, backend_command=(*self.binding.backend_command, *flags))
                    await Gateway(binding).call("everett_send", {"to": "agent-two", "text": "task"})
        asyncio.run(journey())
        self.assertEqual(self.records(), [])

    def test_owned_reply_and_fixed_core_through_stdio_worker(self):
        self.inbox()
        async def journey():
            gateway = Gateway(self.binding)
            core = await gateway.call("everett_core", {})
            reply = await gateway.call("everett_send", {"reply_to": "message-one", "text": "Synthetic result"})
            return core, reply
        core, reply = asyncio.run(journey())
        self.assertEqual(core, {"core": "Synthetic global + project everett"})
        self.assertEqual(reply["to"], "agent-two")
        self.assertEqual(self.records()[-1]["arguments"]["mode"], "inbox")

    def test_worker_timeout_and_capacity_fail_closed(self):
        binding = replace(self.binding, timeout=1, max_workers=1,
                          backend_command=(*self.binding.backend_command, "--hang"))
        async def journey():
            gateway = Gateway(binding)
            with self.assertRaises(BoundaryError):
                await gateway.call("everett_whoami", {})
            borrower = object()
            gateway.workers.acquire_on_behalf_of_nowait(borrower)
            try:
                with self.assertRaisesRegex(BoundaryError, "busy"):
                    await gateway.call("everett_whoami", {})
            finally:
                gateway.workers.release_on_behalf_of(borrower)
            self.assertEqual(gateway.workers.borrowed_tokens, 0)
        asyncio.run(journey())

    def test_http_requires_nonempty_strong_configured_token(self):
        for token in ("", "short", "x" * 31, "x " * 32):
            with self.subTest(token=token), self.assertRaises(BoundaryError):
                build_http_app(self.binding, token)

    def test_http_sdk_roundtrip_and_boundary_rejections(self):
        import httpx
        from mcp import ClientSession
        from mcp.client.streamable_http import streamable_http_client

        async def journey():
            app = build_http_app(self.binding, TOKEN)
            async with app.router.lifespan_context(app):
                async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app),
                                            base_url="http://127.0.0.1:8788") as client:
                    self.assertEqual((await client.post("/mcp", json={})).status_code, 401)
                    self.assertEqual((await client.post("/mcp", json={}, headers={"Authorization": "Bearer wrong"})).status_code, 401)
                    client.headers["Authorization"] = "Bearer " + TOKEN
                    self.assertEqual((await client.post("/mcp", json={}, headers={"Host": "evil.example"})).status_code, 421)
                    self.assertEqual((await client.post("/mcp", json={}, headers={"Origin": "https://evil.example"})).status_code, 403)
                    duplicates = [("Authorization", "Bearer " + TOKEN), ("Authorization", "Bearer " + TOKEN)]
                    self.assertEqual((await client.post("/mcp", json={}, headers=duplicates)).status_code, 400)
                    self.assertEqual((await client.post("/mcp", content=b"x" * (MAX_BODY + 1),
                                                       headers={"Content-Type": "application/json"})).status_code, 413)
                    async def chunks():
                        yield b"x" * MAX_BODY
                        yield b"x"
                    self.assertEqual((await client.post("/mcp", content=chunks(),
                                                       headers={"Content-Type": "application/json"})).status_code, 413)
                    async with streamable_http_client("http://127.0.0.1:8788/mcp", http_client=client) as (read, write, _):
                        async with ClientSession(read, write) as session:
                            await session.initialize()
                            listed = await session.list_tools()
                            self.assertEqual({tool.name for tool in listed.tools},
                                             {"everett_core", "everett_whoami", "everett_inbox", "everett_card", "everett_send"})
                            who = await session.call_tool("everett_whoami", {})
                            assert who.structuredContent is not None
                            self.assertEqual(who.structuredContent["session_id"], "dot-one")
                            for name, args in (("everett_inbox", {"session_id": "agent-two"}),
                                               ("everett_send", {"to": "agent-two", "text": "task", "spawn": True}),
                                               ("everett_route", {"text": "task"})):
                                denied = await session.call_tool(name, args)
                                self.assertTrue(denied.isError)
                            sent = await session.call_tool("everett_send", {"to": "agent-two", "text": "Synthetic task"})
                            self.assertFalse(sent.isError)
                            assert sent.structuredContent is not None
                            self.assertEqual(sent.structuredContent["mode"], "inbox")
        asyncio.run(journey())
        self.assertEqual([r["name"] for r in self.records()], ["everett_whoami", "everett_send"])
