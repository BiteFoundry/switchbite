//! Agent-facing schemas: the domain stays API 1; no host execution authority.
use crate::{
    Plugin,
    contracts::{self, ContractError, MAX_JSON_BYTES, MODEL},
};
use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{Value, json};
use std::sync::Arc;

pub const OPERATIONS: [(&str, &str, &str); 3] = [
    (
        "switchbite_decide",
        "decide",
        "Propose a worker assignment among caller-supplied candidates. Supply factual capabilities and availability; do not invent them. May call Jev using the configured API key. Does not spawn or run any worker. Omit version to use docking API 1.",
    ),
    (
        "switchbite_validate_plan",
        "validate_plan",
        "Validate the supplied task DAG, dependencies, acceptance criteria and outputs. Returns a plan proposal; does not generate tasks or execute a workflow. Offline; version defaults to 1.",
    ),
    (
        "switchbite_feedback",
        "feedback",
        "Normalize an observed task outcome and evidence into a feedback proposal. Does not persist feedback or claim completion on behalf of the host. Offline; version defaults to 1.",
    ),
];

fn optional(schema: &mut Value, name: &str, default: Value) {
    if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|v| v != name);
    }
    if let Some(property) = schema.get_mut("properties").and_then(|v| v.get_mut(name)) {
        property["default"] = default;
    }
}
pub fn input_schema(operation: &str) -> Result<Value, ContractError> {
    let mut schema = contracts::operation_schema(operation, "input")?.clone();
    optional(&mut schema, "version", json!(1));
    if operation == "decide" {
        optional(&mut schema["properties"]["task"], "extensions", json!({}));
        optional(
            &mut schema["properties"]["candidates"]["items"],
            "extensions",
            json!({}),
        );
    }
    Ok(schema)
}
pub fn normalize(operation: &str, arguments: &Value) -> Result<Value, ContractError> {
    contracts::bounded(arguments, MAX_JSON_BYTES)?;
    contracts::validate(arguments, &input_schema(operation)?)?;
    let mut request = arguments.clone();
    request
        .as_object_mut()
        .ok_or(ContractError::InvalidRequest)?
        .entry("version")
        .or_insert(json!(1));
    if operation == "decide" {
        request["task"]
            .as_object_mut()
            .ok_or(ContractError::InvalidRequest)?
            .entry("extensions")
            .or_insert(json!({}));
        for candidate in request["candidates"]
            .as_array_mut()
            .ok_or(ContractError::InvalidRequest)?
        {
            candidate
                .as_object_mut()
                .ok_or(ContractError::InvalidRequest)?
                .entry("extensions")
                .or_insert(json!({}));
        }
    }
    contracts::validate_request(operation, &request)?;
    Ok(request)
}
pub fn status(plugin: &Plugin) -> Value {
    json!({"plugin_id":"switchbite", "plugin_version":env!("CARGO_PKG_VERSION"), "docking_api":1,
        "model":MODEL, "judge_configured":plugin.judge_configured(), "proposal_only":true,
        "notice":"Configuration presence is not authentication, quota or task-success proof."})
}
pub fn status_schema() -> Value {
    json!({"type":"object","properties":{"plugin_id":{"type":"string"},"plugin_version":{"type":"string"},
        "docking_api":{"type":"integer"},"model":{"type":"string"},"judge_configured":{"type":"boolean"},
        "proposal_only":{"type":"boolean"},"notice":{"type":"string"}},
        "required":["plugin_id","plugin_version","docking_api","model","judge_configured","proposal_only","notice"],"additionalProperties":false})
}
pub fn annotations(open_world: bool) -> ToolAnnotations {
    let mut hints = ToolAnnotations::new();
    hints.read_only_hint = Some(true);
    hints.destructive_hint = Some(false);
    hints.open_world_hint = Some(open_world);
    hints
}
pub fn tools() -> Vec<Tool> {
    let mut tools: Vec<Tool> = OPERATIONS
        .iter()
        .map(|&(name, operation, description)| {
            Tool::new(
                name,
                description,
                input_schema(operation)
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .with_raw_output_schema(Arc::new(
                contracts::operation_schema(operation, "output")
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .clone(),
            ))
            .annotate(annotations(operation == "decide"))
        })
        .collect();
    tools.push(Tool::new("switchbite_status", "Check plugin version, configured model and whether a judge is configured. Does not call an API, expose credentials or modify state.",
        json!({"type":"object","properties":{},"additionalProperties":false}).as_object().unwrap().clone())
        .with_raw_output_schema(Arc::new(status_schema().as_object().unwrap().clone())).annotate(annotations(false)));
    tools
}
