//! Bounded native coordinator for web-submitted workflows.
use crate::{adapters::LocalHost, workflow::Workflow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

const MAX_ARTIFACT: u64 = 128 * 1024;
const OBJECTIVE: &str = "Plan or repair the authorized workflow. Return the structured JSON result and write the same JSON to switchbite-observation.json in this worktree for compatibility; do not implement the project. Follow the instructions and context in this task.";
const INSTRUCTIONS: &str = "Return JSON as the final response and write it to switchbite-observation.json. For plan use {\"plan\":{\"nodes\":[...]}}; for repair use {\"task\":{...}} or {\"plan\":{\"nodes\":[...]}}. Create a bounded, actionable DAG, usually at most 8 nodes. Each node has id, objective, role (exploration, implementation, mockup, integration, or review), depends_on, acceptance, outputs, and write_paths. Use relative project paths; coordinate dependencies and avoid conflicting write paths. Preserve the user's complete goal. Include an independent review node after implementation that writes a useful report. Honor model settings explicitly requested in the user goal; otherwise omit routing/model pins so Jev chooses workers. For repair preserve existing task IDs and completed or active work. You may add smaller subtasks, replace the targeted task and other pending or blocked tasks, and update their dependencies. If restating a completed or active node, copy it unchanged. Use the supplied state, evidence and integrated workspace references. Do not implement, modify project files, or launch other agents.";
const REVIEW_OBJECTIVE: &str = "Review the proposed worker assignment. Return structured JSON and write the same JSON to switchbite-observation.json; do not implement the project or change the plan.";
const REVIEW_INSTRUCTIONS: &str = "Choose one eligible existing candidate ID from action.payload.candidate_snapshot. Use the supplied route input, task, dependencies and route result or scores when available. Resolve the uncertain router scores with your assessment of task fit and candidate capabilities. Treat explicit model pins, required capabilities, selected providers, current availability and reviewer exclusions as binding constraints. Return JSON like {\"status\":\"ready\",\"candidate_id\":\"existing-id\",\"reason_codes\":[\"coordinator_review\"],\"summary\":\"Reason for choice\"}. Do not invent candidates, replan, implement, modify project files, or launch other agents.";
const NODE_FORMAT: &str = r#"Use the supplied result schema; use null for optional fields without a value. If coordinator_feedback includes prior_result, correct that existing plan using its diagnostic, preserving its work and constraints. requirements contains only exact worker capability IDs, such as file.read or code.change; put all natural-language task instructions in objective. Model pins belong inside routing, e.g. {"routing":{"provider":"codex","model":"gpt-6.1-terra","effort":"high"}}."#;

/// Structured result contract for coordinator CLI output.
pub fn result_schema(kind: &str) -> Value {
    let plan = crate::workflow_interface::plan_schema();
    let node = plan["properties"]["nodes"]["items"].clone();
    let mut schema = match kind {
        "repair" => {
            json!({"type":"object","additionalProperties":false,"properties":{"plan":plan,"task":node}})
        }
        "review_route" => {
            json!({"type":"object","additionalProperties":false,"required":["status","candidate_id","reason_codes","summary"],"properties":{"status":{"type":"string","enum":["ready"]},"candidate_id":{"type":"string"},"reason_codes":{"type":"array","items":{"type":"string"}},"summary":{"type":"string"}}})
        }
        _ => {
            json!({"type":"object","additionalProperties":false,"required":["plan"],"properties":{"plan":plan}})
        }
    };
    strict_schema(&mut schema);
    schema
}

// Codex structured outputs require every property to be required. Preserve the
// workflow's optional fields as nullable values at the CLI boundary.
fn strict_schema(schema: &mut Value) {
    if let Some(items) = schema.get_mut("items") {
        strict_schema(items);
    }
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for (key, property) in properties.iter_mut() {
            strict_schema(property);
            if !required.contains(key) {
                *property = json!({"anyOf":[property.clone(), {"type":"null"}]});
            }
        }
        let all: Vec<String> = properties.keys().cloned().collect();
        schema["required"] = json!(all);
    }
}

fn error(code: &str) -> Value {
    json!({"code":code})
}
pub(crate) fn key(action_id: &str, attempt: u64, suffix: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{action_id}:{attempt}").as_bytes())
    );
    format!("web-coordinator-{}-{suffix}", &digest[..24])
}
fn action(id: String, kind: &str, run: &str, payload: Value) -> Value {
    // Coordinator scratch branches must not reserve the workflow's integration branch.
    json!({"id":id,"kind":kind,"run_id":format!("{run}-coordinator"),"payload":payload})
}
fn host(root: &Path, run: &str) -> Result<(LocalHost, Value), Value> {
    let request = Workflow::new(root.to_path_buf())
        .request(run)
        .map_err(|e| e.to_value())?;
    let workspace = request["workspace"]
        .as_str()
        .ok_or_else(|| error("invalid_workspace"))?;
    let host = LocalHost::new(
        Path::new(workspace),
        &root.join(run).join("coordinator"),
        &request.get("host").cloned().unwrap_or(json!({})),
    )
    .map_err(|e| e.to_value())?;
    Ok((host, request))
}
fn observation(dir: &Path, kind: &str, action_value: &Value) -> Result<Value, Value> {
    let root = fs::canonicalize(dir).map_err(|_| error("coordinator_artifact_invalid"))?;
    let path = root.join("switchbite-observation.json");
    let meta = fs::symlink_metadata(&path)
        .map_err(|_| json!({"code":"coordinator_artifact_missing","stage":"result_collection"}))?;
    if !meta.file_type().is_file()
        || meta.len() > MAX_ARTIFACT
        || fs::canonicalize(&path)
            .map_err(|_| error("coordinator_artifact_invalid"))?
            .parent()
            != Some(root.as_path())
    {
        return Err(error("coordinator_artifact_invalid"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| error("coordinator_artifact_invalid"))?
        .take(MAX_ARTIFACT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("coordinator_artifact_invalid"))?;
    if bytes.len() as u64 > MAX_ARTIFACT {
        return Err(error("coordinator_artifact_invalid"));
    }
    let json_bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    let result: Value =
        serde_json::from_slice(json_bytes).map_err(|_| error("coordinator_artifact_invalid"))?;
    normalize_observation(kind, result, action_value)
}

/// Normalize and preflight a coordinator result without observing or executing it.
pub fn normalize_observation(
    kind: &str,
    result: Value,
    action_value: &Value,
) -> Result<Value, Value> {
    let prior_result = result.clone();
    normalize_observation_inner(kind, result, action_value).map_err(|mut diagnostic| {
        if kind == "plan"
            && let Some(obj) = diagnostic.as_object_mut()
        {
            obj.insert("code".into(), json!("coordinator_plan_invalid"));
            obj.insert("stage".into(), json!("plan_validation"));
            obj.insert("prior_result".into(), prior_result);
        }
        diagnostic
    })
}

fn normalize_observation_inner(
    kind: &str,
    mut result: Value,
    action_value: &Value,
) -> Result<Value, Value> {
    if kind == "review_route" {
        let candidate_id = result["candidate_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| error("coordinator_review_invalid"))?;
        let candidates = action_value["payload"]["candidate_snapshot"]
            .as_array()
            .ok_or_else(|| error("coordinator_review_snapshot_invalid"))?;
        if !candidates
            .iter()
            .any(|candidate| candidate["id"] == candidate_id && candidate["available"] == true)
        {
            return Err(error("coordinator_review_candidate_invalid"));
        }
        if result["status"] != "ready"
            || !result["reason_codes"]
                .as_array()
                .is_some_and(|codes| codes.iter().any(|code| code == "coordinator_review"))
            || !result["summary"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 2000)
        {
            return Err(error("coordinator_review_invalid"));
        }
        return Ok(
            json!({"status":"ready","candidate_id":candidate_id,"reason_codes":["coordinator_review"],"summary":result["summary"]}),
        );
    }
    if let Some(obj) = result.as_object_mut() {
        for key in ["plan", "task"] {
            if obj.get(key).is_some_and(Value::is_null) {
                obj.remove(key);
            }
        }
    }
    let valid_plan = result["plan"]["nodes"]
        .as_array()
        .is_some_and(|n| !n.is_empty() && n.len() <= 128);
    if !(valid_plan || (kind == "repair" && result["task"].is_object())) {
        return Err(
            json!({"code":"coordinator_artifact_invalid","node_id":null,"field":"plan.nodes","expected":"array of 1 to 128 nodes, or a task for repair"}),
        );
    }
    // A single prose acceptance condition has exactly the same meaning as one
    // list item. Canonicalize it here; the workflow still validates the full DAG.
    if let Some(nodes) = result
        .get_mut("plan")
        .and_then(|plan| plan.get_mut("nodes"))
        .and_then(Value::as_array_mut)
    {
        for node in nodes {
            normalize_acceptance(node);
            normalize_node(node, action_value)?;
        }
    }
    if let Some(task) = result.get_mut("task") {
        normalize_acceptance(task);
        normalize_node(task, action_value)?;
    }
    if kind == "plan" {
        crate::workflow::preflight_plan(&result["plan"])?;
    }
    Ok(result)
}

fn normalize_acceptance(node: &mut Value) {
    if let Some(condition) = node["acceptance"].as_str() {
        node["acceptance"] = json!([condition]);
    }
}

fn normalize_node(node: &mut Value, action_value: &Value) -> Result<(), Value> {
    let node_id = node.get("id").cloned().unwrap_or(Value::Null);
    let conflict = |field: &str| json!({"code":"coordinator_artifact_invalid","node_id":node_id,"field":field,"expected":"matching values for duplicate routing pins"});
    let Some(obj) = node.as_object_mut() else {
        return Ok(());
    };
    for key in [
        "depends_on",
        "requirements",
        "acceptance",
        "outputs",
        "write_paths",
        "role",
        "context_refs",
        "routing",
    ] {
        if obj.get(key).is_some_and(Value::is_null) {
            obj.remove(key);
        }
    }
    if let Some(routing) = obj.get_mut("routing").and_then(Value::as_object_mut) {
        routing.retain(|_, value| !value.is_null());
        if routing.is_empty() {
            obj.remove("routing");
        }
    }
    let mut flat = Vec::new();
    for key in [
        "candidate_id",
        "provider",
        "model",
        "effort",
        "reasoning_effort",
    ] {
        if let Some(value) = obj.remove(key) {
            flat.push((
                if key == "reasoning_effort" {
                    "effort"
                } else {
                    key
                },
                value,
            ));
        }
    }
    if let Some(routing) = obj.get_mut("routing") {
        if let Some(routing) = routing.as_object_mut() {
            if let Some(alias) = routing.remove("reasoning_effort") {
                if let Some(existing) = routing.get("effort") {
                    if existing != &alias {
                        return Err(conflict("routing.effort"));
                    }
                } else {
                    routing.insert("effort".into(), alias);
                }
            }
        } else if !flat.is_empty() {
            return Err(conflict("routing"));
        }
    }
    if !flat.is_empty() {
        let routing = obj
            .entry("routing")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| conflict("routing"))?;
        for (key, value) in flat {
            if let Some(existing) = routing.get(key) {
                if existing != &value {
                    return Err(conflict(&format!("routing.{key}")));
                }
            } else {
                routing.insert(key.to_owned(), value);
            }
        }
    }
    // Preserve exact advertised capabilities even when their IDs contain spaces.
    let known_capabilities = action_value
        .pointer("/payload/known_capabilities")
        .and_then(Value::as_array);
    // Sentence-shaped entries are legacy instructions; ambiguous IDs stay binding.
    if let Some(requirements) = obj.get_mut("requirements").and_then(Value::as_array_mut) {
        let mut prose = Vec::new();
        requirements.retain(|value| {
            if let Some(text) = value.as_str()
                && !known_capabilities.is_some_and(|ids| ids.iter().any(|id| id == text))
                && text.split_whitespace().count() >= 3
                && matches!(text.trim_end().chars().last(), Some('.' | '!' | '?' | '。'))
            {
                prose.push(text.to_owned());
                return false;
            }
            true
        });
        if !prose.is_empty() {
            if let Some(objective) = obj
                .get("objective")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                obj.insert(
                    "objective".into(),
                    Value::String(format!(
                        "{objective}\n\nAdditional task instructions:\n{}",
                        prose.join("\n")
                    )),
                );
            } else {
                return Err(
                    json!({"code":"invalid_plan","node_id":node_id,"field":"objective","expected":"nonempty string to preserve task instructions"}),
                );
            }
        }
    }
    Ok(())
}

/// Execute one coordinator action; the caller owns the workflow claim and observation.
pub fn fulfill(
    root: &Path,
    run: &str,
    action_value: &Value,
    candidate: &Value,
) -> Result<Value, Value> {
    let id = action_value["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("invalid_action"))?;
    let kind = action_value["kind"]
        .as_str()
        .filter(|s| ["plan", "repair", "review_route"].contains(s))
        .ok_or_else(|| error("invalid_action"))?;
    let attempt = action_value["coordinator_attempt"].as_u64().unwrap_or(0);
    let (host, request) = host(root, run)?;
    let state = Workflow::new(root.to_path_buf())
        .status(run)
        .map_err(|e| e.to_value())?;
    if kind == "review_route" && !action_value["payload"]["candidate_snapshot"].is_array() {
        return Err(error("coordinator_review_snapshot_invalid"));
    }
    let (objective, instructions) = if kind == "review_route" {
        (REVIEW_OBJECTIVE, REVIEW_INSTRUCTIONS.to_owned())
    } else {
        (OBJECTIVE, format!("{INSTRUCTIONS} {NODE_FORMAT}"))
    };
    let task = json!({"id":key(id,attempt,"task"),"objective":objective,"instructions":instructions,"goal":request["goal"],"action":action_value,"workflow_state":state,"result_schema":result_schema(kind)});
    // Leave room for the host's framing and JSON escaping.
    if serde_json::to_vec(&task)
        .map_err(|_| error("invalid_action"))?
        .len()
        > 120 * 1024
    {
        return Err(error("coordinator_context_too_large"));
    }
    let prepared = host
        .perform(&action(
            key(id, attempt, "prepare"),
            "prepare",
            run,
            json!({"task":task}),
        ))
        .map_err(|e| e.to_value())?;
    let result = host
        .perform(&action(
            key(id, attempt, "launch"),
            "launch",
            run,
            json!({"task":task,"prepared":prepared,"candidate":candidate,"result_schema":result_schema(kind)}),
        ))
        .map_err(|e| e.to_value())?;
    if result["status"] != "completed" || result["completion_verified"] != true {
        return Err(json!({"code":"coordinator_execution_failed","result":result}));
    }
    let workspace = prepared["workspace"]
        .as_str()
        .ok_or_else(|| error("invalid_workspace"))?;
    let mut validation_action = action_value.clone();
    let capabilities = Workflow::new(root.to_path_buf())
        .known_capabilities(run)
        .map_err(|e| e.to_value())?;
    if let Some(payload) = validation_action
        .get_mut("payload")
        .and_then(Value::as_object_mut)
    {
        payload.insert("known_capabilities".into(), json!(capabilities));
    }
    collect_result(Path::new(workspace), kind, &validation_action, &result)
}

fn collect_result(dir: &Path, kind: &str, action: &Value, result: &Value) -> Result<Value, Value> {
    if let Some(final_result) = result.get("final_result").filter(|v| !v.is_null()) {
        return normalize_observation(kind, final_result.clone(), action);
    }
    let fallback = observation(dir, kind, action);
    if let Err(ref detail) = fallback
        && [
            "coordinator_artifact_missing",
            "coordinator_artifact_invalid",
        ]
        .contains(&detail["code"].as_str().unwrap_or(""))
        && let Some(code) = result.get("final_result_error").and_then(Value::as_str)
    {
        return Err(json!({"code":code,"stage":"result_collection"}));
    }
    fallback
}

/// Signal cancellation through LocalHost's owned execution record, including before launch starts.
pub fn cancel(root: &Path, run: &str, action_value: &Value) -> Result<(), Value> {
    let action_id = action_value["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error("invalid_action"))?;
    let attempt = action_value["coordinator_attempt"].as_u64().unwrap_or(0);
    let (host, _) = host(root, run)?;
    host.perform(&action(
        key(action_id, attempt, "cancel"),
        "inspect",
        run,
        json!({"execution_id":key(action_id,attempt,"launch"),"operation":"cancel"}),
    ))
    .map_err(|e| e.to_value())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_result_errors_use_valid_compatibility_file_or_report_collection_failure() {
        let dir = tempfile::tempdir().unwrap();
        let action = Value::Null;
        let launch = json!({"final_result_error":"coordinator_final_result_too_large"});
        let error = collect_result(dir.path(), "plan", &action, &launch).unwrap_err();
        assert_eq!(error["code"], "coordinator_final_result_too_large");
        assert_eq!(error["stage"], "result_collection");
        fs::write(dir.path().join("switchbite-observation.json"), b"not json").unwrap();
        assert_eq!(
            collect_result(dir.path(), "plan", &action, &launch).unwrap_err()["code"],
            "coordinator_final_result_too_large"
        );
        fs::write(
            dir.path().join("switchbite-observation.json"),
            br#"{"plan":{"nodes":[{"id":"one","objective":"Build"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            collect_result(dir.path(), "plan", &action, &launch).unwrap()["plan"]["nodes"][0]["id"],
            "one"
        );
        fs::write(
            dir.path().join("switchbite-observation.json"),
            br#"{"plan":{"nodes":[{"id":"one","objective":"Build","depends_on":["missing"]}]}}"#,
        )
        .unwrap();
        let error = collect_result(dir.path(), "plan", &action, &launch).unwrap_err();
        assert_eq!(error["code"], "coordinator_plan_invalid");
        assert_eq!(error["field"], "depends_on");
    }

    #[test]
    fn accepts_utf8_bom_for_plan_repair_and_route_review() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("switchbite-observation.json");
        let action = json!({"payload":{"candidate_snapshot":[{"id":"valid","available":true}]}});
        for (kind, value) in [
            (
                "plan",
                json!({"plan":{"nodes":[{"id":"build","objective":"Build"}]}}),
            ),
            ("repair", json!({"task":{"id":"build","objective":"Fix"}})),
            (
                "review_route",
                json!({"status":"ready","candidate_id":"valid","reason_codes":["coordinator_review"],"summary":"Eligible"}),
            ),
        ] {
            let plain = serde_json::to_vec(&value).unwrap();
            fs::write(&path, &plain).unwrap();
            let expected = observation(dir.path(), kind, &action).unwrap();
            let mut bom = b"\xef\xbb\xbf".to_vec();
            bom.extend_from_slice(&plain);
            fs::write(&path, bom).unwrap();
            assert_eq!(observation(dir.path(), kind, &action).unwrap(), expected);
        }
        for bytes in [
            b"\xef\xbb\xbf{".as_slice(),
            b"\xef\xbb\xbf\xef\xbb\xbf{}".as_slice(),
            b"\xff\xfe{\0}\0".as_slice(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                observation(dir.path(), "plan", &action).unwrap_err()["code"],
                "coordinator_artifact_invalid"
            );
        }
    }

    #[test]
    fn route_review_rejects_invented_or_unavailable_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("switchbite-observation.json");
        let action = json!({"payload":{"candidate_snapshot":[{"id":"valid","available":true},{"id":"offline","available":false}]}});
        for id in ["invented", "offline"] {
            fs::write(&path, serde_json::to_vec(&json!({"status":"ready","candidate_id":id,"reason_codes":["coordinator_review"],"summary":"choice"})).unwrap()).unwrap();
            assert_eq!(
                observation(dir.path(), "review_route", &action).unwrap_err()["code"],
                "coordinator_review_candidate_invalid"
            );
        }
    }

    #[test]
    fn canonicalizes_single_acceptance_for_plan_and_repair_without_reinterpreting_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("switchbite-observation.json");
        for original in [
            json!({"plan":{"nodes":[{"id":"one","objective":"One","acceptance":"Works","depends_on":[]}]}}),
            json!({"task":{"id":"one","objective":"One","acceptance":"Works","depends_on":[]}}),
        ] {
            fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
            let result = observation(dir.path(), "repair", &Value::Null).unwrap();
            let node = if result["task"].is_object() {
                &result["task"]
            } else {
                &result["plan"]["nodes"][0]
            };
            assert_eq!(node["acceptance"], json!(["Works"]));
            assert_eq!(node["depends_on"], json!([]));
        }
        let mut invalid = json!({"acceptance":{"unexpected":"shape"}});
        normalize_acceptance(&mut invalid);
        assert!(invalid["acceptance"].is_object());
    }
    #[test]
    fn routing_effort_alias_is_canonicalized_for_plans_and_repairs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("switchbite-observation.json");
        for (kind, original) in [
            (
                "plan",
                json!({"plan":{"nodes":[{"id":"one","objective":"One","routing":{"provider":"codex","model":"gpt-6.1-terra","reasoning_effort":"high"}}]}}),
            ),
            (
                "repair",
                json!({"task":{"id":"one","routing":{"effort":"high","reasoning_effort":"high"}}}),
            ),
        ] {
            fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
            let result = observation(dir.path(), kind, &Value::Null).unwrap();
            let node = if kind == "plan" {
                &result["plan"]["nodes"][0]
            } else {
                &result["task"]
            };
            assert_eq!(node["routing"]["effort"], "high");
            assert!(node["routing"].get("reasoning_effort").is_none());
        }
        for (kind, original) in [
            (
                "plan",
                json!({"plan":{"nodes":[{"routing":{"effort":"low","reasoning_effort":"high"}}]}}),
            ),
            (
                "repair",
                json!({"task":{"routing":{"effort":"high","reasoning_effort":"low"}}}),
            ),
        ] {
            fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
            assert_eq!(
                observation(dir.path(), kind, &Value::Null).unwrap_err()["code"],
                if kind == "plan" {
                    "coordinator_plan_invalid"
                } else {
                    "coordinator_artifact_invalid"
                }
            );
        }
    }
    #[test]
    fn contract_schema_and_diagnostics_cover_legacy_plans() {
        let schema = result_schema("plan");
        let properties =
            &schema["properties"]["plan"]["properties"]["nodes"]["items"]["properties"];
        assert!(properties.get("provider").is_none());
        assert!(
            properties["requirements"]["anyOf"][0]["description"]
                .as_str()
                .unwrap()
                .contains("capability IDs")
        );
        let raw = json!({"plan":{"nodes":[{"id":"one","objective":"Build", "provider":"codex", "model":"m", "effort":"high", "requirements":["file.read", "Run the tests."]}]}});
        let result = normalize_observation("plan", raw, &Value::Null).unwrap();
        let node = &result["plan"]["nodes"][0];
        assert_eq!(
            node["routing"],
            json!({"provider":"codex","model":"m","effort":"high"})
        );
        assert_eq!(node["requirements"], json!(["file.read"]));
        assert!(
            node["objective"]
                .as_str()
                .unwrap()
                .contains("Run the tests.")
        );
        let action = json!({"payload":{"known_capabilities":["custom capability with spaces"]}});
        let custom = normalize_observation("plan", json!({"plan":{"nodes":[{"id":"custom","objective":"Build","requirements":["c++", "language/rust", "code review", "custom capability with spaces", "Run all tests."]}]}}), &action).unwrap();
        assert_eq!(
            custom["plan"]["nodes"][0]["requirements"],
            json!([
                "c++",
                "language/rust",
                "code review",
                "custom capability with spaces"
            ])
        );
        assert!(
            custom["plan"]["nodes"][0]["objective"]
                .as_str()
                .unwrap()
                .contains("Run all tests.")
        );
        let bad = json!({"plan":{"nodes":[{"id":"one","objective":"Build","provider":"codex","routing":{"provider":"claude"}}]}});
        let error = normalize_observation("plan", bad.clone(), &Value::Null).unwrap_err();
        assert_eq!(error["node_id"], "one");
        assert_eq!(error["field"], "routing.provider");
        assert_eq!(error["prior_result"], bad);
        let error = normalize_observation(
            "plan",
            json!({"plan":{"nodes":[{"id":"one","objective":"Build","depends_on":["missing"]}]}}),
            &Value::Null,
        )
        .unwrap_err();
        assert_eq!(error["node_id"], "one");
        assert_eq!(error["field"], "depends_on");
        assert!(error["expected"].as_str().unwrap().contains("missing"));
    }

    #[test]
    fn replay_saved_coordinator_plans_when_available() {
        let run = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".switchbite/workflow/run-1793162389339754200-52752");
        if !run.exists() {
            return;
        } // The preserved runtime records are local, not repository fixtures.
        let recorded: Value =
            serde_json::from_slice(&fs::read(run.join("3.json")).unwrap()).unwrap();
        for token in ["1a7cfaeadd46fe684604b5c6", "ed51b6d48098dbe70ec1f1b8"] {
            let execution: Value = serde_json::from_slice(
                &fs::read(
                    run.join("coordinator/executions")
                        .join(format!("{token}.json")),
                )
                .unwrap(),
            )
            .unwrap();
            let bytes = fs::read(
                Path::new(execution["cwd"].as_str().unwrap()).join("switchbite-observation.json"),
            )
            .unwrap();
            let raw: Value =
                serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))
                    .unwrap();
            let result = normalize_observation("plan", raw.clone(), &Value::Null).unwrap();
            for (before, after) in raw["plan"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .zip(result["plan"]["nodes"].as_array().unwrap())
            {
                for key in ["provider", "model", "effort"] {
                    if let Some(value) = before.get(key) {
                        assert_eq!(&after["routing"][key], value);
                    }
                }
                for requirement in before["requirements"].as_array().unwrap() {
                    assert!(
                        after["requirements"]
                            .as_array()
                            .unwrap()
                            .contains(requirement)
                            || after["objective"]
                                .as_str()
                                .unwrap()
                                .contains(requirement.as_str().unwrap())
                    );
                }
                assert!(
                    recorded["catalog"]["candidates"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|candidate| {
                            candidate["available"] == true
                                && ["provider", "model", "effort"].iter().all(|key| {
                                    after["routing"]
                                        .get(*key)
                                        .is_none_or(|pin| candidate[*key] == *pin)
                                })
                                && after["requirements"].as_array().unwrap().iter().all(|r| {
                                    candidate["capabilities"].as_array().unwrap().contains(r)
                                })
                        }),
                    "no candidate for {}",
                    after["id"]
                );
            }
            let dir = tempfile::tempdir().unwrap();
            let workflow = Workflow::new(dir.path().join("state"));
            let state = workflow
                .start(&json!({"workspace":"workspace","goal":"replay","pool":[]}))
                .unwrap();
            let action = state["actions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["kind"] == "plan")
                .unwrap();
            let next = workflow
                .observe(
                    state["run_id"].as_str().unwrap(),
                    &json!({"action_id":action["id"],"event_id":"replay","result":result}),
                )
                .unwrap();
            assert_eq!(next["tasks"].as_array().unwrap().len(), 8);
        }
    }

    #[test]
    fn task_only_repair_is_accepted_by_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let workflow = Workflow::new(dir.path().join("state"));
        let mut state = workflow.start(&json!({"workspace":"workspace","repair_rounds":1,"pool":[{"id":"worker","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"one","objective":"build"}]}})).unwrap();
        let run = state["run_id"].as_str().unwrap().to_owned();
        for (kind, result) in [
            ("route", json!({"status":"ready","candidate_id":"worker"})),
            (
                "prepare",
                json!({"status":"prepared","workspace":"w","branch":"b","integration_workspace":"i","integration_branch":"ib"}),
            ),
            ("launch", json!({"status":"failed"})),
        ] {
            let action = state["actions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["kind"] == kind)
                .unwrap();
            state = workflow.observe(&run, &json!({"action_id":action["id"],"event_id":format!("test:{kind}"),"result":result})).unwrap();
        }
        let artifact =
            json!({"task":{"id":"one","objective":"retry","routing":{"reasoning_effort":"high"}}});
        fs::write(
            dir.path().join("switchbite-observation.json"),
            serde_json::to_vec(&artifact).unwrap(),
        )
        .unwrap();
        let result = observation(dir.path(), "repair", &Value::Null).unwrap();
        assert!(result.get("plan").is_none());
        let action = state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == "repair")
            .unwrap();
        let next = workflow
            .observe(
                &run,
                &json!({"action_id":action["id"],"event_id":"test:repair","result":result}),
            )
            .unwrap();
        assert!(
            next["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["kind"] == "route" && a["task_id"] == "one")
        );
    }
}
