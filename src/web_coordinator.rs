//! Bounded native coordinator for web-submitted workflows.
use crate::{adapters::LocalHost, workflow::Workflow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

const MAX_ARTIFACT: u64 = 128 * 1024;
const OBJECTIVE: &str = "Plan or repair the authorized workflow. Write only switchbite-observation.json in this worktree; do not implement the project. Follow the instructions and context in this task.";
const INSTRUCTIONS: &str = "Return JSON only in switchbite-observation.json. For plan use {\"plan\":{\"nodes\":[...]}}; for repair use {\"task\":{...}} or {\"plan\":{\"nodes\":[...]}}. Create a bounded, actionable DAG, usually at most 8 nodes. Each node has id, objective, role (exploration, implementation, mockup, integration, or review), depends_on, acceptance, outputs, and write_paths. Use relative project paths; coordinate dependencies and avoid conflicting write paths. Preserve the user's complete goal. Include an independent review node after implementation that writes a useful report. Honor model settings explicitly requested in the user goal; otherwise omit routing/model pins so Jev chooses workers. For repair preserve existing task IDs; only replace the targeted task and, if needed, other pending or blocked tasks. Use the supplied state, evidence and integrated workspace references. Do not implement, modify project files, or launch other agents.";

fn error(code: &str) -> Value {
    json!({"code":code})
}
fn key(action_id: &str, attempt: u64, suffix: &str) -> String {
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
fn observation(dir: &Path, kind: &str) -> Result<Value, Value> {
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
    let result: Value =
        serde_json::from_slice(&bytes).map_err(|_| error("coordinator_artifact_invalid"))?;
    let valid_plan = result["plan"]["nodes"]
        .as_array()
        .is_some_and(|n| !n.is_empty() && n.len() <= 128);
    if !(valid_plan || (kind == "repair" && result["task"].is_object())) {
        return Err(error("coordinator_artifact_invalid"));
    }
    Ok(result)
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
        .filter(|s| *s == "plan" || *s == "repair")
        .ok_or_else(|| error("invalid_action"))?;
    let attempt = action_value["coordinator_attempt"].as_u64().unwrap_or(0);
    let (host, request) = host(root, run)?;
    let state = Workflow::new(root.to_path_buf())
        .status(run)
        .map_err(|e| e.to_value())?;
    let task = json!({"id":key(id,attempt,"task"),"objective":OBJECTIVE,"instructions":INSTRUCTIONS,"goal":request["goal"],"action":action_value,"workflow_state":state});
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
    observation(Path::new(workspace), kind)
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
