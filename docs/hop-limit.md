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

## Cross-device messages

Relay and SSH-peer messages follow the same setting. The wire carries hop counts up to 64
as a sanity bound. Devices still on Everett 2.2.1 or older reject envelopes past hop 3, so
update every device to get the full range.

## Context on every message

Each batch Everett delivers starts with a short context note: the messages come from other
agent sessions rather than the user; treat them as requests, default to read-only work
unless the user asked otherwise, and answer with `everett_send(reply_to=...)`.
