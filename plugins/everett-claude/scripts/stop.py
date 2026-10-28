import json
import signal
import sys

from wake import nudge, peek_ids, valid_session


def expired(_signal, _frame):
    raise TimeoutError()


def main():
    signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, 0.85)
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536:
            return
        event = json.loads(raw)
        if not isinstance(event, dict) or event.get("stop_hook_active") is True:
            return
        session = event.get("session_id")
        if not valid_session(session):
            return
        ids = peek_ids(session)[:5]
        if ids:
            response = {"decision": "block", "reason": nudge(session, ids)}
            encoded = json.dumps(response)
            signal.setitimer(signal.ITIMER_REAL, 0)
            print(encoded)
    except Exception:
        pass
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)


if __name__ == "__main__":
    main()
