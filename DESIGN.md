---
name: Everett dispatch desk
description: A local session manifest with restrained oxide-orange accents.
colors:
  paper: "#f3eee5"
  paper-raised: "#fffaf1"
  paper-muted: "#e9e1d5"
  ink: "#1d211f"
  ink-soft: "#5d625d"
  carbon: "#242825"
  orange: "#b64f1a"
  orange-deep: "#8e3811"
  orange-wash: "#f2d7c5"
  green: "#216b4c"
  red: "#a1382f"
  line: "#cbc1b3"
  focus: "#145e75"
typography:
  title:
    fontFamily: 'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'
    fontSize: "1.75rem"
    fontWeight: 700
    lineHeight: 1.14
    letterSpacing: "-0.035em"
  heading:
    fontFamily: 'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'
    fontSize: "1rem"
    fontWeight: 700
    lineHeight: 1.3
    letterSpacing: "-0.015em"
  body:
    fontFamily: 'ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'
    fontSize: "0.88rem"
    lineHeight: 1.55
  code:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace"
    fontSize: "0.76rem"
    lineHeight: 1.45
rounded:
  surface: "10px"
  control: "7px"
  tag: "5px"
spacing:
  tight: "8px"
  group: "14px"
  panel: "18px"
  section: "24px"
components:
  button-primary:
    backgroundColor: "{colors.orange-deep}"
    textColor: "{colors.paper-raised}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
    height: "38px"
  input-search:
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "0 12px"
    height: "38px"
  harness-tag:
    rounded: "{rounded.tag}"
    padding: "4px 7px"
---

## Overview

**Creative North Star: "Everett dispatch desk"**

The dashboard borrows the compact information structure and routing marks of
parcel labels. It remains a standard browser interface for inspecting existing
coding-agent sessions. The manifest, not decoration, owns the first viewport.

**Key Characteristics:**
- Warm paper surfaces against a charcoal navigation rail.
- Restrained orange on live activity and primary actions.
- A vertical activity route beside compact session rows.
- Native details disclosure with cards and copyable CLI commands.

## Colors

**The Signal Rule.** Orange marks activity and action. Red marks attention.
Green marks a detected harness. Status always has a text label as well as color.

Paper and ink carry ordinary content. Focus uses a contrasting teal outline on
paper and a warm light outline on charcoal. Do not add gradients or a second
brand accent. The frontmatter records the CSS custom properties in the dashboard.

## Typography

Use the platform sans for this compact operational interface. No remote fonts
are fetched. Monospace belongs only to commands, IDs, paths, and measurements.
Card bodies preserve line breaks and wrap long content. Session previews may
truncate, but expanded metadata and cards remain readable.

## Layout

Desktop uses a 220px rail and a flexible main field capped at 1460px. At 1080px
the rail becomes 150px and secondary panels reflow. At 760px it becomes a
horizontal navigation bar; filters stack, metadata reflows, and lower panels
become one column. Narrow screens keep status text visible.

**The Manifest Rule.** Filters sit directly above sessions. Details expand in
place. The dashboard never uses a modal for session inspection.

## Elevation & Depth

Thin warm borders and paper tones do most of the separation. The summary and
manifest use restrained downward soft shadows. The route's active stop has a
small orange shadow. Do not add glass panels or hard offset shadows.

## Shapes

Surfaces use 10px corners, controls use 7px, and tags use 5px. Route stops are
circles for ordinary/recent activity and rounded squares for attention. The
monogram has one tighter corner. Keep shapes subordinate to information.

## Components

- **Session manifest.** Compact summaries open native HTML details. One session
  opens at a time. Expansion shows a card, source, ID, path, and latest event.
- **Activity route.** Filtering moves already-visible stops for 180ms. Reduced
  motion removes this movement and smooth scrolling.
- **Filters.** Search matches project, card, ID, harness, and status. Harness and
  state selects compose with search. A no-match state offers reset.
- **Commands.** Buttons copy inbox-send, local-route, and events commands only.
  T3 details identify the underlying provider separately from the T3 source.
  The UI describes inbox pickup as
  dependent on hooks or polling; it never claims agent delivery or wake-up.
- **Empty state.** Explain how to create a session and diagnose discovery.

## Do's and Don'ts

- Do keep real session content primary.
- Do keep provider stores and commands read-only in the dashboard.
- Do show status text, visible focus, and reduced-motion behavior.
- Don't imply a heartbeat from Everett's recency heuristic.
- Don't imply that copying a command executes it.
- Don't add accounts, remote sync, or hosted storage to this local surface.
