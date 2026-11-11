# Hop limit

Everett counts hops: how many times work has been passed from one agent session to another.
The count is always tracked and shown on every delivered message (`hop 4`).

**The limit is off by default.** Agents can forward and reply as many times as they need.

## Turn it on

The limit stops two agents from looping on each other forever. To turn it on:

Add this line to `~/.everett/config.toml`:

```toml
hop_limit = "on"   # cap at 3 hops; or a number, e.g. hop_limit = "5"
```

Or set it for one shell: `export EVERETT_HOP_LIMIT=on`.

When it is on, a send past the cap exits with code 7, and a reply thread stops after
4 back-and-forths.

## Turn it off again

Set `hop_limit = "off"` in `~/.everett/config.toml`, or delete the line.

Or `export EVERETT_HOP_LIMIT=off`.

## Cross-device messages keep a cap of 3

Messages that go through the hosted relay or an SSH peer still stop at 3 hops, whatever the
setting. The hosted relay and older Everett versions reject envelopes past hop 3, so raising
this needs a relay deploy and a protocol version bump first.

## Context on every message

Each batch Everett delivers starts with a short context note: the messages come from other
agent sessions rather than the user; treat them as requests, default to read-only work
unless the user asked otherwise, and answer with `everett_send(reply_to=...)`.
