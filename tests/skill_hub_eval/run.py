#!/usr/bin/env python3
"""Score the skill hub: how many cases deliver the wrong skill or too many bytes.

Usage: run.py CASES.json   (prints one line per failure, then `failed=N`)
Each case runs the built everett binary in a fresh fixture home.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
BIN = os.environ.get("EVERETT_EVAL_BINARY") or str(HERE.parent.parent / "everett-rs/target/debug/everett")
SKILLS = json.loads((HERE / "skills.json").read_text())
HINT_BYTES = 400       # a suggestion is one line
SHORTLIST_BYTES = 900  # the always-present part
SHORTLIST_MAX = 8
OVERHEAD_BYTES = 700   # match/hook payload beyond the skill body


def body_of(skill):
    return skill.get("body") or f"BODY {skill['name']} {skill['root']}"


def make_home():
    home = Path(tempfile.mkdtemp(prefix="skill-eval-"))
    for skill in SKILLS:
        folder = home / skill["root"] / skill["name"]
        folder.mkdir(parents=True)
        (folder / "SKILL.md").write_text(
            f"---\nname: {skill['name']}\ndescription: {skill['description']}\n---\n\n{body_of(skill)}\n"
        )
    (home / ".everett").mkdir()
    (home / ".everett/config.toml").write_text('[skills]\ndirs = "~/stack/skills"\n')
    for project in ("proj", "other"):
        (home / project / ".git").mkdir(parents=True, exist_ok=True)
    return home


def run(home, args, stdin="", cwd="proj"):
    env = {
        "HOME": str(home), "EVERETT_HOME": str(home), "PATH": "/usr/bin:/bin",
        "EVERETT_NOTIFY": "none", "EVERETT_HOP_LIMIT": "on",
    }
    return subprocess.run([BIN, *args], input=stdin, capture_output=True, text=True,
                          env=env, cwd=home / cwd, timeout=30)


def bodies(text):
    """Names of fixture skills whose body text appears in `text`."""
    return {s["name"] for s in SKILLS if body_of(s) in text}


def skill_body(name):
    return max((body_of(s) for s in SKILLS if s["name"] == name), key=len)


def hook(home, prompt, event="UserPromptSubmit", harness="claude", session="eval-session"):
    payload = {"session_id": session, "hook_event_name": event, "prompt": prompt, "cwd": str(home / "proj")}
    out = run(home, ["hook", f"{harness}_inbox"], json.dumps(payload)).stdout.strip()
    if not out:
        return ""
    return json.loads(out)["hookSpecificOutput"]["additionalContext"]


def account(home):
    """Local MCP tool calls need a signed-in profile; stand in for the issuer."""
    import http.server
    import threading

    class Whoami(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = b'{"result":{"owner":"fixture-owner","deviceId":"fixture-agent","role":"member"}}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        do_POST = do_GET

        def log_message(self, *args):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Whoami)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    path = home / ".everett/cloudflare-auth.json"
    path.write_text(json.dumps({
        "issuer": f"http://127.0.0.1:{server.server_port}", "owner": "fixture-owner", "agent_id": "fixture-agent",
        "device_name": "Fixture", "token": "evt_" + "a" * 64, "expires_at": 2**64 - 1}))
    path.chmod(0o600)


def mcp(home, requests, cwd="proj"):
    if requests:
        account(home)
    lines = [{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "eval", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"}]
    lines += [{"jsonrpc": "2.0", "id": i + 2, **r} for i, r in enumerate(requests)]
    out = run(home, ["mcp"], "".join(json.dumps(line) + "\n" for line in lines), cwd).stdout
    return {m["id"]: m for m in map(json.loads, filter(None, out.splitlines())) if "id" in m}


def shortlist(home, cwd):
    out = run(home, ["skills", "--shortlist", "--json"], cwd=cwd)
    return out.stdout, [row["name"] for row in json.loads(out.stdout)]


def check_match(case, payload, raw):
    skill = payload.get("skill")
    got = skill["name"] if skill else None
    if got != case["expect"]:
        return f"got {got}"
    if skill:
        if skill_body(got) not in skill.get("content", ""):
            return "no body"
        if len(raw) > len(skill["content"]) + OVERHEAD_BYTES:
            return f"{len(raw)} bytes"
        if "skills" in payload:
            return "returned a list"
    return None


def check(case, home):
    kind = case["kind"]
    if kind == "match":
        out = run(home, ["skills", "--match", case["task"], "--json"])
        return check_match(case, json.loads(out.stdout), out.stdout)
    if kind == "mcp_match":
        reply = mcp(home, [{"method": "tools/call", "params": {
            "name": "everett_skill", "arguments": {"action": "match", "task": case["task"]}}}])[2]
        result = reply["result"]
        return check_match(case, result["structuredContent"], json.dumps(result["structuredContent"]))
    if kind == "hook":
        for earlier in case.get("after", []):
            hook(home, earlier, harness=case.get("harness", "claude"))
        text = hook(home, case["prompt"], case.get("event", "UserPromptSubmit"), case.get("harness", "claude"))
        mode, want = case["mode"], case["expect"]
        if mode == "none":
            return None if not text else f"expected silence, got {text[:80]!r}"
        if mode == "full":
            if bodies(text) != {want}:
                return f"bodies {sorted(bodies(text))}"
            return None if len(text) <= len(skill_body(want)) + OVERHEAD_BYTES else f"{len(text)} bytes"
        if bodies(text):
            return "hint carried a body"
        if f"`{want}`" not in text:
            return f"hint {text[:80]!r}"
        others = [s["name"] for s in SKILLS if s["name"] != want and f"`{s['name']}`" in text]
        if others:
            return f"hint also named {others}"
        return None if len(text) <= HINT_BYTES else f"{len(text)} bytes"
    if kind == "get":
        args = ["skills", "--get", case["name"]]
        if case.get("source"):
            args += ["--source", case["source"]]
        out = run(home, args)
        if out.returncode != 0:
            return f"exit {out.returncode}"
        return None if case["expect_body"] in out.stdout else f"body {out.stdout[:60]!r}"
    if kind == "list_once":
        rows = json.loads(run(home, ["skills", "--json"]).stdout)
        count = sum(row["name"] == case["name"] for row in rows)
        return None if count == 1 else f"listed {count} times"
    if kind == "shortlist":
        for name in case.get("used", []):
            run(home, ["skills", "--get", name])
        for task in case.get("matched", []):
            run(home, ["skills", "--match", task, "--json"])
        raw, names = shortlist(home, case.get("cwd", "proj"))
        missing = [n for n in case["include"] if n not in names]
        banned = [n for n in case["exclude"] if n in names]
        if missing or banned:
            return f"missing {missing} banned {banned} in {names}"
        if len(names) > SHORTLIST_MAX:
            return f"{len(names)} entries"
        return None
    if kind == "instructions":
        text = mcp(home, [])[1]["result"]["instructions"]
        start = text.find("Skills for this project")
        if start < 0:
            return "no shortlist in instructions"
        part = text[start:]
        missing = [n for n in case["include"] if f"`{n}`" not in part]
        banned = [n for n in case["exclude"] if f"`{n}`" in part]
        if missing or banned:
            return f"missing {missing} banned {banned}"
        return None if len(part) <= SHORTLIST_BYTES else f"{len(part)} bytes"
    return f"unknown kind {kind}"


def main():
    cases = json.loads(Path(sys.argv[1]).read_text())
    failed = 0
    for index, case in enumerate(cases):
        home = make_home()
        try:
            problem = check(case, home)
        except Exception as error:  # a crash or malformed output is a failed case
            problem = f"{type(error).__name__}: {str(error)[:120]}"
        finally:
            shutil.rmtree(home, ignore_errors=True)
        if problem:
            failed += 1
            label = case.get("task") or case.get("prompt") or case.get("name") or ""
            print(f"FAIL {index} {kind_of(case)} {label!r}: {problem}")
    print(f"cases={len(cases)}")
    print(f"failed={failed}")


def kind_of(case):
    return case["kind"] + ("/" + case["mode"] if "mode" in case else "")


if __name__ == "__main__":
    main()
