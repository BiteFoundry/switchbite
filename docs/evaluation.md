# Evaluate recorded routes

Import one JSON file and inspect the report. Switchbite reads the data; your orchestrator owns execution, provider budgets, outcome verification, and any later policy changes. Evaluation launches no workers, calls no providers, and changes no live state.

The additive Rust API is:

```rust
pub fn evaluate(request: &serde_json::Value)
    -> Result<serde_json::Value, switchbite::ContractError>;
pub fn input_schema() -> serde_json::Value;
pub fn output_schema() -> serde_json::Value;
```

These functions live in `switchbite::evaluation`. The shared `ContractError::InvalidRequest` denotes malformed imports, bound violations, and inconsistent joins. The schemas describe structure; `evaluate` also validates relational constraints. This API does not alter docking API 1 or CLI-profile routing. CLI/MCP exposure belongs to the host integration; a host can expose `switchbite evaluate --request-file FILE` by parsing the file and calling `evaluate`.

## Smallest useful import

```json
{
  "kind": "observed",
  "cases": [{
    "id": "task-42",
    "task_type": "implementation",
    "sample": "local",
    "acceptable_candidate_ids": ["rust-worker"],
    "expected_hold": false,
    "baseline_candidates": {"manual": "generalist", "strongest": "premium"}
  }],
  "decisions": [{
    "case_id": "task-42",
    "status": "ready",
    "candidate_id": "rust-worker",
    "policy_id": "local-policy-v1"
  }],
  "observations": [{
    "case_id": "task-42",
    "candidate_id": "rust-worker",
    "outcome": "passed",
    "duration_ms": 1200,
    "quota_units": 3,
    "rework_count": 0
  }]
}
```

This example describes the import format; its numbers are illustrative. Missing baseline observations produce missing comparison metrics. Nothing executes to fill them in. Use `examples/evaluation-import-template.json` as an empty starting point and replace its placeholders with actual task and candidate IDs. It deliberately starts with `kind: "synthetic"`. Change that label only after collecting real records. `examples/evaluation-synthetic.json` demonstrates implementation, review, mockup, quick, and hold cases with synthetic imported decisions and outcomes, including incomplete coverage and a failed execution after a correct route.

## Collection contract for any orchestrator

1. Define each task and its acceptance check before running it. Record acceptable candidate IDs and whether safe behavior requires a hold. Freeze a manual role assignment and an always-strongest eligible candidate as the two baselines. Baseline IDs are supplied by the host; the evaluator does not guess model strength or choose a worker.
2. Export the actual Switchbite routing decision as `ready` plus `candidate_id`, or `hold` without a candidate. Keep `policy_id` and a short `reason` when available. The evaluator never synthesizes a routing decision. A missing decision stays missing.
3. The host may execute the selected candidate and baseline candidates in isolated, equivalent task environments. Export an observation only after checking its result. `outcome` must be `passed` or `failed`. Keep provider records and verification evidence outside this import. Cancellation or an unverified/unfinished result has no outcome record. Do not turn skipped work into failure or zero cost.
4. Join observations with the exact `case_id` and `candidate_id`. Include optional measurements only when recorded. Import the file and review coverage before interpreting differences.

All root fields (`kind`, `cases`, `decisions`, `observations`) are required. Unknown fields are rejected at every input object level. Optional means omitted, not `null`.

| Record | Required fields | Optional fields |
| --- | --- | --- |
| Case | `id`, `acceptable_candidate_ids`, `expected_hold`, `baseline_candidates` | `task_type`, `sample` |
| Decision | `case_id`, `status` | `candidate_id` (required for ready; forbidden for hold), `duration_ms`, `quota_units`, `rework_count`, `policy_id`, `reason` |
| Observation | `case_id`, `candidate_id`, `outcome` | `duration_ms`, `quota_units`, `rework_count` |

`kind` is `synthetic` or `observed`; it is preserved. `task_type` is `implementation`, `review`, `mockup`, or `quick`. `sample` is `bootstrap` or `local` and identifies the sampling cohort, independently of evidence kind. `baseline_candidates` contains optional `manual` and `strongest` IDs. It may be empty. Expected-hold cases require an empty acceptable list; other cases require at least one acceptable ID. Baselines may intentionally differ from acceptable candidates.

Case IDs are unique. At most one decision exists per case and one observation per `(case_id, candidate_id)`. Every decision/observation references an existing case. A candidate observation must reference an ID declared for that same case by its acceptable list, baseline assignments, or ready decision. A chosen but unacceptable candidate can therefore be evaluated as a route disagreement. Observation records for other declared, unselected acceptable candidates are accepted but do not count as Switchbite execution evidence.

Decision measurements describe the execution of its selected candidate, not routing latency or a predicted cost. They may be imported separately from verified outcomes. A hold has no execution measurements. An observation for the selected candidate supplements missing decision measurements; if both records provide the same metric they must agree numerically (`10` and `10.0` agree). Disagreement is rejected, rather than silently choosing a source. Repeated attempts belong inside one consistently defined task measurement; distinct trials require distinct case IDs.

Keep timing boundaries identical across strategies: for example, execution wall time through final verification. Define whether retries and verification consume quota and rework before collecting. `quota_units` is a host-defined measured unit, not an inferred currency price; compare only like provider units or a documented common normalization. `rework_count` is the recorded number of correction cycles under a fixed definition. Omit unknown data. No switch/handoff cost is inferred or subtracted; hosts can include real overhead in consistently measured elapsed time and quota.

Bounds are 1–32 cases, 0–32 decisions, 0–128 observations, and at most 16 distinct acceptable candidates per case. IDs and policy IDs have 1–128 characters; reasons have 1–512. Strings cannot be entirely whitespace. Duration and quota are finite numbers from 0 to 1,000,000,000,000; rework is an integer from 0 to 1,000,000. Imports additionally use existing contract bounds: 65,536 serialized bytes, depth 12, at most 8,192 nodes, and arrays of at most 128 elements. Split larger experiments into bounded batches; combine raw matched measurements before computing an overall mean, rather than averaging batch means.

At a byte boundary, hosts should use `contracts::parse_json(bytes, contracts::MAX_JSON_BYTES)` or `parse_request` to reject duplicate JSON object members. A `Value` has already discarded duplicate member information, so `evaluate(&Value)` cannot recover it. Record-level duplicates are always rejected by `evaluate`.

## Reading the report

The report contains `schema_version: 1`, the unchanged `kind`, `proposal_only: true`, and an `evidence` label of `synthetic_fixture` or `imported_unverified`. Even an observed import is not independently verified by this pure evaluator. It is never promoted to verified learning data.

`routing.agreement` measures recorded decisions against expected holds or acceptable candidates. Its `sample_count` is the number of decisions, `missing_count` is cases without decisions, `matched_count` is agreeing decisions, and `value` is matched/sample count. A ready decision on an expected-hold case disagrees, even if its execution passed.

`routing.hold_coverage` reports `expected_count`, `observed_count` (expected-hold cases with any decision), `correctly_held_count`, and `missing_count` (expected-hold cases without decisions). Its value is correctly held/expected count, so missing decisions cannot inflate hold coverage. When no case expects a hold its value is `null`. `unexpected_hold_count` counts holds on tasks expected to be routable.

`strategies.switchbite`, `.manual`, and `.strongest` contain selection counts and four metric summaries:

| Metric | Reported `value` | Source |
| --- | --- | --- |
| `success` | passed / observed outcomes | Joined observation only |
| `duration_ms` | arithmetic mean of recorded durations | Selected decision/observation for Switchbite; observation for baselines |
| `quota_units` | arithmetic mean of recorded quota | Same |
| `rework_count` | arithmetic mean of recorded corrections | Same |

Each summary contains its own `sample_count` and `missing_count`. These add to the total number of cases, including held cases and cases with missing baseline assignments. A value is `null` when no measurements exist; a real measured zero remains zero. `selected_count` and `missing_selection_count` distinguish absent candidate selection from absent measurement. Strategy means describe available evidence and can have different populations; do not subtract these means to compare policies.

`paired_comparisons.manual` and `.strongest` provide each metric's `sample_count`, `missing_count`, `case_ids`, `switchbite_mean`, `baseline_mean`, and `delta`. Each metric uses only tasks with both measurements for that metric. Delta is the mean of per-task **Switchbite minus baseline** differences: positive success is better; negative duration/quota/rework is lower measured use. Zero pairs produce null means and delta. A duration pair can exist without verified success; it says nothing about quality. Pairing coverage can differ across metrics.

If a baseline and Switchbite choose the same candidate, they share its one observation and contribute a zero difference. This is shared evidence, not an independent execution or repeated proof. An outcome without the corresponding Switchbite decision may support a declared baseline but cannot create a Switchbite sample.

`cases` is sorted by case ID and retains case labels, expectations, decision metadata, route agreement (null without a decision), and measurements for all three strategies. It supports slicing cohorts, examining disagreements, and manually proposing changes to policy. Input order does not affect the report. The evaluator performs no automatic tuning, live learning, or host mutation.

## Initial benchmark procedure and limits

Start with the synthetic fixture to verify the importer, schema, and missing-data handling. Synthetic numbers test software behavior; they provide **no real-world performance evidence**.

For an initial real benchmark, preselect a small balanced set of actual implementation, review, mockup, and quick tasks, plus explicit missing-capability/budget/unavailability hold cases. Freeze inputs, acceptance checks, candidate capabilities, baseline choices, policy version, provider/model versions, and measurement definitions. Record bootstrap versus local samples. Use independent acceptance checks (tests, review rubric, or approved mockup criteria), not the worker's claim of success. Keep tuning tasks separate from held-out evaluation tasks.

Run controlled trials through the host orchestrator. Set explicit per-provider spending/quota limits, per-task token/time/retry caps, maximum concurrent workers, and an overall stop budget before launching. Count baseline executions in that budget. Respect provider limits and record budget-related holds. A budget-exhausted or canceled run leaves missing evidence unless a predeclared acceptance check establishes failure. Routing/provider inference overhead, if relevant, must be recorded by the host under a consistent definition.

Reset task state between strategy runs and vary execution order to reduce cache/order effects. Record all selected trials, including verified failures and omitted measurements, without selecting only favorable outcomes. Repeat representative tasks with distinct trial IDs when the budget permits. Review per-metric paired task IDs and cohort coverage before accepting a proposed policy change. The host decides whether to test and apply it, and owns rollback.

This report is descriptive. It has no confidence intervals, causal guarantee, significance test, automatic unit normalization, provider verification, or adjustment for heterogeneous task difficulty. Small or selectively missing samples can bias conclusions. Passed/failed outcomes may miss quality differences; imported IDs and labels cannot establish equivalent environments or truthful measurements. Preserve experiment provenance outside this bounded schema and validate it before calibration. Hold accuracy, route agreement, and execution success answer different questions and should be reviewed together.
