# Routing API2

The recommended MCP surface is `switchbite_route` in `serve --profile agent`.
The normal schema requires `task` and `pool`; advanced replay fields are hidden.
The native `route --request-file FILE` command and Rust `routing::route` use the
advanced input schema. The pure Rust function receives `now_ms` explicitly.

`task` requires `id` and `objective`; `requirements` and `context_refs` default
to empty arrays. Context references are labels, not files fetched by the plugin.
Pool targets require `id`, `description`, `capabilities`, `available`. Optional
observed estimates are `estimated_cost`, `estimated_latency_ms`, `has_context`.
At most 16 targets are accepted. Unknown fields and invented default availability
are not supported. Global 64 KiB/depth/value bounds still apply.

## Policy

`policy.profile` selects balanced/quick/implementation/review/mockup defaults.
Explicit settings override the profile. Weights are normalized; their total must
be positive. Fields include `fit_weight`, `preference_weight`, `min_fit`,
`min_context`, `min_confidence`, `tie_margin`, `tie_behavior`, `switch_margin`,
`switch_penalty`, `excluded_candidate_ids`, `producer_candidate_id`, `max_cost`,
`max_latency_ms`, `id` and `judgment_validity_ms`. The embedded schema specifies
each range. Profile meanings are policy defaults, not measured vendor rankings.

Unavailable, missing-capability, excluded/producer and over-budget/latency targets
are gated before judging. A missing estimate cannot satisfy a declared hard limit.
Jev supplies typed fit/preference judgments. API2 validates their complete
distributions using the API1 core, then applies its own thresholds and ranking;
API1's fixed hold thresholds do not override an explicit API2 policy.

Concentration thresholds apply to fit and any used preference axis. Qualifying
targets are ranked by weighted fit/preference minus an applicable switch penalty.
Continuity may retain the current qualified target within `switch_margin`.
`has_context=false` disables that affinity. `continuity.in_progress=true` holds
before inference, even with an empty pool; no running assignment is migrated.

Ties within `tie_margin` hold by default. Explicit `stable_id`, `lowest_cost` or
`lowest_latency` policies resolve them. Estimate-based policies require observations
for every tied contender; no missing value is treated as zero.

## Snapshots and replay

Normal calls construct their own snapshot. `snapshot --request-file FILE` computes
one offline for host tooling. Binding hashes canonical normalized task, full pool,
preferences, effective policy and continuity. Order and explicit defaults normalize
consistently. The default validity is 60 seconds.

Advanced replay contains `input_sha256`, `expires_at_ms`, and a typed
`judgment`. It must match current normalized inputs, remain unexpired and not
extend beyond the current policy validity horizon. Mismatched, expired or excessive
future validity holds with `refresh_judgment` before any provider call. Replaying
preserves the original expiry. A hash is not authentication: trusted host tooling
must bind the original judgment and provenance, and recheck facts before execution.

An explicitly configured non-Jev judge adds `judge_model` to its snapshot and
includes that identity in the input hash. Its judgment must name the same model;
cross-model replay is refused. Legacy Jev snapshots and hashes remain unchanged.

## Response and agent behavior

Responses have `version`, `status`, `candidate_id`, `next_action`, `summary`,
`reason_codes`, `selected`, `alternatives`, `excluded`, `snapshot`.
Selected/alternative entries include deterministic policy IDs and score breakdowns.
At most three alternatives are returned. Only `recheck_and_dispatch` proposes host
rechecking/reservation. All other actions hold. No proposal is an authorization
receipt, execution request or completion proof.

Runtime errors are MCP error results with static `code`, `message`, `retryable`,
bounded `retry_after_ms`, and machine-readable `next_action`. They never echo a
provider body or credential. Retry guidance concerns the routing judge, not
repeating worker side effects. Pending provider work may settle after cancellation;
the runtime keeps its concurrency slot occupied until actual exit.

API1 remains unchanged at `describe`/`invoke` and `serve --profile cli`; its replay
format still relies on host freshness checks. Migrate coordinators to the agent
profile and API2 to use bound replay and configurable policy.
