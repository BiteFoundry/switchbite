# Spec workflow implementation contract

Base: a7ccf84. Integration branch: feature/spec-workflow. All implementation is
Rust, plugin-only. Original Stackbite source/install/runtime and existing seats
are preserved. Private clone sources are not copied into this public repository.

## Public Rust entry points (owned by core worker)

`workflow::Workflow::new(root: PathBuf) -> Self`

Methods accept strict bounded JSON data and return `Result<Value, WorkflowError>`:

- `start(&self, request: &Value)`
- `next(&self, run_id: &str)`
- `observe(&self, run_id: &str, event: &Value)`
- `status(&self, run_id: &str)`
- `control(&self, run_id: &str, action: &str)` (pause/resume/cancel)

WorkflowError implements Error/Display with sanitized code, and `to_value()`.

Start request: `{workspace, goal?, plan?:{nodes:[...]}, pool:[...], preferences?,
max_parallel?:2, repair_rounds?:2, checks?:[{program,args:[]}], host?:{...}}`.
Plan nodes reuse id/objective/depends_on/requirements/acceptance/outputs, adding
optional role (exploration/implementation/mockup/integration/review), context_refs.
Candidate profiles reuse route-v2 id/description/capabilities/available and add
optional provider/model/effort/executable/resource_key/args. Metadata is stripped
before calling existing routing::route. IDs and resource reservations are internal.

Response: `{run_id,status,summary,tasks:[...],actions:[...],events:[...],progress}`.
Actions: `{id,kind,run_id,task_id?,payload:{...}}`. Kinds: plan, route, prepare,
launch, inspect, verify, integrate, gather_context, repair, review.
`next` persists action leases; repeated calls return outstanding actions with the
same IDs, never create duplicate launches. Disjoint tasks can have outstanding
actions together within max_parallel/resource/output reservations. Artifact refs
are separate from repository write paths; only actual owned paths conflict.
An observation is `{action_id,event_id,result:{...}}`. Duplicate identical events
are idempotent; changed payload under the same ID conflicts. Result fields:

- plan/repair: `{plan:{nodes:[...]}}` or repaired task details (document exact form).
- route: existing ready/hold routing output, candidate_id exact.
- prepare: `{status:"prepared",workspace,branch,integration_workspace,integration_branch}`.
- launch/inspect: `{status:"running"|"completed"|"failed"|"unknown",execution_id?,completion_verified?,...}`.
- verify: `{status:"passed"|"failed",evidence:[...],...}`.
- integrate: `{status:"integrated"|"failed",commit?,evidence:[...],...}`.
- gather_context: `{status:"gathered",context_refs:[...],summary?}`.

Only verified completion advances to verifying. Only verification plus successful
integration/publication releases descendants. Missing context creates gather_context;
known failure creates bounded repair. Unknown execution creates inspect and keeps
its reservation. Independent work continues. Pause stops new dispatches; cancel
returns cancellation actions for owned running work (agree exact action with host).
Persist events/state with serialized mutations and atomic snapshots; recover from
process restart without silently re-running non-idempotent worker actions.

## Host adapter API (owned by host worker)

`adapters::LocalHost::new(workspace: &Path, state_dir: &Path, config: &Value)
 -> Result<Self, HostError>`.
`LocalHost::describe(&self) -> Value`
`LocalHost::perform(&self, action: &Value) -> Result<Value, HostError>`.
HostError implements Error/Display plus `to_value()`.

Host action payload always receives `workspace` (original workspace), `task`
(full node), `candidate` (full profile), `checks`, and all prepare-result fields
under `prepared` where available. Host result shapes above are fixed.
Implement owned git integration/task worktrees from an explicit base, structured
Claude and Codex CLI invocation, receipt parsing, bounded argv-based checks,
integration, inspect/cancel and context gathering. Reuse known private clone
execution protocol through its explicit binary/state when configured; do not
edit that clone or original app. No ambient Stackbite state discovery/mutation.
Execution receipts distinguish model completion, checks, and integration.
All state is task-owned; no secret values in public receipts. Key forwarding to
workers is not required for judgment. Avoid shell-composed commands.

## Interface (owned by interface worker)

Add `serve --profile workflow`: three tools switchbite_start/next/observe.
Workflow root comes from explicit `--workspace` or cwd; store `.switchbite/workflow`.
Pass configured Jev/Laya identity unchanged. Three calls drive core methods; next
can resolve route actions with existing route_with_judge and return subsequent
host/coordinator actions. Coordinator owns plan/replan meaning.
CLI: workflow start --request-file FILE; workflow next/status RUN; workflow
observe RUN --request-file FILE; workflow pause/resume/cancel RUN.
Plan CLI and MCP input error paths are bounded and sanitized. Preserve legacy
agent/docking/cli profiles. Stackbite installer selects workflow profile, judge
selection and stable server name stay unchanged. Main and mcp_server single owner.

Root coordinator owns automatic runner orchestration and cross-module integration,
release version, packaging, public verification. Communicate contract corrections
before changing cross-scope APIs. Build output existing path:
C:/Users/user/source/Switchbite/.switchbite/laya-build. Use login:false for shell.
