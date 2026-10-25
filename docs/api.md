# Docking API 1

`schemas/docking-v1.json` is embedded in the library and returned by `describe`.
Use MCP with no process arguments for `describe()` and `invoke(operation, request)`.
Use `serve --profile cli` for the four typed `switchbite_*` tools. Typed tools
default omitted `version` to 1 and task/candidate extensions to `{}`. Generic
`invoke` and the JSON CLI require the full domain input.

## Decisions

Input: `version`, `task`, `candidates`, `preferences`, optionally `judgment`.
A task contains `id`, `objective`, `requirements`, `context_refs`, `extensions`.
A candidate contains `id`, `description`, `capabilities`, `available`, `extensions`.
Candidate IDs must be unique. The pool limit is 16; requirements and capabilities
match exact strings. Extensions are opaque and excluded from provider state.

Only available candidates covering all requirements are judged. Jev receives
task facts, preferences and eligible candidate descriptions/capabilities. Question
keys are `fit_c` / `preference_c` followed by SHA-256 of each exact UTF-8 candidate
ID. Input ordering does not alter candidate identity or question mapping.

Validated scores are ranked by `0.7 * fit + 0.3 * preference`, then fit and lexical
ID. Fit must reach 0.5, context sufficiency 0.5 and selected fit confidence 0.3.
Ties within 0.02 are reported. Probability sums, expected scores, concentration
and optional legends are checked. Confidence is not calibrated correctness.
The default judge response model is `jev-1.14.0`. A host may explicitly configure
another judge identity; the request, response and reported model must match that
identity exactly. Local Laya is opt-in and never impersonates Jev. The local
adapter derives canonical concentration from its probability distribution;
this is not empirical correctness calibration.

Only `ready=true` produces an assignment and non-null top-level `candidate_id`.
Held results include reasons and next actions; `scores.recommended` may still
contain a diagnostic candidate and must not be used to dispatch a held decision.
Errors use MCP `isError=true` with static `error.code` and `error.message`.

The caller must bind replay judgments to their original task, candidate facts and
provenance. Eligible IDs alone cannot prove freshness after text changes.
`scores.request_sha256` hashes the canonical provider body for host auditing;
it is not a signature or authorization receipt.

## Plans and feedback

`validate_plan` checks supplied nodes/dependencies, acceptance criteria and output
references. Cycles, missing/self dependencies and duplicate IDs hold. Valid
graphs produce a deterministic topological proposal. It does not generate or run
tasks, allocate workers or optimize scheduling.

`feedback` requires an observed outcome and nonempty evidence. It returns an
event proposal without persisting, verifying host receipts or learning weights.

## Bounds and execution

Domain request/response JSON: 65,536 UTF-8 bytes, depth 12, 8,192 values and at most
128 members/items per container; finite numbers and signed 64-bit integers only.
Unknown fields are refused except bounded opaque extensions. Generated provider
bodies and MCP frames are limited to 131,072 bytes. Malformed frames and duplicate
JSON object keys close the stream. Errors do not echo request data or secrets.

CLI profile permits up to four concurrent calls and one decision. Docking profile
permits one call. Excess calls return `busy`; hosts should wait for pending calls
to settle. These are per-process limits, not orchestration-wide reservations.

`Plugin::default()` is offline and reads no environment. Rust callers can use
`Plugin::with_judge`. The executable optionally loads `TYPESAFE_API_KEY`; a host
must withhold it unless it authorizes provider access. Plan/feedback/status and
supplied replays never need the API. Jev is the only bundled decision backend.
