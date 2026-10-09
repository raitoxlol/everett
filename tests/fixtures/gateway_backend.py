"""Synthetic stdio backend for gateway tests; never invokes a model or harness."""
import json
import os
import sys
import time
from pathlib import Path


def main():
    home = Path(os.environ["EVERETT_HOME"])
    own = os.environ["EVERETT_SESSION_ID"]
    for line in sys.stdin:
        message = json.loads(line)
        if "id" not in message:
            continue
        method = message["method"]
        if method == "initialize":
            capabilities = {"tools": {}}
            if "--legacy" not in sys.argv:
                capabilities["experimental"] = {"everettExactDestinationIds": True}
            result = {"protocolVersion": "2025-06-18", "capabilities": capabilities,
                      "serverInfo": {"name": "synthetic-everett", "version": "0"}}
        elif method == "tools/list":
            result = {"tools": [{"name": name, "inputSchema": {"type": "object"}}
                                for name in ("everett_core", "everett_whoami", "everett_inbox", "everett_card", "everett_send")]}
        elif method == "tools/call":
            name = message["params"]["name"]
            args = message["params"]["arguments"]
            record = {"name": name, "arguments": args, "pid": os.getpid(), "cwd": os.getcwd(),
                      "agent_id": own, "harness": os.environ["EVERETT_HARNESS_NAME"],
                      "has_secret": any(key in os.environ for key in ("EVERETT_GATEWAY_TOKEN", "OPENAI_API_KEY", "TYPESAFE_API_KEY"))}
            with (home / "worker-calls.jsonl").open("a") as log:
                log.write(json.dumps(record) + "\n")
            if "--hang" in sys.argv:
                time.sleep(60)
            if name == "everett_whoami":
                data = {"session_id": own, "harness": os.environ["EVERETT_HARNESS_NAME"]}
            elif name == "everett_core":
                data = {"core": "Synthetic global + project " + args["project"], "pending_learnings": 99}
            elif name == "everett_inbox":
                path = home / ".everett" / "inbox" / (own + ".jsonl")
                data = {"messages": [json.loads(row) for row in path.read_text().splitlines()] if path.exists() else []}
            elif name == "everett_send":
                if args["mode"] != "inbox" or args["wait"] != 0 or args["spawn"] is not False:
                    raise AssertionError("unsafe send arguments")
                to = args.get("to")
                if "reply_to" in args:
                    rows = (home / ".everett" / "inbox" / (own + ".jsonl")).read_text().splitlines()
                    to = next(json.loads(row)["from"] for row in rows if json.loads(row)["id"] == args["reply_to"])
                data = {"delivered": True, "mode": "inbox", "message_id": "synthetic-reply", "to": to}
            else:
                data = {"session_id": own, "written": True}
            result = {"content": [{"type": "text", "text": json.dumps(data)}],
                      "structuredContent": data, "isError": False}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)


if __name__ == "__main__":
    main()
