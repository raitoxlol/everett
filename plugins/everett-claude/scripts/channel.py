import json
import os
import selectors
import sys
import time

from wake import INSTRUCTIONS, nudge, peek_ids, valid_session


def main():
    session = os.environ.get("EVERETT_SESSION_ID")
    enabled = os.environ.get("EVERETT_CLAUDE_CHANNEL") == "1" and valid_session(session)
    phase = "initialize"
    seen = set()
    buffer = bytearray()
    outgoing = bytearray()
    output_deadline = None
    next_poll = time.monotonic() + 0.5
    os.set_blocking(sys.stdout.fileno(), False)
    with selectors.DefaultSelector() as selector:
        def send(message):
            nonlocal output_deadline
            encoded = (json.dumps({"jsonrpc": "2.0", **message}) + "\n").encode()
            if len(outgoing) + len(encoded) > 1024 * 1024:
                raise BufferError("outgoing queue full")
            if not outgoing:
                output_deadline = time.monotonic() + 1
                selector.register(sys.stdout, selectors.EVENT_WRITE)
            outgoing.extend(encoded)

        selector.register(sys.stdin, selectors.EVENT_READ)
        while True:
            now = time.monotonic()
            if output_deadline is not None and now >= output_deadline:
                return
            deadline = min(next_poll, output_deadline) if output_deadline is not None else next_poll
            events = selector.select(max(0, deadline - now))
            # Observe stdin EOF before attempting any ready output write.
            for key, _events in sorted(events, key=lambda event: event[0].fileobj is sys.stdout):
                if key.fileobj is sys.stdout:
                    try:
                        written = os.write(sys.stdout.fileno(), outgoing[:65536])
                    except BlockingIOError:
                        continue
                    del outgoing[:written]
                    if not outgoing:
                        selector.unregister(sys.stdout)
                        output_deadline = None
                    continue
                chunk = os.read(sys.stdin.fileno(), 65536)
                if not chunk:
                    return
                buffer.extend(chunk)
                while b"\n" in buffer:
                    line, _, buffer = buffer.partition(b"\n")
                    if len(line) > 65536:
                        return
                    try:
                        message = json.loads(line)
                    except (ValueError, UnicodeError):
                        send({"id": None, "error": {"code": -32700, "message": "Parse error"}})
                        continue
                    if not isinstance(message, dict):
                        continue
                    method = message.get("method")
                    if method == "initialize" and phase == "initialize" and "id" in message:
                        requested = message.get("params", {}).get("protocolVersion")
                        protocol = requested if requested in ("2024-11-05", "2025-03-26", "2025-06-18") else "2025-03-26"
                        send({"id": message["id"], "result": {
                            "protocolVersion": protocol,
                            "capabilities": {"experimental": {"claude/channel": {}}} if enabled else {},
                            "serverInfo": {"name": "everett-wake", "version": "0.1.0"},
                            "instructions": INSTRUCTIONS,
                        }})
                        phase = "initialized"
                    elif method == "notifications/initialized" and phase == "initialized":
                        phase = "ready"
                        next_poll = time.monotonic() + 0.5
                    elif "id" in message:
                        if method == "ping":
                            send({"id": message["id"], "result": {}})
                        elif method == "tools/list":
                            send({"id": message["id"], "result": {"tools": []}})
                        else:
                            send({"id": message["id"], "error": {"code": -32601, "message": "Method not found"}})
                if len(buffer) > 65536:
                    return
            if time.monotonic() >= next_poll:
                if phase == "ready" and enabled and len(seen) < 10000:
                    try:
                        ids = [mid for mid in peek_ids(session) if mid not in seen][:min(5, 10000 - len(seen))]
                    except Exception:
                        ids = []
                    if ids:
                        seen.update(ids)
                        send({"method": "notifications/claude/channel", "params": {
                            "content": nudge(session, ids), "meta": {},
                        }})
                next_poll = time.monotonic() + 0.5


if __name__ == "__main__":
    try:
        main()
    except (BrokenPipeError, OSError, ValueError, TypeError, AttributeError, BufferError):
        pass
