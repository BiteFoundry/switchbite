//! Workflow-facing adapter shared by CLI and MCP.
use crate::{agent, workflow::Workflow};
use rmcp::model::Tool;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub const INSTRUCTIONS: &str = "Coordinate the user's authorized work. Start with a factual worker pool and a goal or a plan. Decompose a goal into bounded tasks when a plan action is returned; give each task an ID, objective, dependencies, acceptance checks and write paths. Execute independent work within reservations. Call next and act on its returned actions. With a host configuration or complete local worker metadata, next can drive supported host actions; otherwise perform them with your host and observe results. Observe plan or repair actions with a plan or task and stable action_id and event_id. Verify completed work and integrate it before unblocking dependants. Gather missing context and perform bounded repairs. Inspect unknown executions rather than launching duplicates. Continue until complete or an actionable input is needed.";

pub fn tools() -> Vec<Tool> {
    let strings = json!({"type":"array","maxItems":64,"items":{"type":"string","maxLength":1000}});
    let node = json!({"type":"object","additionalProperties":false,"required":["id","objective"],"properties":{
        "id":{"type":"string","description":"Unique task ID in this plan; referenced by depends_on."},
        "objective":{"type":"string","description":"Bounded work with enough context for the assigned worker."},
        "depends_on":{"type":"array","items":{"type":"string"},"description":"Task IDs that must be verified and integrated first."},
        "role":{"type":"string","enum":["exploration","implementation","mockup","integration","review"]},
        "requirements":strings,"acceptance":{"type":"array","items":{"type":"string"},"description":"Observable task acceptance checks."},
        "outputs":{"type":"array","items":{"type":"string"},"description":"Output or artifact references. Use write_paths for repository ownership."},
        "write_paths":{"type":"array","items":{"type":"string"},"description":"Relative repository paths owned by this task; overlapping paths serialize work."},
        "context_refs":{"type":"array","items":{"type":"string"},"description":"Relevant context references."}
    }});
    let plan = json!({"type":"object","additionalProperties":false,"required":["nodes"],"properties":{"nodes":{"type":"array","minItems":1,"maxItems":128,"items":node}}});
    let candidate = json!({"type":"object","additionalProperties":false,"required":["id","description","capabilities","available"],"properties":{
        "id":{"type":"string","description":"Exact unique worker/model/settings ID."},"description":{"type":"string","description":"Factual strengths and constraints."},
        "capabilities":{"type":"array","items":{"type":"string"}},"available":{"type":"boolean"},
        "provider":{"type":"string","description":"Actual worker harness, e.g. codex or claude when using the local host."},
        "model":{"type":"string","description":"Exact model identity available to this worker."},"effort":{"type":"string"},
        "executable":{"type":"string","description":"Worker executable for the configured host."},"resource_key":{"type":"string","description":"Shared resource identity to prevent conflicting reservations."},
        "args":{"type":"array","items":{"type":"string"}},"estimated_cost":{"type":"number"},"estimated_latency_ms":{"type":"number"},"has_context":{"type":"boolean"}
    }});
    let start = json!({"type":"object","additionalProperties":false,"required":["request"],"properties":{"request":{
        "type":"object","additionalProperties":false,"required":["workspace","pool"],"anyOf":[{"required":["goal"]},{"required":["plan"]}],"properties":{
            "workspace":{"type":"string","description":"Absolute project workspace path."},
            "goal":{"type":"string","description":"Authorized goal; without a plan, the next action requests a plan."},"plan":plan,
            "pool":{"type":"array","maxItems":16,"items":candidate,"description":"Factual available worker profiles; do not invent availability or identities."},
            "preferences":{"type":"string"},"max_parallel":{"type":"integer","minimum":1,"maximum":16,"default":2},"repair_rounds":{"type":"integer","minimum":0,"maximum":10,"default":2},
            "checks":{"type":"array","maxItems":32,"items":{"type":"object","additionalProperties":false,"required":["program"],"properties":{"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}}}},
            "host":{"type":"object","description":"Optional local host configuration. A host object or complete provider/executable/model/effort worker metadata enables local host actions; otherwise actions go to the coordinator.","properties":{"base":{"type":"string","description":"Git base reference; defaults to HEAD."},"timeout_seconds":{"type":"integer","minimum":1,"maximum":3600}}}
        }}},"examples":[{"request":{"workspace":"/path/to/project","goal":"Implement the approved feature","pool":[{"id":"codex-worker","description":"Available implementation worker","capabilities":["implementation"],"available":true}]}}]});
    let observe = json!({"type":"object","additionalProperties":false,"required":["run_id","event"],"properties":{
        "run_id":{"type":"string"},"event":{"type":"object","additionalProperties":false,"required":["action_id","event_id","result"],"properties":{
            "action_id":{"type":"string","description":"Exact action ID from the workflow."},"event_id":{"type":"string","description":"Stable unique receipt ID; repeat the same event unchanged to retry safely."},
            "result":{"type":"object","description":"Action-specific result. Plan: {plan:{nodes:[...]}}. Repair: {task:{...}} or {plan:{nodes:[...]}}. Host actions return their specified status and evidence.","properties":{
                "plan":plan,"task":node,"status":{"type":"string"},"candidate_id":{"type":"string"},"execution_id":{"type":"string"},"completion_verified":{"type":"boolean"},"evidence":{"type":"array","items":{}},"context_refs":{"type":"array","items":{"type":"string"}},"workspace":{"type":"string"},"branch":{"type":"string"},"integration_workspace":{"type":"string"},"integration_branch":{"type":"string"},"commit":{"type":"string"}
            }}
        }}},"examples":[{"run_id":"RUN_ID","event":{"action_id":"PLAN_ACTION_ID","event_id":"plan-1","result":{"plan":{"nodes":[{"id":"build","objective":"Implement the feature","depends_on":[],"role":"implementation","acceptance":["Feature checks pass"],"write_paths":["src/feature.rs"]}]}}}}]});
    vec![
        Tool::new("switchbite_start", "Start a workflow with a goal or plan and a factual worker pool. Run and action IDs and scheduling defaults are generated automatically.", start.as_object().unwrap().clone()),
        Tool::new("switchbite_next", "Advance a run and return actions. Operation defaults to next; status, pause, resume and cancel inspect or control the same run.", json!({"type":"object","properties":{"run_id":{"type":"string","description":"Run ID returned by start."},"operation":{"type":"string","enum":["next","status","pause","resume","cancel"],"default":"next"}},"required":["run_id"],"additionalProperties":false}).as_object().unwrap().clone()),
        Tool::new("switchbite_observe", "Record a plan, repair or host action result with its exact action ID and a stable event ID; identical retries are idempotent.", observe.as_object().unwrap().clone()),
    ]
}
pub struct Interface {
    workflow: Workflow,
    root: PathBuf,
    judge: Option<Arc<agent::ConfiguredJudge>>,
}
impl Interface {
    pub fn new(workspace: PathBuf, judge: Option<Arc<agent::ConfiguredJudge>>) -> Self {
        let root = workspace.join(".switchbite").join("workflow");
        Self {
            workflow: Workflow::new(root.clone()),
            root,
            judge,
        }
    }
    pub fn start(&self, request: &Value) -> Result<Value, Value> {
        self.workflow.start(request).map_err(|e| e.to_value())
    }
    pub fn observe(&self, run: &str, event: &Value) -> Result<Value, Value> {
        self.workflow.observe(run, event).map_err(|e| e.to_value())
    }
    pub fn status(&self, run: &str) -> Result<Value, Value> {
        self.workflow.status(run).map_err(|e| e.to_value())
    }
    pub fn control(&self, run: &str, action: &str) -> Result<Value, Value> {
        self.workflow.control(run, action).map_err(|e| e.to_value())
    }
    pub fn next(&self, run: &str) -> Result<Value, Value> {
        let request = self.workflow.request(run).map_err(|e| e.to_value())?;
        let local = request.get("host").is_some_and(Value::is_object)
            || request["pool"].as_array().is_some_and(|pool| {
                !pool.is_empty()
                    && pool.iter().all(|c| {
                        ["provider", "executable", "model", "effort"]
                            .iter()
                            .all(|k| c.get(k).is_some_and(Value::is_string))
                    })
            });
        if local {
            return crate::workflow_runner::advance(&self.root, run, self.judge.clone())
                .map_err(|e| e.to_value());
        }
        let state = self.workflow.next(run).map_err(|e| e.to_value())?;
        let routes: Vec<Value> = state["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["kind"] == "route")
            .cloned()
            .collect();
        if routes.is_empty() {
            return Ok(state);
        }
        for action in routes {
            let id = action["id"].as_str().ok_or_else(invalid)?;
            let result = agent::route_with_judge(
                &action["payload"]["route_input"],
                self.judge.clone(),
                agent::now_ms(),
            )
            .map_err(|e| e.to_value())?;
            self.workflow
                .observe(
                    run,
                    &json!({"action_id":id,"event_id":format!("route:{id}"),"result":result}),
                )
                .map_err(|e| e.to_value())?;
        }
        self.workflow.next(run).map_err(|e| e.to_value())
    }
}
pub fn invalid() -> Value {
    json!({"error":{"code":"invalid_request","message":"Invalid workflow request."}})
}
