//! Small, opinionated entry point for coordinator agents. No host execution.
use crate::{cli_tools, contracts, routing};
use rmcp::model::Tool;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub type AgentJudge = dyn Fn(&Value) -> Result<Value, crate::runtime::RuntimeError> + Send + Sync;

pub const INSTRUCTIONS: &str = "Before assigning a newly ready task, call switchbite_route with the task and the host's factual worker/model pool. Each harness/model/settings combination needs its own ID. Default policy is sufficient to start; specify a profile or continuity only when relevant. Read status, next_action and summary first. Only recheck_and_dispatch permits the host to recheck authorization/capacity and reserve a target; it is still a proposal. Honor every hold. Never resubmit or migrate running work. Do not invent availability, capabilities, costs or completion receipts. Use switchbite_validate_plan for an existing DAG and switchbite_feedback for observed outcomes. Switchbite does not execute workers.";

pub const GUIDE: &str = include_str!("../docs/coordinator.md");

pub fn tools() -> Vec<Tool> {
    let mut tools = vec![Tool::new(
        "switchbite_route",
        "Choose a target for one newly ready task. Required: task {id, objective} and pool with factual id, description, capabilities, available for each target. Optional policy/profile and continuity. Returns status, next_action, summary and an exact candidate ID or hold. Never executes work. Start with defaults; no hashes, replay or model-provider details are needed.",
        routing::agent_input_schema().as_object().expect("route schema").clone(),
    ).with_raw_output_schema(Arc::new(routing::output_schema().as_object().expect("route output").clone()))
        .annotate(cli_tools::annotations(true))];
    // Keep the two familiar offline operations and status; hide legacy replay
    // schemas from the default agent surface.
    tools.extend(cli_tools::tools().into_iter().filter(|t| t.name != "switchbite_decide"));
    tools
}

pub fn setup(client: &str, config: &str) -> Value {
    json!({"client":client,"profile":"agent","mcp_config":config,
        "coordinator_instructions":INSTRUCTIONS,"writes_configuration":false,
        "next_steps":["Merge mcp_config into the client's effective MCP configuration.",
        "Restart the client and approve the Switchbite server if prompted.",
        "Call switchbite_status, then switchbite_route for each newly ready assignment."]})
}

pub fn validate_arguments(args: &Value) -> Result<(), contracts::ContractError> {
    contracts::bounded(args, contracts::MAX_JSON_BYTES)?;
    contracts::validate(args, &routing::agent_input_schema())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis().min(i64::MAX as u128) as u64).unwrap_or(0)
}

/// Retain sanitized transport diagnostics per call; never use a global last error.
pub fn route(request: &Value, judge: Option<Arc<AgentJudge>>, now: u64)
    -> Result<Value, crate::runtime::RuntimeError>
{
    let failure = Arc::new(Mutex::new(None));
    let diagnostic = failure.clone();
    let configured = judge.is_some();
    let adapter = move |body: &Value| {
        let Some(judge) = &judge else { return Err(contracts::ContractError::JudgeFailed); };
        judge(body).map_err(|error| {
            *diagnostic.lock().expect("request diagnostic") = Some(error);
            contracts::ContractError::JudgeFailed
        })
    };
    let result = routing::route(request, if configured {Some(&adapter)} else {None}, now);
    if let Some(error) = failure.lock().expect("request diagnostic").take() {
        return Err(error);
    }
    result.map_err(|error| crate::runtime::RuntimeError::from_code(error.code()))
}
