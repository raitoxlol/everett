# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Rust server with HTML rendered for the browser. The first build is local and
Mac-first. It must not require a JavaScript application runtime.

## Users

Developers who work across several AI coding-agent harnesses and need to see
what their existing sessions are doing without opening every terminal.

## Product Purpose

Everett discovers sessions that supported harnesses already created, summarizes
them with cards and events, routes work between them, and gives them shared
memory. The dashboard makes that local activity visible and scannable in one
place.

## Positioning

Everett is a neutral layer over existing coding-agent sessions. It does not
require every session to be launched by a new manager before it can discover,
summarize, route to, or message that session.

## Operating Context

The first dashboard runs from the Everett CLI on a Mac and opens in a browser.
It reads the same local provider stores, cards, events, and shared core as the
CLI. Recent sessions across supported harnesses are the primary view.

## Capabilities and Constraints

- Show recent sessions across Claude Code, Codex, OMP, Pi, Hermes, Grok CLI,
  and Devin CLI. T3 Code appears as an annotation over the provider session it
  drives.
- Show Everett cards, current state, recency, project, harness, and whether a
  session appears active.
- Keep the first version local and read-only. It must not imply that viewing a
  session delivered a message or woke an agent.
- Account-backed, hosted access is a future direction: one Everett account may
  eventually connect CLI installations across devices and expose their
  activity through a web dashboard. Authentication, synchronization, storage,
  hosting, and Vercel deployment are deliberately undecided and out of this
  build.

## Brand Commitments

The product name is Everett. This dashboard uses orange as a restrained accent,
not a vivid full-surface treatment. It should feel considered rather than
sloppy or overstimulating.

## Evidence on Hand

The repository contains real adapters, session records, cards, event states,
routing, inboxes, and shared-core behavior. It contains no approved logo,
customer claims, usage metrics, or product photography; the dashboard must not
fabricate them.

## Product Principles

- Existing sessions first: reflect real harness state rather than inventing a
  parallel source of truth.
- Honest delivery: distinguish discovered, queued, delivered, read, and replied
  states when those concepts appear.
- Local by default: keep transcripts and session data on the user's device in
  this build.
- Scan before drill-down: let a developer understand the fleet quickly, then
  inspect one card or project.

## Accessibility & Inclusion

The dashboard must support keyboard navigation, visible focus, reduced motion,
high-contrast text, and layouts that remain usable in narrow browser windows.
