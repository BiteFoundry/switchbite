# Model pools and multi-agent orchestration

For current v0.4 policy, estimate limits, continuity, bound snapshots and the
recommended four-tool agent profile, see [API2](api-v2.md) and
[the coordinator guide](coordinator.md). The capability table and example below
record the v0.3/API1 interface, which remains available for compatibility.

Switchbite's `decide` operation already accepts an arbitrary candidate pool.
Candidate IDs have no built-in provider or CLI meaning. Up to 16 candidates can
be evaluated in one request. A pool can contain multiple models/settings for the
same harness, with distinct IDs for each actual executable target.

## What exists in v0.3.0

| Capability | State |
| --- | --- |
| Accept the host's model/worker pool | Implemented: `candidates`. |
| Filter unavailable or unsupported targets | Implemented: explicit `available` and exact capability strings. |
| Rank by task fit and user preference | Implemented: typed Jev scores, fixed weights and hold thresholds. |
| Preserve exact target identity | Implemented: opaque IDs; host maps the returned ID back to the candidate. |
| Abstain on missing context/judge, low fit/confidence | Implemented: hold reasons and no assignment. |
| Discover installed models, accounts, quotas or costs | Not implemented; host supplies facts. |
| Interpret numeric budget, latency or cost constraints | Not implemented. Opaque extensions do not influence ranking. |
| Optimize simultaneous assignments across a DAG | Not implemented; one task and one proposed assignment per call. |
| Dispatch workers, reserve capacity or replan automatically | Not implemented; host responsibility. |
| Learn/calibrate from feedback | Not implemented; feedback returns an event proposal only. |
| Native Herdr/Orca/Stackbite adapters | Not shipped or integration-tested. |

The algorithm recommends the highest-ranked eligible candidate under supplied
facts and policy. No measured optimality across real tasks/models is claimed.
The decision **judge** (Jev 1.14 by default; explicit local Laya on this branch) is separate from the candidate models
that would perform the task (for example, a Codex or Claude worker).

## Minimal host integration

1. Snapshot currently permitted targets from the orchestrator's own registry.
2. Give each `harness + model + settings + account/worker instance` combination a
   stable opaque ID, description, capabilities and factual availability.
3. Apply hard policy locally first: budget, permissions, quota, workspace claims,
   region and any host-specific requirements. Remove ineligible targets or mark
   them unavailable. Do not place secrets in descriptions, preferences or IDs.
4. Call `decide` via the Rust library, native JSON CLI, or MCP `invoke`. CLI agents
   can instead call `switchbite_decide` with the same fields.
5. If `ready` is false or the call fails, honor the hold. Do not dispatch the
   `scores.recommended` diagnostic on a held result.
6. If ready, resolve `candidate_id` against that exact snapshot, recheck current
   authorization/capacity, reserve the target atomically and dispatch through the
   host's existing execution path. A proposal is not a capacity reservation.
7. Observe the host's completion receipts. Optionally normalize settled outcomes
   through `feedback` and persist the returned event in the host.

```text
Herdr / Orca / Stackbite / another host
  -> permitted target pool + task + preferences
  -> Switchbite decide (capability gate -> Jev -> ranking/holds)
  -> candidate_id or hold
  -> host recheck/reserve/execute/observe
```

This is a transport-neutral integration recipe, not a claim that a particular
host currently loads the plugin automatically. Registering Switchbite in the
coordinator agent's MCP configuration makes the decision tools available to that
agent; it does not replace the orchestrator's scheduler or grant host access.

## Example and verification

`examples/model-pool-replay.json` contains synthetic host-supplied targets,
including two Codex profiles, an implementation worker, a busy worker and a
candidate missing a required capability. The replay is illustrative, not a
benchmark of the named products. It selects `pool:claude:implementation` offline:

```powershell
.\bin\switchbite.exe invoke decide --request-file examples\model-pool-replay.json
```

The example is available in the source checkout. Delete the `judgment` field to
request a fresh Jev assessment with a host-authorized `TYPESAFE_API_KEY`. Refresh
all candidate facts first. Model names/settings in extensions are host metadata;
the description and capabilities must state any qualities the judge should see.

Regression tests verify distinct profiles within one harness, exact target IDs,
exclusion before judging, snapshot immutability, input-order independence and
refusal of a replay whose eligible candidate set has changed.
