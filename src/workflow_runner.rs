//! Automatic host-action driver shared by CLI and the compact MCP profile.
use crate::{adapters::LocalHost, agent, workflow::Workflow};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fmt,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct RunnerError(pub Value);
impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for RunnerError {}
impl RunnerError {
    pub fn to_value(&self) -> Value {
        self.0.clone()
    }
}

type Job = JoinHandle<Result<Value, Value>>;
static JOBS: OnceLock<Mutex<BTreeMap<String, Job>>> = OnceLock::new();
static ADVANCE: Mutex<()> = Mutex::new(());
fn jobs() -> &'static Mutex<BTreeMap<String, Job>> {
    JOBS.get_or_init(|| Mutex::new(BTreeMap::new()))
}
fn error(code: &str) -> RunnerError {
    RunnerError(json!({"code":code}))
}
fn map_core(e: crate::workflow::WorkflowError) -> RunnerError {
    RunnerError(e.to_value())
}

/// Progress one run without blocking on long worker executions. Expensive host
/// actions run on owned threads; a later call settles their durable receipts.
pub fn advance(
    root: &Path,
    run_id: &str,
    judge: Option<Arc<agent::ConfiguredJudge>>,
) -> Result<Value, RunnerError> {
    let _guard = ADVANCE.lock().map_err(|_| error("driver_busy"))?;
    let engine = Workflow::new(root.to_path_buf());
    let request = engine.request(run_id).map_err(map_core)?;
    let key_prefix = format!("{}:{run_id}:", root.display());
    let finished: Vec<_> = {
        let map = jobs().lock().map_err(|_| error("driver_busy"))?;
        map.iter()
            .filter(|(key, job)| key.starts_with(&key_prefix) && job.is_finished())
            .map(|(key, _)| key.clone())
            .collect()
    };
    for key in finished {
        let job = jobs()
            .lock()
            .map_err(|_| error("driver_busy"))?
            .remove(&key)
            .ok_or_else(|| error("driver_busy"))?;
        let action_id = key
            .strip_prefix(&key_prefix)
            .ok_or_else(|| error("invalid_action"))?;
        let current = engine.status(run_id).map_err(map_core)?;
        let action_kind = current["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["id"] == action_id)
            .and_then(|a| a["kind"].as_str())
            .unwrap_or("launch")
            .to_owned();
        let result = match job.join() {
            Ok(Ok(result)) => result,
            Ok(Err(failure)) => json!({"status":"failed","error":failure}),
            Err(_) if action_kind == "launch" || action_kind == "inspect" => {
                json!({"status":"unknown","execution_id":action_id,"completion_verified":false})
            }
            Err(_) => json!({"status":"failed","error":{"code":"host_panic"}}),
        };
        // Cancellation can supersede an outstanding launch lease. The host's
        // receipt remains persisted and the cancellation inspect reconciles it.
        if let Err(e) = engine.observe(
            run_id,
            &json!({"action_id":action_id,"event_id":format!("host:{action_id}"),"result":result}),
        ) {
            let state = engine.status(run_id).map_err(map_core)?;
            if !(state["status"] == "cancelled" && e.code == "action_not_found") {
                return Err(map_core(e));
            }
        }
    }
    let annotate = |mut state: Value| -> Result<Value, RunnerError> {
        let active = jobs()
            .lock()
            .map_err(|_| error("driver_busy"))?
            .keys()
            .filter(|k| k.starts_with(&key_prefix))
            .count();
        state["driver"] = json!({"active_actions":active,"next_poll_ms":250});
        Ok(state)
    };
    let mut state = engine.next(run_id).map_err(map_core)?;
    if state["status"] == "paused" || state["status"] == "completed" {
        return annotate(state);
    }
    let actions = state["actions"].as_array().cloned().unwrap_or_default();
    for action in actions.iter().filter(|a| a["kind"] == "route") {
        let id = action["id"]
            .as_str()
            .ok_or_else(|| error("invalid_action"))?;
        if !engine.claim(run_id, id).map_err(map_core)? {
            continue;
        }
        match agent::route_with_judge(
            &action["payload"]["route_input"],
            judge.clone(),
            agent::now_ms(),
        ) {
            Ok(result) => {
                engine
                    .observe(
                        run_id,
                        &json!({"action_id":id,"event_id":format!("route:{id}"),"result":result}),
                    )
                    .map_err(map_core)?;
            }
            Err(e) => {
                state["attention"] = e.to_value();
                return annotate(state);
            }
        }
    }
    state = engine.next(run_id).map_err(map_core)?;
    if state["status"] == "paused" || state["status"] == "completed" {
        return annotate(state);
    }
    let actions = state["actions"].as_array().cloned().unwrap_or_default();
    let workspace = request["workspace"]
        .as_str()
        .ok_or_else(|| error("invalid_workspace"))?;
    let host_state = root.join(run_id).join("host");
    for action in actions {
        let kind = action["kind"].as_str().unwrap_or("");
        if ![
            "prepare",
            "launch",
            "inspect",
            "verify",
            "integrate",
            "gather_context",
        ]
        .contains(&kind)
        {
            continue;
        }
        // Paused/cancelled runs may reconcile existing owned work, never launch
        // another worker from an earlier lease.
        if state["status"] == "cancelled" && kind != "inspect" {
            continue;
        }
        let id = action["id"]
            .as_str()
            .ok_or_else(|| error("invalid_action"))?;
        let key = format!("{key_prefix}{id}");
        let mut map = jobs().lock().map_err(|_| error("driver_busy"))?;
        if map.contains_key(&key) {
            continue;
        }
        if !engine.claim(run_id, id).map_err(map_core)? {
            continue;
        }
        let host = LocalHost::new(
            Path::new(workspace),
            &host_state,
            &request.get("host").cloned().unwrap_or(json!({})),
        )
        .map_err(|e| RunnerError(e.to_value()))?;
        map.insert(
            key,
            thread::spawn(move || host.perform(&action).map_err(|e| e.to_value())),
        );
    }
    annotate(state)
}

pub fn run_to_completion(
    request: &Value,
    root: &Path,
    judge: Option<Arc<agent::ConfiguredJudge>>,
) -> Result<Value, RunnerError> {
    let engine = Workflow::new(root.to_path_buf());
    let state = engine.start(request).map_err(map_core)?;
    let run = state["run_id"]
        .as_str()
        .ok_or_else(|| error("invalid_run"))?;
    resume_to_completion(run, root, judge)
}

/// Drive until settled or a coordinator action (plan/repair/question) is needed.
/// The returned action is resumable through observe + next.
pub fn resume_to_completion(
    run: &str,
    root: &Path,
    judge: Option<Arc<agent::ConfiguredJudge>>,
) -> Result<Value, RunnerError> {
    let started = Instant::now();
    loop {
        let mut state = advance(root, run, judge.clone())?;
        let active = state["driver"]["active_actions"].as_u64().unwrap_or(0);
        let actions = state["actions"].as_array().cloned().unwrap_or_default();
        if active == 0 {
            if ["completed", "paused"]
                .iter()
                .any(|s| state["status"] == *s)
            {
                return Ok(state);
            }
            if state["status"] == "cancelled" {
                let unsettled = state["tasks"].as_array().is_some_and(|tasks| {
                    tasks
                        .iter()
                        .any(|t| t["status"] == "running" || t["status"] == "unknown")
                });
                if !unsettled {
                    return Ok(state);
                }
                if started.elapsed() > Duration::from_secs(60) {
                    state["next_action"] = json!("inspect_remaining");
                    return Ok(state);
                }
            }
            if actions
                .iter()
                .any(|a| ["plan", "repair", "review", "refresh_pool"].iter().any(|k| a["kind"] == *k))
            {
                state["next_action"] = json!("continue_with_coordinator");
                return Ok(state);
            }
            if state.get("attention").is_some() {
                return Ok(state);
            }
            if actions.is_empty()
                && state["tasks"].as_array().is_some_and(|tasks| {
                    tasks.iter().any(|t| t["status"]=="blocked"||t["status"]=="held")
                        && !tasks.iter().any(|t|["running","unknown"].iter().any(|s|t["status"]==*s))
                })
            {
                state["next_action"] = json!("continue_with_coordinator");
                return Ok(state);
            }
        }
        if started.elapsed() > Duration::from_secs(3600) && active == 0 {
            state["next_action"] = json!("resume_workflow");
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(200));
    }
}
