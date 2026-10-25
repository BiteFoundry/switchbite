# Coordinate with Switchbite

You decompose the user's authorized project; the configured judge (Jev or local
Laya) evaluates each task against the supplied model pool. Before delegating:

1. Create bounded tasks with an objective, acceptance checks, owned outputs and
   `depends_on` IDs. Validate the DAG with `switchbite_validate_plan`.
2. Call **this server's** `switchbite_status`. With the Stackbite installer its
   name is `switchbite-stackbite`, separate from a user's generic `switchbite`.
3. Route only tasks with no dependencies or with verified successful receipts
   for all dependencies. Parallel tasks need separate worker reservations and
   non-conflicting outputs. Keep that state in the host, not in Jev.

For every **newly ready** assignment:

1. Snapshot the host's permitted targets. A target is one harness/model/settings/
   worker-instance combination. Supply its actual ID, capabilities and availability.
2. Call `switchbite_route` once with `task` and `pool`. Put acceptance checks and
   relevant evidence into `objective`/`context_refs`; use defaults initially.
3. Read `status`, `next_action`, `summary` and `candidate_id` first.
4. On `recheck_and_dispatch`, resolve the exact ID in that snapshot, recheck
   authorization and capacity, reserve it atomically, and dispatch through the host.
5. For every other action, honor the hold and perform the requested next step.
   A diagnostic alternative is never permission to dispatch.
6. Observe completion using host receipts; use `switchbite_feedback` to normalize
   settled outcomes. Store those events in the host if desired.

Do not invent capabilities, availability, costs, model access or receipts. Do not
dispatch, retry or migrate a running assignment based on a routing recommendation.
At a task boundary, pass continuity to account for existing context. For review,
identify the producer so the same worker can be excluded. Budget/latency limits
are hard constraints; unknown estimates cannot prove a candidate meets them.
Use `policy.producer_candidate_id` for independent review. Update the factual
pool after every reservation/completion. Never route a blocked descendant early.
Replan only unfinished tasks within the user's authorization, retain evidence
for completed/running work, and validate the changed DAG again.

The normal agent interface requires no digest calculations or replay envelopes.
Advanced native snapshot/replay tooling binds stored judgments to their inputs;
it does not provide execution authority. Independent model accounts and quotas,
workspace claims and reservations remain the host's responsibility.

If routing fails, follow the static error's retry guidance. Never treat an unknown
worker outcome as a failed execution that can safely be repeated. Provider retry
guidance concerns the **routing judge**, not replaying a worker's side effects.

Switchbite validates supplied DAGs; it does not decompose a project, discover
installed models, execute a workflow, or certify that a worker's task succeeded.
