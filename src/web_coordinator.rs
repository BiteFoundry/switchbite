//! Bounded native coordinator for web-submitted workflows.
use crate::{adapters::LocalHost, workflow::Workflow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

const MAX_ARTIFACT: u64 = 128 * 1024;
const OBJECTIVE: &str = "Plan or repair the authorized workflow. Write only switchbite-observation.json in this worktree; do not implement the project. Follow the instructions and context in this task.";
const INSTRUCTIONS: &str = "Return JSON only in switchbite-observation.json. For plan use {\"plan\":{\"nodes\":[...]}}; for repair use {\"task\":{...}} or {\"plan\":{\"nodes\":[...]}}. Create a bounded, actionable DAG, usually at most 8 nodes. Each node has id, objective, role (exploration, implementation, mockup, integration, or review), depends_on, acceptance, outputs, and write_paths. Use relative project paths; coordinate dependencies and avoid conflicting write paths. Preserve the user's complete goal. Include an independent review node after implementation that writes a useful report. Honor model settings explicitly requested in the user goal; otherwise omit routing/model pins so Jev chooses workers. For repair preserve existing task IDs and completed or active work. You may add smaller subtasks, replace the targeted task and other pending or blocked tasks, and update their dependencies. If restating a completed or active node, copy it unchanged. Use the supplied state, evidence and integrated workspace references. Do not implement, modify project files, or launch other agents.";
const REVIEW_OBJECTIVE: &str = "Review the proposed worker assignment. Write only switchbite-observation.json; do not implement the project or change the plan.";
const REVIEW_INSTRUCTIONS: &str = "Choose one eligible existing candidate ID from action.payload.candidate_snapshot. Use the supplied route input, task, dependencies and route result or scores when available. Honor explicit user model pins and review independence; follow the route input policy and exclusions. Return JSON like {\"status\":\"ready\",\"candidate_id\":\"existing-id\",\"reason_codes\":[\"coordinator_review\"],\"summary\":\"Reason for choice\"}. Do not invent candidates, replan, implement, modify project files, or launch other agents.";
const NODE_FORMAT: &str = r#"id, objective and role are strings. depends_on, requirements, acceptance, outputs, write_paths and context_refs are arrays of strings, even for one value. Example node: {"id":"build","objective":"Implement the requested change","role":"implementation","depends_on":[],"acceptance":["The requested behavior works"],"outputs":["src/feature.rs"],"write_paths":["src/feature.rs"]}."#;

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
    let meta = fs::symlink_metadata(&path).map_err(|_| error("coordinator_artifact_missing"))?;
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
    let mut result: Value =
        serde_json::from_slice(&bytes).map_err(|_| error("coordinator_artifact_invalid"))?;
    if kind == "review_route" {
        let candidate_id = result["candidate_id"].as_str().filter(|id| !id.is_empty())
            .ok_or_else(|| error("coordinator_review_invalid"))?;
        let candidates = action_value["payload"]["candidate_snapshot"].as_array()
            .ok_or_else(|| error("coordinator_review_snapshot_invalid"))?;
        if !candidates.iter().any(|candidate| candidate["id"] == candidate_id && candidate["available"] == true) {
            return Err(error("coordinator_review_candidate_invalid"));
        }
        if result["status"] != "ready" || !result["reason_codes"].as_array().is_some_and(|codes| codes.iter().any(|code| code == "coordinator_review"))
            || !result["summary"].as_str().is_some_and(|s| !s.trim().is_empty() && s.len() <= 2000)
        {
            return Err(error("coordinator_review_invalid"));
        }
        return Ok(json!({"status":"ready","candidate_id":candidate_id,"reason_codes":["coordinator_review"],"summary":result["summary"]}));
    }
    let valid_plan = result["plan"]["nodes"]
        .as_array()
        .is_some_and(|n| !n.is_empty() && n.len() <= 128);
    if !(valid_plan || (kind == "repair" && result["task"].is_object())) {
        return Err(error("coordinator_artifact_invalid"));
    }
    // A single prose acceptance condition has exactly the same meaning as one
    // list item. Canonicalize it here; the workflow still validates the full DAG.
    if let Some(nodes) = result["plan"]["nodes"].as_array_mut() {
        for node in nodes {
            normalize_acceptance(node);
        }
    }
    if let Some(task) = result.get_mut("task") {
        normalize_acceptance(task);
    }
    Ok(result)
}

fn normalize_acceptance(node: &mut Value) {
    if let Some(condition) = node["acceptance"].as_str() {
        node["acceptance"] = json!([condition]);
    }
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
    let task = json!({"id":key(id,attempt,"task"),"objective":objective,"instructions":instructions,"goal":request["goal"],"action":action_value,"workflow_state":state});
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
            json!({"task":task,"prepared":prepared,"candidate":candidate}),
        ))
        .map_err(|e| e.to_value())?;
    if result["status"] != "completed" || result["completion_verified"] != true {
        return Err(json!({"code":"coordinator_execution_failed","result":result}));
    }
    let workspace = prepared["workspace"]
        .as_str()
        .ok_or_else(|| error("invalid_workspace"))?;
    observation(Path::new(workspace), kind, action_value)
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
    fn route_review_rejects_invented_or_unavailable_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("switchbite-observation.json");
        let action = json!({"payload":{"candidate_snapshot":[{"id":"valid","available":true},{"id":"offline","available":false}]}});
        for id in ["invented", "offline"] {
            fs::write(&path, serde_json::to_vec(&json!({"status":"ready","candidate_id":id,"reason_codes":["coordinator_review"],"summary":"choice"})).unwrap()).unwrap();
            assert_eq!(observation(dir.path(), "review_route", &action).unwrap_err()["code"], "coordinator_review_candidate_invalid");
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
}
