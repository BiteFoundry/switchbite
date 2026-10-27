# Changelog

## 0.8.3-live.2 — 2026-10-27 (branch preview)

- Coordinator instructions specify node field types and an executable example.
  A single acceptance string is canonicalized to one list item before registration.
- Codex workers use non-login shells, avoiding PowerShell profile side effects.
- Command start, update and completion events update the same visible entry;
  separate executions of the same command remain separate.

## 0.8.3-live.1 — 2026-10-27 (branch preview)

- Live output for coordinator, worker and check processes, with last activity,
  elapsed time, owned-process liveness and quiet/input-needed indicators.
- Native execution checkpoints and retained bounded output on completion,
  failure, cancellation and timeout. Check streams use unique action IDs.
- Authenticated activity API, provider-event extraction and secret masking.
- Automatic active-stream following, manual selection, scroll retention and
  explicit worker interruption followed by the existing repair workflow.

## 0.8.2-tools.1 — 2026-10-27 (branch preview)

- Six-tool inventory and checkbox selection in the web composer, with automatic
  saving, refresh and preservation of temporarily unavailable preferences.
- The run captures selected providers for both coordination and worker routing;
  later catalog refreshes retain that scope. Any supported selected CLI can plan.
- Native sandboxed Antigravity (`agy`) execution with matching conversation and
  terminal receipt validation; a real file-contract task passed and integrated.
- Grok discovery reports installation and model information; its execution
  adapter is pending qualification. The web uses existing CLI authentication to
  create new task sessions.

## 0.8.1-web.1 — 2026-10-27 (branch preview)

- `workflow app` accepts a project and goal in the browser, runs a native
  coordinator for planning and repairs, and drives the existing workflow engine.
- Live graph, pause/resume/cancel, explicit retry and final result worktree,
  branch and HEAD. Existing single-run viewer remains available.
- Coordinator scratch branches use a separate namespace from task integration.
- HTTP end-to-end qualification uses an isolated fake CLI; live routing and native
  worker qualification from the 0.8 pool preview are recorded separately.

## 0.8.0-pool.1 — 2026-10-27 (branch preview)

- Automatic Claude, Codex, Cursor and OpenCode model catalogs, per-task shortlists
  and exact provider/model/effort pins; explicit pools remain available.
- Routing context includes acceptance, outputs, upstream integration and actual
  harness/model/effort facts using the existing judgment contract.
- Native Cursor and OpenCode adapters alongside the existing Claude/Codex path.
- One-command connections use the workflow profile with durable workspace lookup
  across projects; `models --refresh` inspects installed targets.
- Candidate snapshots preserve in-flight decisions across pool changes and
  recheck changed target facts before dispatch.
- This package implements W0–W6 of the routing expansion plan. ACP/Herdr docking,
  event-stream UI, outcome-based adaptation and PR/CI feedback are the next stages.

## 0.7.0-workflow.1 — 2026-10-27 (branch preview)

- Three-tool workflow MCP profile and native CLI runner, with background progress
  for supplied DAGs after one next call; existing individual routing stays available.
- Durable task state, resource/output reservations, role-aware Jev/Laya routing,
  independent review, compact event history and factual worker-pool refresh.
- Owned Git worktrees per attempt, structured Claude/Codex execution, output and
  command checks, serialized integration and interrupted-transaction recovery.
- Windows job-owned process trees, cancellation and stopped-owner restart recovery.
- Bounded context gathering and repair handoff, plan observations and resumable runs.
- Loopback workflow graph with progress, assignment, evidence and run controls.
Validation on 2026-10-27 used one synthetic three-task project with live Jev
routing and real Claude and Codex workers on Windows. Two implementation tasks ran independently; a
Codex review followed their integration. One task failed its first check and was
retried after a coordinator-supplied repair and shell fix. All three tasks then
passed the supplied checks and integrated. This is a workflow smoke test, not a
comparison of model quality or a claim about other projects.

## 0.6.0-stackbite.1 — 2026-10-25 (branch preview)

- Stackbite coordinator project installation defaults to Jev 1.14, with explicit
  local Laya selection and credential-name forwarding without storing key values.
- Separate `switchbite-stackbite` MCP server avoids generic user-server name
  collisions; preserves existing connections and supports judge switching.
- Coordinator initialization covers decomposition, DAG validation, ready-task
  routing, host reservations, parallel output ownership, verified receipts and replanning.
- Packaged Korean setup guide and contract-checked synthetic model-pool workflow.
- Opt-in installed-MCP Jev qualification; no native host changes or worker execution
  claims. Existing standalone Studio remains separate.

## 0.6.0-laya.1 — 2026-10-25 (branch preview)

- Project-scoped Stackbite coordinator MCP installation for Codex and Claude,
  preserving existing configurations and reporting partial writes accurately.
- Explicit local-only Laya transport, with no proxy, redirects, cloud keys or
  Jev fallback; exact model validation and model-bound replay snapshots.
- Canonical score concentration from validated probabilities; absent user
  preferences are deterministic neutral policy, not extra model questions.
- Local probe, installation guide, real installed-MCP qualification and packaged
  Windows preview. Domain learning, CUDA tuning and worker execution are separate.

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
