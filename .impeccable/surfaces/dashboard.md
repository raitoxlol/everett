# Local dashboard

## Mode

Operate.

## Job

Let a developer scan recent coding-agent activity, recognize what needs
attention, and open the exact CLI action that continues the work.

## Direction

**Everett dispatch desk.** The world borrows the type, palette, density, and
one signature move from international parcel-routing labels. It does not mimic
shipping software or replace standard web controls.

The palette is warm paper, carbon ink, muted oxide orange, and calm semantic
green/red. Orange marks current selection, live state, and primary action only.
Information is compact but never tiny. Corners are clipped just enough to feel
physical, without turning every region into a card.

## First viewport

At 1440px, a slim charcoal navigation rail anchors Everett and the primary
sections. The main field opens directly on **Recent sessions**, not a hero.
One-line totals for active, attention, and represented harnesses sit in the
header. Search and harness/state filters scope the session manifest beneath.

The manifest leads with a narrow activity route. Each stop aligns to a real
session row and carries its state through shape and text, not color alone. The
selected row expands a card detail region in place, showing its full Everett
card, project, session ID, source, and honest next action. There is no modal.

The viewport must show at least one live session, one needs-input or blocked
session, one quiet session, an agent-authored card, an automatic card, and the
all-clear empty state when filters remove every row.

## Signature interaction

Filtering or selecting a session redraws the activity route without losing row
identity. Stops travel to their new positions over 180ms while content remains
visible; reduced-motion users get an instant state change. The route is a
spatial index of the current list, never a claim that messages traveled between
the sessions.

## Structure

- Persistent navigation: Sessions, Activity, Shared core, Setup.
- Main workspace: title/totals, search and filter controls, session manifest.
- Session row: state stop, harness, project, card summary, recency, running
  state, expand control.
- Expanded detail: card body and metadata plus copyable CLI commands for
  `send`, `route`, and `events`. The first release does not execute writes.
- Mobile: navigation becomes a compact top bar, totals scroll horizontally,
  controls stack, metadata drops by priority, details stay inline.

## States

- Loading uses aligned skeleton rows.
- Empty teaches which filter to clear and offers **Reset filters**.
- Discovery failure names the local source problem and recommends
  `everett doctor`.
- Running, blocked, needs-input, done, and unknown use labels and shapes in
  addition to color.
- Keyboard focus is always visible. Selection and expansion are independent.

## Constraints

- Rust renders the HTML and serves it from the native Everett binary.
- The default listener is loopback-only. No account, remote sync, or hosted
  backend is introduced.
- Provider stores stay read-only.
- No external font, image, analytics, or JavaScript dependency.
- Desktop target: 1440px. Narrow target: 390px. Support common widths from
  1280px to 1600px without changing the type scale.
