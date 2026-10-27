//! Workflow-facing adapter shared by CLI and MCP.
use crate::{agent, workflow::Workflow};
use rmcp::model::Tool;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub const INSTRUCTIONS: &str = "Coordinate the user's authorized work. Decompose the goal into bounded tasks with dependencies, owned outputs and acceptance checks, or provide a plan when requested. Start the workflow with the factual worker pool. Call next, perform the returned actions with the host, and observe each action using a stable event ID and evidence. Continue independent tasks in parallel where permitted. Verify completed work and integrate it before unblocking dependants. Gather missing context and perform bounded repairs when requested. Inspect unknown executions instead of launching duplicates. Pause or cancel only when needed; follow the workflow's actions until the goal is complete.";

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new("switchbite_start", "Start an authorized workflow with a goal or plan and a factual worker pool.", json!({"type":"object","properties":{"request":{"type":"object"}},"required":["request"],"additionalProperties":false}).as_object().unwrap().clone()),
        Tool::new("switchbite_next", "Get pending workflow actions, or read status and control a run.", json!({"type":"object","properties":{"run_id":{"type":"string"},"operation":{"type":"string","enum":["next","status","pause","resume","cancel"]}},"required":["run_id"],"additionalProperties":false}).as_object().unwrap().clone()),
        Tool::new("switchbite_observe", "Record an action result with stable action and event IDs.", json!({"type":"object","properties":{"run_id":{"type":"string"},"event":{"type":"object"}},"required":["run_id","event"],"additionalProperties":false}).as_object().unwrap().clone()),
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
