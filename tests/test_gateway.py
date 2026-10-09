import sandbox  # noqa: F401  # isort: skip (must be first: isolates HOME)

import concurrent.futures
import http.client
import json
import os
import select
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

BINARY = os.environ.get("EVERETT_GATEWAY_TEST_BINARY", "")
FIXTURE = Path(__file__).parent / "fixtures" / "gateway_backend.py"
TOKEN = "synthetic-gateway-test-token-not-a-real-secret"
INITIALIZE = {"protocolVersion": "2025-06-18", "capabilities": {},
              "clientInfo": {"name": "synthetic-gateway-tests", "version": "0"}}


class Wire:
    def __init__(self, command, env):
        self.process = subprocess.Popen(command, env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        stdin, stdout, stderr = self.process.stdin, self.process.stdout, self.process.stderr
        assert stdin is not None and stdout is not None and stderr is not None
        self.stdin, self.stdout, self.stderr = stdin, stdout, stderr
        self.buffer = b""
        self.counter = 0
        self.request("initialize", INITIALIZE)
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.stdin.write(json.dumps(message).encode() + b"\n")
        self.stdin.flush()

    def receive(self, seconds=6):
        deadline = time.monotonic() + seconds
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.stdout], [], [], remaining)[0]:
                raise AssertionError("native gateway response timed out")
            chunk = os.read(self.stdout.fileno(), 65536)
            if not chunk:
                raise AssertionError("native gateway closed its response stream")
            self.buffer += chunk
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def request(self, method, params=None):
        self.counter += 1
        message = {"jsonrpc": "2.0", "id": self.counter, "method": method}
        if params is not None:
            message["params"] = params
        self.send(message)
        return self.receive()

    def call(self, name, arguments=None):
        return self.request("tools/call", {"name": name, "arguments": arguments or {}})

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=5)
        for stream in (self.stdin, self.stdout, self.stderr):
            stream.close()


@unittest.skipUnless(BINARY, "set EVERETT_GATEWAY_TEST_BINARY to the native Everett binary")
class NativeGateway(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="everett-native-gateway-")
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        self.env = dict(os.environ, HOME=str(self.home), EVERETT_HOME=str(self.home),
                        PATH="/nonexistent", EVERETT_NOTIFY="none", EVERETT_ROUTER="local",
                        EVERETT_GATEWAY_TOKEN=TOKEN, OPENAI_API_KEY="synthetic", TYPESAFE_API_KEY="synthetic")
        self.config = {"agent_id": "dot-one", "harness": "openai-dot", "cwd": str(self.home),
                       "project": "everett", "destinations": ["agent-two"], "allow_shared_core": True}
        self.number = 0

    def config_path(self, changes=None):
        self.number += 1
        path = self.home / f"binding-{self.number}.json"
        path.write_text(json.dumps(dict(self.config, **(changes or {}))))
        return path

    def cli(self, *args):
        return subprocess.run([BINARY, *args], env=self.env, capture_output=True, timeout=10, check=False)

    def peer(self, changes=None):
        wire = Wire([BINARY, "gateway", "--config", str(self.config_path(changes))], self.env)
        self.addCleanup(wire.close)
        return wire

    def fixture(self, *options):
        return {"backend_command": [sys.executable, str(FIXTURE), *options]}

    def inbox(self, sender="agent-two", to="dot-one", message_id="message-one"):
        directory = self.home / ".everett" / "inbox"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / "dot-one.jsonl"
        path.write_text(json.dumps({"id": message_id, "to": to, "from": sender,
                                    "text": "Synthetic handoff", "ts": time.time()}) + "\n")
        return path

    def records(self):
        path = self.home / "worker-calls.jsonl"
        return [json.loads(row) for row in path.read_text().splitlines()] if path.exists() else []

    def success(self, response):
        self.assertNotIn("error", response, response)
        result = response["result"]
        self.assertFalse(result.get("isError"), result)
        return result["structuredContent"]

    def denied(self, response):
        self.assertTrue("error" in response or response.get("result", {}).get("isError"), response)

    def http_server(self, changes=None):
        process = subprocess.Popen([BINARY, "gateway", "--config", str(self.config_path(changes)),
                                    "--transport", "http", "--port", "0"], env=self.env,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        stdout, stderr = process.stdout, process.stderr
        assert stdout is not None and stderr is not None
        def close():
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            stdout.close()
            stderr.close()
        self.addCleanup(close)
        self.assertTrue(select.select([stderr], [], [], 5)[0], "HTTP startup timed out")
        line = stderr.readline().decode()
        self.assertIn("listening on 127.0.0.1:", line)
        return int(line.rsplit(":", 1)[1])

    def http(self, port, message=None, headers=None, method="POST", body=None):
        defaults = {"Authorization": "Bearer " + TOKEN, "Host": "127.0.0.1:8788",
                    "Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                    "MCP-Protocol-Version": "2025-06-18"}
        for key, value in (headers or {}).items():
            if value is None:
                defaults.pop(key, None)
            else:
                defaults[key] = value
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=8)
        try:
            connection.request(method, "/mcp", body if body is not None else json.dumps(message), defaults)
            response = connection.getresponse()
            return response.status, response.read(), dict(response.getheaders())
        finally:
            connection.close()

    def test_native_stdio_journey_for_both_external_harnesses_without_python(self):
        for harness in ("openai-dot", "grok-bot"):
            with self.subTest(harness=harness):
                self.assertEqual(self.cli("external", "add", "--id", "agent-two", "--harness", harness).returncode, 0)
                self.assertEqual(self.cli("external", "add", "--id", "dot-one", "--harness", harness).returncode, 0)
                peer = self.peer({"harness": harness})
                tools = peer.request("tools/list")["result"]["tools"]
                self.assertEqual({t["name"] for t in tools}, {"everett_whoami", "everett_inbox", "everett_core", "everett_card", "everett_send"})
                identity = self.success(peer.call("everett_whoami"))
                self.assertEqual((identity["session_id"], identity["harness"]), ("dot-one", harness))
                incoming_id = "message-" + harness
                self.inbox(message_id=incoming_id)
                peek = self.success(peer.call("everett_inbox", {"peek": True}))
                self.assertEqual(peek["messages"][0]["id"], incoming_id)
                self.assertEqual(self.success(peer.call("everett_inbox"))["messages"][0]["id"], incoming_id)
                self.assertEqual(self.success(peer.call("everett_inbox"))["messages"], [])
                reply = self.success(peer.call("everett_send", {"text": "Synthetic reply", "reply_to": incoming_id}))
                self.assertEqual(reply["to"], "agent-two")
                direct = self.success(peer.call("everett_send", {"text": "Synthetic direct", "to": "agent-two"}))
                self.assertEqual((direct["session"]["id"], direct["mode"]), ("agent-two", "inbox"))
                self.assertTrue(direct["queued"])
                self.assertFalse(direct["hooked"])
                self.success(peer.call("everett_card", {"what": "Synthetic", "state": "working", "next": "reply"}))
                self.assertIn("Synthetic", (self.home / ".everett/cards/dot-one.md").read_text())
                self.assertFalse((self.home / ".everett/cards/agent-two.md").exists())
                self.assertEqual(set(self.success(peer.call("everett_core"))), {"core"})
                peer.close()
                for agent in ("agent-two", "dot-one"):
                    self.assertEqual(self.cli("external", "remove", "--id", agent).returncode, 0)

    def test_policy_rejects_foreign_identity_project_and_unsafe_arguments(self):
        peer = self.peer(self.fixture())
        cases = [(name, {}) for name in ("everett_ls", "everett_route", "everett_learn", "everett_event", "everett_subscribe")]
        cases += [(name, {"session_id": "other"}) for name in ("everett_inbox", "everett_card", "everett_send")]
        cases += [("everett_core", {"project": "elsewhere"}), ("everett_inbox", {"peek": 1}),
                  ("everett_send", {"text": "task"}), ("everett_send", {"text": "task", "to": "agent"}),
                  ("everett_send", {"text": "task", "to": "dot-one"}), ("everett_send", {"text": "task", "to": "foreign"}),
                  ("everett_send", {"text": "task", "to": "agent-two", "reply_to": "one"}),
                  ("everett_card", {"what": "x", "state": " ", "next": "x"}),
                  ("everett_send", {"text": "x" * 16001, "to": "agent-two"})]
        for key in ("spawn", "dir", "harness", "mode", "wait", "timeout", "router"):
            cases.append(("everett_send", {"text": "task", "to": "agent-two", key: False}))
        for name, arguments in cases:
            with self.subTest(name=name, arguments=arguments):
                self.denied(peer.call(name, arguments))
        self.assertEqual(self.records(), [])

    def test_core_is_an_explicit_grant_and_output_is_redacted(self):
        peer = self.peer(dict(self.fixture(), allow_shared_core=False))
        self.assertNotIn("everett_core", {t["name"] for t in peer.request("tools/list")["result"]["tools"]})
        self.denied(peer.call("everett_core"))
        peer = self.peer(self.fixture())
        self.assertEqual(self.success(peer.call("everett_core")), {"core": "Synthetic global + project everett"})

    def test_reply_authorization_fails_closed(self):
        peer = self.peer(self.fixture())
        for sender, to in (("foreign", "dot-one"), ("agent-two", "other")):
            self.inbox(sender=sender, to=to)
            self.denied(peer.call("everett_send", {"text": "reply", "reply_to": "message-one"}))
        path = self.inbox()
        for raw in (path.read_text() * 2, "not json\n", " " * (8 * 1024 * 1024 + 1)):
            path.write_text(raw)
            self.denied(peer.call("everett_send", {"text": "reply", "reply_to": "message-one"}))
        self.assertEqual(self.records(), [])

    def test_legacy_or_malformed_exact_id_capability_blocks_direct_send_only(self):
        for options in (("--legacy",), ("--capability", "true"), ("--capability", '{"enforced":"true"}'), ("--capability", '{"enforced":false}')):
            with self.subTest(options=options):
                peer = self.peer(self.fixture(*options))
                self.denied(peer.call("everett_send", {"text": "task", "to": "agent-two"}))
                self.inbox()
                if options == ("--capability", "true"):
                    # The SDK rejects non-object experimental capabilities at initialize.
                    self.denied(peer.call("everett_send", {"text": "reply", "reply_to": "message-one"}))
                    continue
                self.assertEqual(self.success(peer.call("everett_send", {"text": "reply", "reply_to": "message-one"}))["to"], "agent-two")
                self.assertIn("core", self.success(peer.call("everett_core")))

    def test_worker_identity_cwd_credentials_and_lifecycle(self):
        peer = self.peer(self.fixture())
        self.success(peer.call("everett_whoami"))
        self.success(peer.call("everett_whoami"))
        other = self.peer(dict(self.fixture(), agent_id="grok-one", harness="grok-bot"))
        self.assertEqual(self.success(other.call("everett_whoami"))["session_id"], "grok-one")
        records = self.records()
        self.assertEqual(len({r["pid"] for r in records}), 3)
        for record in records:
            self.assertFalse(record["has_secret"])
            self.assertEqual(Path(record["cwd"]).resolve(), self.home.resolve())
            with self.assertRaises(ProcessLookupError):
                os.kill(record["pid"], 0)

    def test_worker_timeout_and_concurrency_are_bounded_and_reap_children(self):
        port = self.http_server(dict(self.fixture("--hang"), max_workers=1, timeout=1))
        message = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "everett_whoami", "arguments": {}}}
        with concurrent.futures.ThreadPoolExecutor() as executor:
            first = executor.submit(self.http, port, message)
            deadline = time.monotonic() + 3
            while not self.records() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(self.records())
            start = time.monotonic()
            status, raw, _ = self.http(port, message)
            self.assertEqual(status, 200)
            self.denied(json.loads(raw))
            self.assertIn(b"busy", raw)
            self.assertLess(time.monotonic() - start, 0.8)
            status, raw, _ = first.result(timeout=5)
            self.assertEqual(status, 200)
            self.denied(json.loads(raw))
        for record in self.records():
            with self.assertRaises(ProcessLookupError):
                os.kill(record["pid"], 0)

    def test_configuration_rejects_bad_types_reserved_ids_paths_and_unknown_fields(self):
        for fields in ({"agent_id": "../dot"}, {"agent_id": "HUMAN"}, {"agent_id": "LIVE"},
                       {"harness": "grok"}, {"project": "../other"}, {"project": ""},
                       {"cwd": "relative"}, {"max_workers": 0}, {"max_workers": 9},
                       {"max_workers": True}, {"timeout": 31}, {"timeout": True},
                       {"allow_shared_core": "yes"}, {"extra": True}, {"backend_command": []},
                       {"backend_command": ["everett", "mcp"]}, {"allowed_hosts": []},
                       {"allowed_hosts": ["*.example.com"]}, {"allowed_hosts": ["example.com:99999"]},
                       {"allowed_origins": ["https://example.com/path"]}):
            with self.subTest(fields=fields):
                result = self.cli("gateway", "--config", str(self.config_path(fields)))
                self.assertEqual(result.returncode, 2, result.stderr)
        for key in ("allow_shared_core", "agent_id", "harness", "cwd", "project", "destinations"):
            path = self.config_path()
            config = dict(self.config)
            del config[key]
            path.write_text(json.dumps(config))
            self.assertEqual(self.cli("gateway", "--config", str(path)).returncode, 2)
        path.write_text("x" * 16385)
        self.assertEqual(self.cli("gateway", "--config", str(path)).returncode, 2)

    def test_http_requires_a_strong_token_without_logging_it(self):
        for token in (None, "", "short", "x" * 513, "x" * 32 + " ", "é" * 32):
            with self.subTest(token=token):
                env = dict(self.env)
                if token is None:
                    env.pop("EVERETT_GATEWAY_TOKEN")
                else:
                    env["EVERETT_GATEWAY_TOKEN"] = token
                result = subprocess.run([BINARY, "gateway", "--config", str(self.config_path()), "--transport", "http"],
                                        env=env, capture_output=True, timeout=5, check=False)
                self.assertEqual(result.returncode, 2)
                if token and len(token) >= 32:
                    self.assertNotIn(token.encode(), result.stderr)

    def test_http_sdk_roundtrip_is_stateless_and_native(self):
        port = self.http_server()
        for message in ({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": INITIALIZE},
                        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                        {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "everett_whoami", "arguments": {}}}):
            status, raw, headers = self.http(port, message)
            self.assertEqual(status, 200, raw)
            self.assertNotIn("mcp-session-id", {key.lower() for key in headers})
            response = json.loads(raw)
            self.assertNotIn("error", response)
            if message["method"] == "tools/call":
                self.assertEqual(self.success(response)["session_id"], "dot-one")
        status, _, _ = self.http(port, method="GET")
        self.assertIn(status, (405, 400))

    def test_http_rejects_auth_host_origin_oversized_and_non_json_requests(self):
        port = self.http_server()
        message = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": INITIALIZE}
        for headers, expected in (({"Authorization": None}, 401), ({"Authorization": "Bearer wrong"}, 401),
                                  ({"Host": "evil.example"}, 421), ({"Origin": "https://evil.example"}, 403),
                                  ({"Content-Type": "text/plain"}, 415)):
            with self.subTest(headers=headers):
                self.assertEqual(self.http(port, message, headers)[0], expected)
        self.assertEqual(self.http(port, body="x" * 65537)[0], 413)
        status, _, _ = self.http(port, headers={"Content-Type": "application/json; charset=utf-8"}, message=message)
        self.assertEqual(status, 200)

    def test_exact_public_host_and_allowed_origin_work(self):
        port = self.http_server({"allowed_hosts": ["gateway.example:8443"], "allowed_origins": ["https://client.example"]})
        message = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": INITIALIZE}
        self.assertEqual(self.http(port, message, {"Host": "gateway.example:8443", "Origin": "https://client.example"})[0], 200)
        self.assertEqual(self.http(port, message, {"Host": "gateway.example:8443", "Origin": "https://client.example/"})[0], 403)

    def test_duplicate_security_headers_fail_closed(self):
        port = self.http_server()
        for header, value in (("Host", "127.0.0.1:8788"), ("Origin", "https://evil.example"),
                              ("Authorization", "Bearer " + TOKEN), ("Content-Length", "2"),
                              ("Transfer-Encoding", "chunked")):
            with self.subTest(header=header):
                raw = f"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n{header}: {value}\r\nConnection: close\r\n\r\n{{}}".encode()
                if header == "Origin":
                    raw = raw.replace(b"Connection: close", b"Origin: https://evil.example\r\nConnection: close")
                if header == "Transfer-Encoding":
                    raw = raw.replace(b"Content-Length: 2", b"Transfer-Encoding: chunked")
                with socket.create_connection(("127.0.0.1", port), timeout=5) as client:
                    client.sendall(raw)
                    reply = client.recv(8192)
                self.assertIn(b" 400 ", reply, reply)

    def test_http_body_read_timeout_and_request_capacity(self):
        port = self.http_server({"max_workers": 1})
        clients = []
        try:
            for _ in range(2):
                client = socket.create_connection(("127.0.0.1", port), timeout=8)
                clients.append(client)
                client.sendall(f"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:8788\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx".encode())
            deadline = time.monotonic() + 2
            status = 200
            while status != 503 and time.monotonic() < deadline:
                status, _, _ = self.http(port, {"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
            self.assertEqual(status, 503)
            for client in clients:
                self.assertIn(b" 504 ", client.recv(8192))
        finally:
            for client in clients:
                client.close()

    def test_oversized_stdio_frame_closes_without_dispatching(self):
        peer = self.peer(self.fixture())
        peer.stdin.write(b"x" * 65537 + b"\n")
        peer.stdin.flush()
        peer.process.wait(timeout=5)
        self.assertEqual(self.records(), [])

    def test_backend_results_are_structured_bounded_and_identity_checked(self):
        for mode in ("text-only", "oversized", "amplified", "non-object", "wrong-identity", "wrong-harness"):
            with self.subTest(mode=mode):
                peer = self.peer(dict(self.fixture("--result-mode", mode), timeout=1))
                tool = "everett_whoami" if mode.startswith("wrong-") else "everett_core"
                response = peer.call(tool)
                self.denied(response)
                self.assertLess(len(json.dumps(response)), 1024)
                peer.close()

    def test_native_backend_never_resolves_destination_prefix_or_title(self):
        self.assertEqual(self.cli("external", "add", "--id", "agent-two-suffix", "--harness", "grok-bot", "--title", "agent-two").returncode, 0)
        peer = self.peer()
        self.denied(peer.call("everett_send", {"text": "Must not route", "to": "agent-two"}))
        self.assertFalse((self.home / ".everett/inbox/agent-two-suffix.jsonl").exists())

    def test_symlinked_working_directory_is_canonicalized(self):
        link = self.home / "linked-project"
        link.symlink_to(self.home, target_is_directory=True)
        peer = self.peer(dict(self.fixture(), cwd=str(link)))
        self.success(peer.call("everett_whoami"))
        self.assertEqual(self.records()[0]["cwd"], str(self.home.resolve()))
