#!/usr/bin/env python3
"""Local fixture acceptance. No real provider or public-network authentication."""
from http.client import HTTPConnection
import json
import os
from pathlib import Path
import select
import stat
import subprocess
import sys
import tempfile

fixture = Path(sys.argv[1]).resolve()
helper = Path(sys.argv[2]).resolve()
node = Path(sys.argv[3]).resolve()
binary = Path(sys.argv[4]).resolve()

def check(value, stage):
    if not value:
        raise RuntimeError(stage)

def private(path):
    info = path.lstat()
    check(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o600, "private_file")
    return path.read_text()

def helper_run(mode, home):
    result = subprocess.run([str(node), str(helper), mode, str(home)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
    check(result.returncode == 0, "helper_" + mode)
    return json.loads(result.stdout)

def http(method, path, token=None, body=None):
    connection = HTTPConnection("127.0.0.1", 18840, timeout=10)
    headers = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json", "MCP-Protocol-Version": "2025-06-18"}
    if token is not None:
        headers["Authorization"] = "Bearer " + token
    try:
        connection.request(method, path, body, headers)
        response = connection.getresponse()
        raw = response.read(262144)
        return response.status, dict(response.getheaders()), raw
    finally:
        connection.close()

def whoami(token, expected=None):
    request = json.dumps({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "everett_whoami", "arguments": {}}})
    status, headers, raw = http("POST", "/mcp", token, request)
    if expected:
        check(status == 200, "gateway_http")
        content = raw.decode()
        if content.startswith("event:") or content.startswith("data:"):
            content = next(line[5:].strip() for line in content.splitlines() if line.startswith("data:"))
        result = json.loads(content)["result"]
        check(not result.get("isError", False), "gateway_tool")
        identity = result["structuredContent"]
        check(identity["session_id"] == expected and identity["harness"] == "openai-dot", "gateway_identity")
        check(identity["gateway_auth"]["mode"] == "oauth", "gateway_oauth")
    return status, headers

process = None
stage = "setup"
try:
    info = fixture.lstat()
    check(stat.S_ISDIR(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o700 and info.st_uid == os.getuid(), "private_home")
    config = json.loads(private(fixture / "fixture.json"))
    check(config["issuer"] == "http://127.0.0.1:18839" and config["resource"] == "http://127.0.0.1:18840/mcp", "fixture_target")
    first = json.loads(private(fixture / "tokens.json"))
    old_access = private(fixture / "access.token")
    secret = private(fixture / "introspection.secret")
    second = Path(sys.argv[5]).resolve()
    second_info = second.lstat()
    check(stat.S_ISDIR(second_info.st_mode) and stat.S_IMODE(second_info.st_mode) == 0o700 and second_info.st_uid == os.getuid(), "second_private_home")
    stage = "independent_check"
    independent = helper_run("check", second)
    check(independent["active"] and independent["agent_id"] != first["agent_id"], stage)
    independent_access = private(second / "access.token")
    gateway_home = Path(tempfile.mkdtemp(prefix="gateway-home-", dir=fixture))
    binding = {"agent_id": "gateway-fixture", "harness": "openai-dot", "cwd": str(gateway_home), "project": "everett", "destinations": [], "allow_shared_core": False, "allowed_hosts": ["127.0.0.1:18840"], "oauth": {"issuer": config["issuer"], "resource": config["resource"]}}
    config_path = gateway_home / "gateway.json"
    config_path.write_text(json.dumps(binding))
    os.chmod(config_path, 0o600)
    environment = {"PATH": "/nonexistent", "EVERETT_HOME": str(gateway_home), "EVERETT_OAUTH_INTROSPECTION_SECRET": secret, "EVERETT_NOTIFY": "none"}
    stage = "gateway_start"
    process = subprocess.Popen([str(binary), "gateway", "--config", str(config_path), "--transport", "http", "--port", "18840"], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    ready = select.select([process.stderr], [], [], 5)[0]
    check(ready and process.stderr.readline().startswith(b"everett.gateway: listening on 127.0.0.1:18840"), stage)
    check(process.poll() is None, stage)
    stage = "discovery"
    status, _, raw = http("GET", "/.well-known/oauth-protected-resource/mcp")
    metadata = json.loads(raw)
    check(status == 200 and metadata["resource"] == config["resource"] and metadata["authorization_servers"] == [config["issuer"]], stage)
    status, headers = whoami(None)
    check(status == 401 and "resource_metadata" in headers.get("www-authenticate", ""), "challenge")
    stage = "issued_identity"
    check(whoami(old_access, first["agent_id"])[0] == 200, stage)
    check(whoami(independent_access, independent["agent_id"])[0] == 200, "independent_identity")
    stage = "refresh"
    refreshed = helper_run("refresh", fixture)
    check(refreshed["active"] and refreshed["agent_id"] == first["agent_id"], stage)
    new_access = private(fixture / "access.token")
    check(new_access != old_access, "access_rotated")
    old_status = whoami(old_access, first["agent_id"])[0]
    check(whoami(new_access, first["agent_id"])[0] == 200, "refreshed_identity")
    stage = "owner_revoke"
    revoked = helper_run("revoke-agent", fixture)
    check(revoked["active"] is False and revoked["refresh_http"] == 400, stage)
    check(whoami(old_access)[0] == 401 and whoami(new_access)[0] == 401, "revoked_gateway")
    check(whoami(independent_access, independent["agent_id"])[0] == 200, "independent_survives")
    print(json.dumps({"status": "passed", "metadata_http": 200, "missing_http": 401, "issued_whoami_http": 200, "refresh_whoami_http": 200, "old_access_after_refresh_http": old_status, "old_access_after_owner_revoke_http": 401, "new_access_after_owner_revoke_http": 401, "revoked_refresh_http": 400, "independent_whoami_after_revoke_http": 200, "backend": "actual native Everett mcp", "provider": "synthetic outbound GitHub and CIMD", "issuer": config["issuer"], "resource": config["resource"]}))
except Exception as error:
    detail = str(error) if type(error) is RuntimeError and str(error).replace("_", "").isalnum() else type(error).__name__
    print(json.dumps({"status": "failed", "stage": stage, "check": detail, "detail": "response and credential values suppressed"}))
    sys.exit(1)
finally:
    if process is not None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
