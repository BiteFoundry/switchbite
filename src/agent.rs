//! Small, opinionated entry point for coordinator agents. No host execution.
use crate::{cli_tools, contracts, routing};
use rmcp::model::Tool;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub type AgentJudge = dyn Fn(&Value) -> Result<Value, crate::runtime::RuntimeError> + Send + Sync;

/// Explicit provider identity alongside the legacy diagnostic-aware callback.
pub struct ConfiguredJudge {
    model: String,
    judge: Arc<AgentJudge>,
}
impl ConfiguredJudge {
    pub fn new(
        model: impl Into<String>,
        judge: Arc<AgentJudge>,
    ) -> Result<Self, contracts::ContractError> {
        let model = model.into();
        contracts::validate_model(&model)?;
        Ok(Self { model, judge })
    }
    pub fn legacy(judge: Arc<AgentJudge>) -> Self {
        Self {
            model: contracts::MODEL.into(),
            judge,
        }
    }
    pub fn model(&self) -> &str {
        &self.model
    }
}

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
    tools.extend(
        cli_tools::tools()
            .into_iter()
            .filter(|t| t.name != "switchbite_decide" && t.name != "switchbite_status"),
    );
    tools.push(Tool::new("switchbite_status", "Check setup and the next step. No provider call, credentials, quota claims or host mutation.",
        json!({"type":"object","properties":{},"additionalProperties":false}).as_object().unwrap().clone())
        .with_raw_output_schema(Arc::new(status_schema().as_object().unwrap().clone()))
        .annotate(cli_tools::annotations(false)));
    tools
}

pub fn status(judge_configured: bool) -> Value {
    let mut status = cli_tools::status(&crate::Plugin::default());
    status["judge_configured"] = json!(judge_configured);
    status["profile"] = json!("agent");
    status["routing_api"] = json!(2);
    status["primary_tool"] = json!("switchbite_route");
    status["next_action"] = json!(if judge_configured {
        "route_new_task"
    } else {
        "configure_judge"
    });
    status
}

pub fn configured_status(judge: Option<&ConfiguredJudge>) -> Value {
    let mut result = status(judge.is_some());
    if let Some(judge) = judge {
        result["model"] = json!(judge.model());
    }
    result
}

pub fn status_schema() -> Value {
    let mut schema = cli_tools::status_schema();
    for (key, definition) in [
        ("profile", json!({"type":"string","const":"agent"})),
        ("routing_api", json!({"type":"integer","const":2})),
        (
            "primary_tool",
            json!({"type":"string","const":"switchbite_route"}),
        ),
        (
            "next_action",
            json!({"type":"string","enum":["route_new_task","configure_judge"]}),
        ),
    ] {
        schema["properties"][key] = definition;
        schema["required"].as_array_mut().unwrap().push(json!(key));
    }
    schema
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis().min(i64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// Retain sanitized transport diagnostics per call; never use a global last error.
pub fn route(
    request: &Value,
    judge: Option<Arc<AgentJudge>>,
    now: u64,
) -> Result<Value, crate::runtime::RuntimeError> {
    route_with_judge(
        request,
        judge.map(|j| Arc::new(ConfiguredJudge::legacy(j))),
        now,
    )
}

struct DiagnosticJudge {
    configured: Arc<ConfiguredJudge>,
    failure: Arc<Mutex<Option<crate::runtime::RuntimeError>>>,
}
impl crate::Judge for DiagnosticJudge {
    fn model(&self) -> &str {
        self.configured.model()
    }
    fn judge(&self, body: &Value) -> Result<Value, contracts::ContractError> {
        (self.configured.judge)(body).map_err(|error| {
            *self.failure.lock().expect("request diagnostic") = Some(error);
            contracts::ContractError::JudgeFailed
        })
    }
}

pub fn route_with_judge(
    request: &Value,
    judge: Option<Arc<ConfiguredJudge>>,
    now: u64,
) -> Result<Value, crate::runtime::RuntimeError> {
    let failure = Arc::new(Mutex::new(None));
    let adapter = judge.map(|configured| DiagnosticJudge {
        configured,
        failure: failure.clone(),
    });
    let result = routing::route(
        request,
        adapter.as_ref().map(|j| j as &dyn crate::Judge),
        now,
    );
    if let Some(error) = failure.lock().expect("request diagnostic").take() {
        return Err(error);
    }
    result.map_err(|error| crate::runtime::RuntimeError::from_code(error.code()))
}
