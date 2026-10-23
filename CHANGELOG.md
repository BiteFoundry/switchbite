# Changelog

## 0.4.0 — 2026-10-23

- Recommended four-tool agent profile, coordinator instructions and read-only setup handoff.
- Single-call routing API2 with concise next actions and readable MCP summaries.
- Bound, expiring replay snapshots; configurable role policies, hard estimate limits,
  producer exclusion, tie policies and context continuity without migrating running work.
- Bounded concurrent decisions, finite queue/deadlines, cancellation-safe permits,
  reusable HTTP connections, classified static errors and per-judge cooldowns.
- Offline observed-result evaluation with missing coverage and paired baseline comparisons.
- Node24 GitHub actions and a separate minimum-Rust check before publishing.

Existing docking API1 and the original CLI profile remain available. Actual model
discovery, worker execution and receipt verification remain host-owned.

## 0.3.0 — 2026-10-23

First public preview under BiteFoundry, named Switchbite.

- Native Rust library and Windows x64 MCP stdio executable.
- Typed worker/model-pool decision tools and generic docking API 1.
- Jev 1.14 judgments, deterministic eligibility/ranking and explicit holds.
- Supplied DAG validation and stateless feedback proposals.
- Configuration snippets for six CLI clients without editing their settings.
- Model-pool replay example and regression coverage.
- MIT license, dependency notices, deterministic ZIP packaging and Windows CI.

This is a curated source snapshot. Earlier private prototype history, Python
code and obsolete host deployment records are not included. Direct orchestrator
adapters, automatic planning/execution and other decision backends are not shipped.
