# Messages and status

## Sub-features

Inbox delivery, replies, consumption, subscriptions, status events.

## How to get to it (user POV)

`everett send "request" --to SESSION --mode inbox`, `everett reply MESSAGE_ID "answer"`, `everett inbox --session SESSION --json`, and MCP send/inbox/subscribe/event tools.

## Driving it with the release helper

An MCP request receives a CLI reply; a CLI request receives an MCP reply. Verify message ids, reply_to, text and destination. A second inbox read is empty after consumption. A done event must arrive in the subscriber's inbox and appear in session listing and CLI events.

## Gotchas

This proves file delivery; a live harness needs installed hooks to inject the inbox. Headless resumes are outside this no-model journey. Hop and self-send guards are tested by the unit suite.
