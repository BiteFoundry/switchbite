# Coordinate with Switchbite

The `switchbite-stackbite` project server exposes `switchbite_start`, `switchbite_next`, and `switchbite_observe`. Start with the user's authorized goal, a factual worker pool, and optionally a plan. Decompose work into bounded tasks with objectives, dependencies, owned outputs, acceptance checks, and relevant context. Supply the plan when the workflow requests it.

Call next and carry out its actions using your available host tools. Record each result with its action ID, a stable event ID, and evidence. Route decisions use the configured Jev or Laya judge and preserve its exact candidate identity. Execute independent tasks concurrently only within the workflow's reservations. Inspect unknown executions rather than launching duplicates. Verify worker completion and integrate the result before dependent work begins. Gather missing context and follow bounded repair actions when checks or execution fail. Continue this execution, verification, and integration loop until the workflow reports completion or needs user input.

Use next with `operation` set to `status`, `pause`, `resume`, or `cancel` to inspect or control a run. Cancellation can return actions for running work; carry them out and report the outcomes. Do not invent worker capabilities or completion evidence. A failed judge does not switch providers automatically.

The legacy agent profile remains available for direct routing, plan validation, feedback, and status.
