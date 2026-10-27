//! Durable, lease-based workflow coordination. Hosts execute actions, never this module.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

static MUTATION: Mutex<()> = Mutex::new(());
const LIMIT: usize = 1_048_576;
#[derive(Debug, Clone, Copy)]
pub struct WorkflowError {
    pub code: &'static str,
}
impl WorkflowError {
    pub fn to_value(&self) -> Value {
        json!({"code":self.code})
    }
}
impl fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}
impl std::error::Error for WorkflowError {}
fn err(code: &'static str) -> WorkflowError {
    WorkflowError { code }
}
fn bounded(v: &Value) -> Result<(), WorkflowError> {
    crate::contracts::bounded(v, LIMIT).map_err(|_| err("invalid_request"))
}
fn text<'a>(v: &'a Value, key: &str, max: usize) -> Result<&'a str, WorkflowError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= max)
        .ok_or(err("invalid_request"))
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
        && s != "."
        && s != ".."
}
fn strings(v: Option<&Value>, max: usize) -> Result<Vec<String>, WorkflowError> {
    match v {
        None => Ok(vec![]),
        Some(Value::Array(a)) if a.len() <= 64 => a
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty() && s.len() <= max)
                    .map(str::to_owned)
                    .ok_or(err("invalid_request"))
            })
            .collect(),
        _ => Err(err("invalid_request")),
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Task {
    node: Value,
    state: String,
    candidate: Option<Value>,
    prepared: Option<Value>,
    execution_id: Option<String>,
    attempts: u64,
    context_gathered: bool,
    #[serde(default)]
    context_summary: Option<String>,
    #[serde(default)]
    next_inspect_at_ms: u64,
    #[serde(default)]
    evidence: Vec<Value>,
}
#[derive(Serialize, Deserialize)]
struct State {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    catalog: Option<Value>,
    run_id: String,
    status: String,
    request: Value,
    tasks: Vec<Task>,
    actions: Vec<Value>,
    #[serde(default)]
    claimed_actions: BTreeSet<String>,
    events: BTreeMap<String, Value>,
    #[serde(default)]
    recent_events: Vec<Value>,
    sequence: u64,
    revision: u64,
}
#[derive(Clone)]
pub struct Workflow {
    root: PathBuf,
}
impl Workflow {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    fn lock(&self) -> Result<fs::File, WorkflowError> {
        fs::create_dir_all(&self.root).map_err(|_| err("storage_error"))?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(".lock"))
            .map_err(|_| err("storage_error"))?;
        lock_file(&file)?;
        Ok(file)
    }
    fn dir(&self, run: &str) -> Result<PathBuf, WorkflowError> {
        if !id(run) {
            return Err(err("invalid_request"));
        }
        Ok(self.root.join(run))
    }
    fn load(&self, run: &str) -> Result<State, WorkflowError> {
        let dir = self.dir(run)?;
        let mut files: Vec<_> = fs::read_dir(dir)
            .map_err(|_| err("not_found"))?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json")
                    .and_then(|n| n.parse::<u64>().ok())
                    .map(|n| (n, e.path()))
            })
            .collect();
        files.sort_by_key(|x| x.0);
        let path = files.last().ok_or(err("not_found"))?.1.clone();
        let data = fs::read(path).map_err(|_| err("storage_error"))?;
        serde_json::from_slice(&data).map_err(|_| err("storage_error"))
    }
    fn save(&self, s: &mut State) -> Result<(), WorkflowError> {
        s.revision += 1;
        let dir = self.dir(&s.run_id)?;
        fs::create_dir_all(&dir).map_err(|_| err("storage_error"))?;
        let tmp = dir.join(format!("{}.tmp", s.revision));
        let dest = dir.join(format!("{}.json", s.revision));
        if tmp.exists() {
            fs::remove_file(&tmp).map_err(|_| err("storage_error"))?;
        }
        let bytes = serde_json::to_vec(s).map_err(|_| err("storage_error"))?;
        {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|_| err("storage_error"))?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| err("storage_error"))?;
        }
        fs::rename(tmp, dest).map_err(|_| err("storage_error"))?;
        // Keep one prior recovery snapshot; retaining every cumulative state would grow quadratically.
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(n) = name
                    .strip_suffix(".json")
                    .and_then(|n| n.parse::<u64>().ok())
                    && n.saturating_add(1) < s.revision
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Ok(())
    }
    fn response(s: &State) -> Value {
        let done = s.tasks.iter().filter(|t| t.state == "done").count();
        let mut result = json!({"schema_version":s.schema_version,"run_id":s.run_id,"status":s.status,"summary":format!("{done}/{} tasks completed",s.tasks.len()),"tasks":s.tasks.iter().map(|t|json!({"task":t.node,"id":t.node["id"],"status":t.state,"candidate":t.candidate,"prepared":t.prepared,"execution_id":t.execution_id,"repair_rounds":t.attempts,"evidence":t.evidence,"next_inspect_at_ms":t.next_inspect_at_ms})).collect::<Vec<_>>(),"actions":s.actions,"events":s.recent_events,"event_count":s.events.len(),"event_cursor":{"first":s.events.len().saturating_sub(s.recent_events.len())+if s.recent_events.is_empty(){0}else{1},"last":s.events.len()},"revision":s.revision,"progress":{"done":done,"total":s.tasks.len(),"blocked":s.tasks.iter().filter(|t| ["held","blocked"].contains(&t.state.as_str())).count()}});
        if let Some(catalog) = &s.catalog {
            result["catalog"] = json!({"revision":catalog["revision"],"candidate_count":catalog["candidates"].as_array().map_or(0, Vec::len),"diagnostics":catalog["diagnostics"]});
        }
        result
    }
    pub fn start(&self, request: &Value) -> Result<Value, WorkflowError> {
        bounded(request)?;
        let obj = request.as_object().ok_or(err("invalid_request"))?;
        if obj.keys().any(|k| {
            ![
                "workspace",
                "goal",
                "plan",
                "pool",
                "catalog",
                "preferences",
                "max_parallel",
                "repair_rounds",
                "checks",
                "host",
            ]
            .contains(&k.as_str())
        }) {
            return Err(err("invalid_request"));
        }
        text(request, "workspace", 4096)?;
        if request.get("goal").is_some() {
            text(request, "goal", 24000)?;
        }
        if request.get("host").is_some_and(|v| !v.is_object()) {
            return Err(err("invalid_request"));
        }
        if request
            .get("preferences")
            .is_some_and(|v| v.as_str().is_none_or(|s| s.len() > 8000))
        {
            return Err(err("invalid_request"));
        }
        if let Some(checks) = request.get("checks") {
            let checks = checks
                .as_array()
                .filter(|a| a.len() <= 32)
                .ok_or(err("invalid_request"))?;
            for check in checks {
                let o = check.as_object().ok_or(err("invalid_request"))?;
                if o.keys().any(|k| !["program", "args"].contains(&k.as_str())) {
                    return Err(err("invalid_request"));
                }
                text(check, "program", 4096)?;
                strings(check.get("args"), 4096)?;
            }
        }
        let mut request = request.clone();
        let catalog = if let Some(pool) = request.get("pool") {
            validate_pool(pool)?;
            request.as_object_mut().unwrap().remove("catalog");
            None
        } else {
            let selection = request.get("catalog").cloned().unwrap_or(json!({}));
            let workspace = PathBuf::from(text(&request, "workspace", 4096)?);
            if !workspace.is_absolute() {
                return Err(err("invalid_workspace"));
            }
            let catalog = crate::catalog::snapshot(&workspace, &selection)
                .map_err(|_| err("catalog_unavailable"))?;
            validate_catalog(&catalog)?;
            request["catalog"] = selection;
            Some(catalog)
        };
        for (k, default, max) in [("max_parallel", 2, 16), ("repair_rounds", 2, 10)] {
            if let Some(v) = request.get(k)
                && v.as_u64()
                    .filter(|n| *n <= max && (k != "max_parallel" || *n > 0))
                    .is_none()
            {
                return Err(err("invalid_request"));
            }
            let _ = default;
        }
        let tasks = if let Some(plan) = request.get("plan") {
            validate_plan(plan)?
        } else {
            text(&request, "goal", 24000)?;
            vec![]
        };
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| err("storage_error"))?
            .as_nanos();
        let run = format!("run-{now}-{}", std::process::id());
        let mut s = State {
            schema_version: 2,
            catalog,
            run_id: run,
            status: "running".into(),
            request: request.clone(),
            tasks,
            actions: vec![],
            claimed_actions: BTreeSet::new(),
            events: BTreeMap::new(),
            recent_events: vec![],
            sequence: 0,
            revision: 0,
        };
        self.schedule(&mut s);
        self.save(&mut s)?;
        Ok(Self::response(&s))
    }
    /// Read the persisted request for a trusted local host constructing an adapter.
    pub fn request(&self, run_id: &str) -> Result<Value, WorkflowError> {
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        Ok(self.load(run_id)?.request)
    }
    /// Re-probe automatic targets on an explicit reconnect/refresh. Active
    /// attempts retain their exact candidate; only outstanding routes change.
    pub fn refresh_catalog(&self, run_id: &str) -> Result<Value, WorkflowError> {
        let request = self.request(run_id)?;
        let Some(mut selection) = request.get("catalog").cloned() else {
            return self.status(run_id);
        };
        selection["refresh"] = json!(true);
        let catalog = crate::catalog::snapshot(
            &PathBuf::from(text(&request, "workspace", 4096)?),
            &selection,
        )
        .map_err(|_| err("catalog_unavailable"))?;
        validate_catalog(&catalog)?;
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        let mut s = self.load(run_id)?;
        if s.status != "running" {
            return Ok(Self::response(&s));
        }
        s.catalog = Some(catalog);
        let abandoned: BTreeSet<String> = s
            .actions
            .iter()
            .filter(|a| a["kind"] == "route" || a["kind"] == "refresh_pool")
            .filter(|a| {
                !a["id"]
                    .as_str()
                    .is_some_and(|id| s.claimed_actions.contains(id))
            })
            .filter_map(|a| a["id"].as_str().map(str::to_owned))
            .collect();
        s.actions
            .retain(|a| !a["id"].as_str().is_some_and(|id| abandoned.contains(id)));
        s.claimed_actions.retain(|id| !abandoned.contains(id));
        for task in &mut s.tasks {
            if ["routing", "needs_pool", "awaiting_pool", "held"].contains(&task.state.as_str())
                && !s.actions.iter().any(|a| a["task_id"] == task.node["id"])
            {
                task.state = "pending".into();
            }
        }
        self.schedule(&mut s);
        self.save(&mut s)?;
        Ok(Self::response(&s))
    }
    pub fn status(&self, run_id: &str) -> Result<Value, WorkflowError> {
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        Ok(Self::response(&self.load(run_id)?))
    }
    pub fn next(&self, run_id: &str) -> Result<Value, WorkflowError> {
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        let mut s = self.load(run_id)?;
        let before = serde_json::to_vec(&s).map_err(|_| err("storage_error"))?;
        self.schedule(&mut s);
        if serde_json::to_vec(&s).map_err(|_| err("storage_error"))? != before {
            self.save(&mut s)?;
        }
        Ok(Self::response(&s))
    }
    /// Check dispatch authorization against the current durable lease and run state.
    /// Hosts must still provide idempotent claims for the side effect itself.
    pub fn claim(&self, run_id: &str, action_id: &str) -> Result<bool, WorkflowError> {
        if !id(action_id) {
            return Err(err("invalid_request"));
        }
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        let mut state = self.load(run_id)?;
        let Some(action) = state.actions.iter().find(|a| a["id"] == action_id) else {
            return Ok(false);
        };
        let allowed = state.status == "running"
            || (state.status == "cancelled"
                && ((action["kind"] == "inspect" && action["payload"]["operation"] == "cancel")
                    || (state.claimed_actions.contains(action_id)
                        && ["prepare", "verify", "integrate", "gather_context"]
                            .contains(&action["kind"].as_str().unwrap_or("")))));
        if allowed && state.claimed_actions.insert(action_id.to_owned()) {
            self.save(&mut state)?;
        }
        Ok(allowed)
    }
    pub fn control(&self, run_id: &str, action: &str) -> Result<Value, WorkflowError> {
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        let mut s = self.load(run_id)?;
        match action {
            "pause" if s.status == "running" => s.status = "paused".into(),
            "resume" if s.status == "paused" => {
                s.status = "running".into();
                self.schedule(&mut s)
            }
            "cancel" if s.status == "running" || s.status == "paused" => {
                s.status = "cancelled".into();
                // A launch lease may already be executing even if no receipt was observed.
                for a in &s.actions {
                    if a["kind"] == "launch"
                        && a["id"]
                            .as_str()
                            .is_some_and(|id| s.claimed_actions.contains(id))
                        && let Some(i) = s.tasks.iter().position(|t| t.node["id"] == a["task_id"])
                    {
                        s.tasks[i].execution_id = a["id"].as_str().map(str::to_owned);
                        s.tasks[i].state = "unknown".into();
                    }
                }
                s.actions.retain(|a| {
                    a["id"]
                        .as_str()
                        .is_some_and(|id| s.claimed_actions.contains(id))
                        && ["prepare", "verify", "integrate", "gather_context"]
                            .contains(&a["kind"].as_str().unwrap_or(""))
                });
                s.claimed_actions
                    .retain(|id| s.actions.iter().any(|a| a["id"] == *id));
                for task in &mut s.tasks {
                    if task.state != "done"
                        && !["running", "unknown"].contains(&task.state.as_str())
                        && !s.actions.iter().any(|a| a["task_id"] == task.node["id"])
                    {
                        task.state = "cancelled".into();
                    }
                }
                for i in 0..s.tasks.len() {
                    if s.tasks[i].execution_id.is_some()
                        && ["running", "unknown"].contains(&s.tasks[i].state.as_str())
                    {
                        let execution_id = s.tasks[i].execution_id.clone();
                        self.action(
                            &mut s,
                            "inspect",
                            Some(i),
                            json!({"operation":"cancel","execution_id":execution_id}),
                        );
                    }
                }
            }
            "pause" | "resume" | "cancel" => return Err(err("invalid_state")),
            _ => return Err(err("invalid_request")),
        };
        self.save(&mut s)?;
        Ok(Self::response(&s))
    }
    pub fn observe(&self, run_id: &str, event: &Value) -> Result<Value, WorkflowError> {
        let _g = MUTATION.lock().map_err(|_| err("storage_error"))?;
        let _lock = self.lock()?;
        bounded(event)?;
        let eid = text(event, "event_id", 128)?;
        let aid = text(event, "action_id", 128)?;
        if !id(eid)
            || !id(aid)
            || !event.get("result").is_some_and(Value::is_object)
            || event
                .as_object()
                .unwrap()
                .keys()
                .any(|k| !["event_id", "action_id", "result"].contains(&k.as_str()))
        {
            return Err(err("invalid_request"));
        }
        let mut s = self.load(run_id)?;
        let digest =
            crate::core::sha256(&serde_json::to_vec(event).map_err(|_| err("invalid_request"))?);
        if let Some(old) = s.events.get(eid) {
            return if old.as_str() == Some(digest.as_str()) || old == event {
                Ok(Self::response(&s))
            } else {
                Err(err("event_conflict"))
            };
        }
        if s.events.len() >= 4096 {
            return Err(err("event_limit"));
        }
        let pos = s
            .actions
            .iter()
            .position(|a| a["id"] == aid)
            .ok_or(err("action_not_found"))?;
        let a = s.actions[pos].clone();
        let result = &event["result"];
        let kind = a["kind"].as_str().unwrap();
        let index = a
            .get("task_id")
            .and_then(Value::as_str)
            .and_then(|id| s.tasks.iter().position(|t| t.node["id"] == id));
        match kind {
            "plan" => {
                s.tasks = validate_plan(result.get("plan").ok_or(err("invalid_receipt"))?)
                    .map_err(|_| err("invalid_receipt"))?;
            }
            "repair" => {
                let i = index.ok_or(err("invalid_receipt"))?;
                let nodes = if let Some(plan) = result.get("plan") {
                    plan.get("nodes")
                        .and_then(Value::as_array)
                        .ok_or(err("invalid_receipt"))?
                        .clone()
                } else {
                    vec![result.get("task").ok_or(err("invalid_receipt"))?.clone()]
                };
                if nodes.is_empty() || !nodes.iter().any(|n| n["id"] == s.tasks[i].node["id"]) {
                    return Err(err("invalid_receipt"));
                }
                let mut merged = s.tasks.iter().map(|t| t.node.clone()).collect::<Vec<_>>();
                let mut seen = BTreeSet::new();
                for node in nodes {
                    let name = text(&node, "id", 128).map_err(|_| err("invalid_receipt"))?;
                    if !seen.insert(name.to_owned()) {
                        return Err(err("invalid_receipt"));
                    }
                    let j = s
                        .tasks
                        .iter()
                        .position(|t| t.node["id"] == name)
                        .ok_or(err("invalid_receipt"))?;
                    if j != i && s.tasks[j].state != "pending" && s.tasks[j].state != "blocked" {
                        return Err(err("invalid_receipt"));
                    }
                    merged[j] = node;
                }
                let validated =
                    validate_plan(&json!({"nodes": merged})).map_err(|_| err("invalid_receipt"))?;
                for (old, new) in s.tasks.iter_mut().zip(validated) {
                    old.node = new.node;
                }
                s.tasks[i].state = "pending".into();
                s.tasks[i].candidate = None;
                s.tasks[i].prepared = None;
                s.tasks[i].execution_id = None;
            }
            "route" => {
                let i = index.ok_or(err("invalid_receipt"))?;
                match result["status"].as_str() {
                    Some("ready") => {
                        let cid = text(result, "candidate_id", 128)
                            .map_err(|_| err("invalid_receipt"))?;
                        let candidates = a["payload"]
                            .get("candidate_snapshot")
                            .unwrap_or(&s.request["pool"]);
                        let c = candidates
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|c| {
                                c["id"] == cid
                                    && c["available"] == true
                                    && strings(s.tasks[i].node.get("requirements"), 128)
                                        .unwrap_or_default()
                                        .iter()
                                        .all(|r| {
                                            c["capabilities"]
                                                .as_array()
                                                .is_some_and(|a| a.iter().any(|v| v == r))
                                        })
                            })
                            .ok_or(err("invalid_receipt"))?
                            .clone();
                        if review_exclusions(&s, i).contains(cid)
                            || !crate::route_context::matches_pin(&c, &s.tasks[i].node)
                        {
                            return Err(err("invalid_receipt"));
                        }
                        let current_pool = s
                            .catalog
                            .as_ref()
                            .map(|v| &v["candidates"])
                            .unwrap_or(&s.request["pool"]);
                        let still_available = current_pool.as_array().is_some_and(|pool| {
                            pool.iter().any(|now| {
                                now["id"] == c["id"]
                                    && now["available"] == true
                                    && [
                                        "provider",
                                        "model",
                                        "effort",
                                        "executable",
                                        "resource_key",
                                        "args",
                                    ]
                                    .iter()
                                    .all(|key| now.get(*key) == c.get(*key))
                                    && strings(s.tasks[i].node.get("requirements"), 128)
                                        .unwrap_or_default()
                                        .iter()
                                        .all(|r| {
                                            now["capabilities"]
                                                .as_array()
                                                .is_some_and(|a| a.iter().any(|v| v == r))
                                        })
                            })
                        });
                        s.tasks[i].candidate = still_available.then_some(c);
                        s.tasks[i].state =
                            if still_available { "routed" } else { "pending" }.into();
                    }
                    Some("hold") => {
                        s.tasks[i].state = if result["reason_codes"]
                            .as_array()
                            .is_some_and(|r| r.iter().any(|v| v == "context_insufficient"))
                            && !s.tasks[i].context_gathered
                        {
                            "needs_context"
                        } else {
                            "needs_pool"
                        }
                        .into();
                    }
                    _ => return Err(err("invalid_receipt")),
                }
            }
            "refresh_pool" => {
                if let Some(catalog) = result.get("catalog") {
                    validate_catalog(catalog).map_err(|_| err("invalid_receipt"))?;
                    s.catalog = Some(catalog.clone());
                    s.request["catalog"] = s.request.get("catalog").cloned().unwrap_or(json!({}));
                    s.request.as_object_mut().unwrap().remove("pool");
                } else {
                    let pool = result.get("pool").ok_or(err("invalid_receipt"))?;
                    validate_pool(pool).map_err(|_| err("invalid_receipt"))?;
                    let replacement = pool.as_array().unwrap();
                    for task in &s.tasks {
                        if [
                            "routed",
                            "preparing",
                            "prepared",
                            "launching",
                            "running",
                            "unknown",
                            "verifying",
                            "integrating",
                        ]
                        .contains(&task.state.as_str())
                            && let Some(candidate) = &task.candidate
                            && !replacement.iter().any(|c| c == candidate)
                        {
                            return Err(err("invalid_receipt"));
                        }
                    }
                    s.request["pool"] = pool.clone();
                    s.request.as_object_mut().unwrap().remove("catalog");
                    s.catalog = None;
                }
                for task in &mut s.tasks {
                    if ["needs_pool", "awaiting_pool", "held", "routing"]
                        .contains(&task.state.as_str())
                        && !s.actions.iter().any(|a| {
                            a["task_id"] == task.node["id"]
                                && a["id"]
                                    .as_str()
                                    .is_some_and(|id| s.claimed_actions.contains(id))
                        })
                    {
                        task.state = "pending".into();
                    }
                }
            }
            "prepare" => {
                let i = index.ok_or(err("invalid_receipt"))?;
                if result["status"] == "failed" && host_error(result).is_some() {
                    s.tasks[i].state = "failed".into();
                } else if result["status"] != "prepared"
                    || [
                        "workspace",
                        "branch",
                        "integration_workspace",
                        "integration_branch",
                    ]
                    .iter()
                    .any(|k| text(result, k, 4096).is_err())
                {
                    return Err(err("invalid_receipt"));
                }
                if result["status"] == "prepared" {
                    s.tasks[i].prepared = Some(result.clone());
                    s.tasks[i].state = "prepared".into();
                }
            }
            "launch" | "inspect" => {
                let i = index.ok_or(err("invalid_receipt"))?;
                let expected = if kind == "launch" {
                    Some(aid)
                } else {
                    s.tasks[i].execution_id.as_deref()
                };
                let received = result.get("execution_id").and_then(Value::as_str);
                if received.is_some() && received != expected {
                    return Err(err("invalid_receipt"));
                }
                if result["status"] == "completed" && (expected.is_none() || received != expected) {
                    return Err(err("invalid_receipt"));
                }
                match result["status"].as_str() {
                    Some("running") | Some("unknown") => {
                        let ex = result
                            .get("execution_id")
                            .and_then(Value::as_str)
                            .or(s.tasks[i].execution_id.as_deref())
                            .filter(|x| !x.is_empty() && x.len() <= 1024)
                            .ok_or(err("invalid_receipt"))?
                            .to_string();
                        if s.tasks[i]
                            .execution_id
                            .as_ref()
                            .is_some_and(|old| old != &ex)
                        {
                            return Err(err("invalid_receipt"));
                        }
                        s.tasks[i].execution_id = Some(ex);
                        s.tasks[i].state = result["status"].as_str().unwrap().into();
                        s.tasks[i].next_inspect_at_ms = now_ms().saturating_add(1000);
                    }
                    Some("completed") if result["completion_verified"] == true => {
                        if let (Some(expected), Some(received)) = (
                            s.tasks[i].execution_id.as_deref(),
                            result.get("execution_id").and_then(Value::as_str),
                        ) && expected != received
                        {
                            return Err(err("invalid_receipt"));
                        }
                        s.tasks[i].state = "verifying".into();
                    }
                    Some("failed") => {
                        s.tasks[i].state =
                            if s.status == "cancelled" && result["cancelled"] == true {
                                "cancelled"
                            } else {
                                "failed"
                            }
                            .into();
                    }
                    Some("cancelled") if s.status == "cancelled" => {
                        s.tasks[i].state = "cancelled".into();
                    }
                    _ => return Err(err("invalid_receipt")),
                }
            }
            "verify" => {
                let i = index.ok_or(err("invalid_receipt"))?;
                if !result["evidence"].is_array()
                    && !(result["status"] == "failed" && host_error(result).is_some())
                {
                    return Err(err("invalid_receipt"));
                }
                s.tasks[i].state = match result["status"].as_str() {
                    Some("passed") => "integrating",
                    Some("failed") => "failed",
                    _ => return Err(err("invalid_receipt")),
                }
                .into();
            }
            "integrate" => {
                let i = index.ok_or(err("invalid_receipt"))?;
                s.tasks[i].state = match result["status"].as_str() {
                    Some("integrated") => "done",
                    Some("failed") => "failed",
                    _ => return Err(err("invalid_receipt")),
                }
                .into();
            }
            "gather_context" => {
                let i = index.ok_or(err("invalid_receipt"))?;
                if result["status"] == "failed" && host_error(result).is_some() {
                    s.tasks[i].state = "failed".into();
                } else if result["status"] != "gathered" {
                    return Err(err("invalid_receipt"));
                }
                if result["status"] == "gathered" {
                    let refs = strings(result.get("context_refs"), 1000)
                        .map_err(|_| err("invalid_receipt"))?;
                    s.tasks[i].node["context_refs"] = json!(refs);
                    s.tasks[i].context_summary = match result.get("summary") {
                        None => None,
                        Some(Value::String(v)) if v.len() <= 4000 => Some(v.clone()),
                        _ => return Err(err("invalid_receipt")),
                    };
                    s.tasks[i].context_gathered = true;
                    s.tasks[i].state = "pending".into();
                }
            }
            _ => return Err(err("invalid_receipt")),
        }
        if let Some(i) = index {
            if s.status == "cancelled" && ["prepare", "verify", "gather_context"].contains(&kind) {
                s.tasks[i].state = "cancelled".into();
            }
            let summary = evidence_summary(kind, result);
            if s.tasks[i].evidence.len() >= 64 {
                s.tasks[i].evidence.remove(0);
            }
            s.tasks[i].evidence.push(summary);
        }
        let mut summary = evidence_summary(kind, result);
        summary["sequence"] = json!(s.events.len() + 1);
        summary["event_id"] = json!(eid);
        summary["action_id"] = json!(aid);
        summary["kind"] = json!(kind);
        summary["task_id"] = a["task_id"].clone();
        if s.recent_events.len() >= 32 {
            s.recent_events.remove(0);
        }
        s.recent_events.push(summary);
        s.actions.remove(pos);
        if kind == "refresh_pool" {
            s.actions.retain(|a| {
                (a["kind"] != "refresh_pool" && a["kind"] != "route")
                    || a["id"]
                        .as_str()
                        .is_some_and(|id| s.claimed_actions.contains(id))
            });
        }
        s.claimed_actions
            .retain(|id| s.actions.iter().any(|a| a["id"] == *id));
        s.events.insert(eid.to_string(), json!(digest));
        self.schedule(&mut s);
        self.save(&mut s)?;
        Ok(Self::response(&s))
    }
    fn action(&self, s: &mut State, kind: &str, index: Option<usize>, extra: Value) {
        s.sequence += 1;
        let mut payload = json!({"workspace":s.request["workspace"],"checks":s.request.get("checks").cloned().unwrap_or(json!([])),"host":s.request.get("host").cloned().unwrap_or(json!({}))});
        let mut tid = Value::Null;
        if let Some(i) = index {
            let t = &s.tasks[i];
            tid = t.node["id"].clone();
            payload["task"] = t.node.clone();
            payload["attempt"] = json!(t.attempts);
            payload["candidate"] = t.candidate.clone().unwrap_or(Value::Null);
            payload["prepared"] = t.prepared.clone().unwrap_or(Value::Null);
            if let Some(ex) = &t.execution_id {
                payload["execution_id"] = json!(ex);
            }
        }
        if let Some(m) = extra.as_object() {
            for (k, v) in m {
                payload[k] = v.clone();
            }
        }
        s.actions.push(json!({"id":format!("{}-action-{}",s.run_id,s.sequence),"kind":kind,"run_id":s.run_id,"task_id":tid,"payload":payload}));
    }
    fn schedule(&self, s: &mut State) {
        if s.status == "cancelled" {
            for i in 0..s.tasks.len() {
                if ["running", "unknown"].contains(&s.tasks[i].state.as_str())
                    && s.tasks[i].next_inspect_at_ms <= now_ms()
                    && !s
                        .actions
                        .iter()
                        .any(|a| a["task_id"] == s.tasks[i].node["id"])
                {
                    let execution_id = s.tasks[i].execution_id.clone();
                    self.action(
                        s,
                        "inspect",
                        Some(i),
                        json!({"operation":"cancel", "execution_id":execution_id}),
                    );
                }
            }
            return;
        }
        if s.status != "running" {
            return;
        }
        if s.tasks.is_empty() {
            if s.actions.is_empty() {
                self.action(s, "plan", None, json!({"goal":s.request["goal"]}));
            }
            return;
        }
        if s.tasks.iter().all(|t| t.state == "done") {
            s.status = "completed".into();
            return;
        }
        let max = s.request["max_parallel"].as_u64().unwrap_or(2) as usize;
        for i in 0..s.tasks.len() {
            let tid = s.tasks[i].node["id"].clone();
            if s.actions.iter().any(|a| a["task_id"] == tid) {
                continue;
            }
            let state = s.tasks[i].state.clone();
            let deps = strings(s.tasks[i].node.get("depends_on"), 128).unwrap_or_default();
            if state == "pending"
                && !deps.iter().all(|d| {
                    s.tasks
                        .iter()
                        .any(|t| t.node["id"] == *d && t.state == "done")
                })
            {
                continue;
            }
            let active = s
                .tasks
                .iter()
                .filter(|t| {
                    ![
                        "pending",
                        "done",
                        "held",
                        "failed",
                        "blocked",
                        "needs_pool",
                        "awaiting_pool",
                    ]
                    .contains(&t.state.as_str())
                })
                .count();
            match state.as_str() {
                "pending" if active < max => {
                    let node = &s.tasks[i].node;
                    let dependencies: Vec<Value> = s.tasks.iter().filter(|t| deps.iter().any(|d| t.node["id"] == *d))
                        .map(|t| json!({"id":t.node["id"],"outputs":t.node["outputs"],"status":t.state,"integration":t.evidence.iter().rev().find(|e| e["stage"] == "integrate")})).collect();
                    let task = match crate::route_context::task(
                        node,
                        s.tasks[i].context_summary.as_deref(),
                        &dependencies,
                    ) {
                        Ok(task) => task,
                        Err(code) => {
                            s.tasks[i].state = "repairing".into();
                            self.action(s, "repair", Some(i), json!({"reason":code,"summary":"Split this task so its acceptance contract fits the routing input."}));
                            continue;
                        }
                    };
                    let profile = match node["role"].as_str() {
                        Some("review") => "review",
                        Some("implementation") | Some("integration") => "implementation",
                        Some("mockup") => "mockup",
                        _ => "balanced",
                    };
                    let exclusions = review_exclusions(s, i);
                    let preferences = if s.catalog.is_some() {
                        crate::route_context::preferences(&s.request)
                    } else {
                        s.request["preferences"].as_str().unwrap_or("")
                    };
                    let candidates = if let Some(catalog) = &s.catalog {
                        let mut eligible = catalog.clone();
                        if let Some(candidates) = eligible["candidates"].as_array_mut() {
                            candidates.retain(|c| {
                                !c["id"].as_str().is_some_and(|id| exclusions.contains(id))
                            });
                        }
                        crate::catalog::shortlist(&eligible, node, preferences, 16)
                            .unwrap_or_default()
                            .iter()
                            .map(clean_candidate)
                            .collect::<Vec<_>>()
                    } else {
                        s.request["pool"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|c| crate::route_context::matches_pin(c, node))
                            .cloned()
                            .collect()
                    };
                    let revision = s.catalog.as_ref().map(|c| c["revision"].clone());
                    let pool: Vec<Value> = candidates
                        .iter()
                        .map(crate::route_context::candidate)
                        .collect();
                    let policy_exclusions: Vec<_> = exclusions
                        .iter()
                        .filter(|id| pool.iter().any(|c| c["id"].as_str() == Some(id.as_str())))
                        .collect();
                    let route_input = json!({"version":2,"task":task,"pool":pool,"preferences":preferences,"policy":{"profile":profile,"tie_behavior":"stable_id","excluded_candidate_ids":policy_exclusions}});
                    if crate::routing::normalize(&route_input).is_err() {
                        s.tasks[i].state = "repairing".into();
                        self.action(s, "repair", Some(i), json!({"reason":"routing_input_too_large","summary":"Reduce this task's routing context while preserving its acceptance contract."}));
                        continue;
                    }
                    s.tasks[i].state = "routing".into();
                    self.action(s,"route",Some(i),json!({"route_input":route_input,"candidate_snapshot":candidates,"catalog_revision":revision}));
                }
                "routed" => {
                    if self.conflicts(s, i) {
                        continue;
                    }
                    s.tasks[i].state = "preparing".into();
                    self.action(s, "prepare", Some(i), json!({}));
                }
                "prepared" => {
                    s.tasks[i].state = "launching".into();
                    self.action(s, "launch", Some(i), json!({}));
                }
                "running" | "unknown" if s.tasks[i].next_inspect_at_ms <= now_ms() => {
                    self.action(s, "inspect", Some(i), json!({}));
                }
                "verifying" => self.action(s, "verify", Some(i), json!({})),
                "integrating" => self.action(s, "integrate", Some(i), json!({})),
                "needs_pool" => {
                    s.tasks[i].state = "awaiting_pool".into();
                    self.action(
                        s,
                        "refresh_pool",
                        Some(i),
                        json!({"evidence":s.tasks[i].evidence}),
                    );
                }
                "needs_context" => {
                    s.tasks[i].state = "gathering".into();
                    self.action(s, "gather_context", Some(i), json!({}));
                }
                "failed" => {
                    if s.tasks[i].attempts < s.request["repair_rounds"].as_u64().unwrap_or(2) {
                        s.tasks[i].attempts += 1;
                        s.tasks[i].state = "repairing".into();
                        self.action(
                            s,
                            "repair",
                            Some(i),
                            json!({"round":s.tasks[i].attempts, "evidence":s.tasks[i].evidence}),
                        );
                    } else {
                        s.tasks[i].state = "blocked".into();
                    }
                }
                _ => {}
            }
        }
    }
    fn conflicts(&self, s: &State, i: usize) -> bool {
        let c = &s.tasks[i];
        let key = c
            .candidate
            .as_ref()
            .and_then(|v| v.get("resource_key").or_else(|| v.get("id")))
            .and_then(Value::as_str);
        let outputs = strings(
            c.node.get("write_paths").or_else(|| c.node.get("outputs")),
            1000,
        )
        .unwrap_or_default();
        s.tasks.iter().enumerate().any(|(j, t)| {
            if j == i
                || ![
                    "preparing",
                    "prepared",
                    "launching",
                    "running",
                    "unknown",
                    "verifying",
                    "integrating",
                ]
                .contains(&t.state.as_str())
            {
                return false;
            }
            let other = t
                .candidate
                .as_ref()
                .and_then(|v| v.get("resource_key").or_else(|| v.get("id")))
                .and_then(Value::as_str);
            let paths = strings(
                t.node.get("write_paths").or_else(|| t.node.get("outputs")),
                1000,
            )
            .unwrap_or_default();
            (key.is_some() && key == other)
                || outputs
                    .iter()
                    .any(|a| paths.iter().any(|b| path_overlap(a, b)))
        })
    }
}
fn clean_candidate(candidate: &Value) -> Value {
    let mut c = candidate.clone();
    if let Some(object) = c.as_object_mut() {
        object.remove("catalog_info");
    }
    c
}
fn validate_catalog(value: &Value) -> Result<(), WorkflowError> {
    text(value, "revision", 128)?;
    let candidates = value["candidates"]
        .as_array()
        .filter(|a| a.len() <= 1024)
        .ok_or(err("invalid_catalog"))?;
    let mut ids = BTreeSet::new();
    for chunk in candidates.chunks(16) {
        let clean: Vec<Value> = chunk.iter().map(clean_candidate).collect();
        validate_pool(&json!(clean))?;
        for c in chunk {
            if !ids.insert(text(c, "id", 128)?.to_owned()) {
                return Err(err("invalid_catalog"));
            }
        }
    }
    Ok(())
}
fn validate_pool(value: &Value) -> Result<(), WorkflowError> {
    let pool = value
        .as_array()
        .filter(|p| p.len() <= 16)
        .ok_or(err("invalid_request"))?;
    let mut ids = BTreeSet::new();
    for c in pool {
        let obj = c.as_object().ok_or(err("invalid_request"))?;
        if obj.keys().any(|k| {
            ![
                "id",
                "description",
                "capabilities",
                "available",
                "estimated_cost",
                "estimated_latency_ms",
                "has_context",
                "provider",
                "model",
                "effort",
                "executable",
                "resource_key",
                "args",
            ]
            .contains(&k.as_str())
        }) {
            return Err(err("invalid_request"));
        }
        for key in ["provider", "model", "effort", "executable", "resource_key"] {
            if c.get(key).is_some() {
                text(c, key, 4096)?;
            }
        }
        strings(c.get("args"), 4096)?;
        let mut route_candidate = c.clone();
        route_candidate.as_object_mut().unwrap().retain(|k, _| {
            [
                "id",
                "description",
                "capabilities",
                "available",
                "estimated_cost",
                "estimated_latency_ms",
                "has_context",
            ]
            .contains(&k.as_str())
        });
        crate::routing::normalize(&json!({"version":2,"task":{"id":"validation","objective":"validation"},"pool":[route_candidate]})).map_err(|_| err("invalid_request"))?;
        let cid = text(c, "id", 128)?;
        if !id(cid) || !ids.insert(cid.to_owned()) {
            return Err(err("invalid_request"));
        }
    }
    Ok(())
}
fn review_exclusions(s: &State, index: usize) -> BTreeSet<String> {
    let task = &s.tasks[index];
    if task.node["role"] != "review" {
        return BTreeSet::new();
    }
    let deps = strings(task.node.get("depends_on"), 128).unwrap_or_default();
    s.tasks
        .iter()
        .filter(|t| deps.iter().any(|id| t.node["id"] == *id))
        .filter_map(|t| t.candidate.as_ref()?.get("id")?.as_str().map(str::to_owned))
        .collect()
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn host_error(result: &Value) -> Option<&str> {
    result.get("error")?.get("code")?.as_str().filter(|s| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    })
}
fn evidence_summary(kind: &str, result: &Value) -> Value {
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .filter(|s| {
            [
                "ready",
                "hold",
                "prepared",
                "running",
                "unknown",
                "completed",
                "failed",
                "cancelled",
                "passed",
                "integrated",
                "gathered",
            ]
            .contains(s)
        })
        .unwrap_or("accepted");
    let mut summary = json!({"stage":kind, "status":status});
    for key in ["message_ref", "stream_ref"] {
        if let Some(reference) = result[key]
            .as_str()
            .filter(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        {
            summary[key] = json!(reference);
        }
    }
    if let Some(code) = result
        .get("reason_codes")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
    {
        summary["reason_code"] = json!(code);
    }
    if let Some(action) = result
        .get("next_action")
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
    {
        summary["next_action"] = json!(action);
    }
    if let Some(code) = host_error(result) {
        summary["error_code"] = json!(code);
    }
    if let Some(evidence) = result.get("evidence").and_then(Value::as_array) {
        summary["checks_passed"] =
            json!(evidence.iter().filter(|e| e["status"] == "passed").count());
        summary["checks_failed"] =
            json!(evidence.iter().filter(|e| e["status"] == "failed").count());
        summary["evidence_count"] = json!(evidence.len());
    }
    if let Some(commit) = result
        .get("commit")
        .and_then(Value::as_str)
        .filter(|s| (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        summary["commit"] = json!(commit);
    }
    summary
}
// Reserve the containing directory of a glob conservatively.
fn ownership_prefix(path: &str) -> String {
    let path = path.replace('\\', "/");
    if let Some(pos) = path.find('*') {
        let prefix = &path[..pos];
        return prefix
            .rsplit_once('/')
            .map(|(dir, _)| if dir.is_empty() { "." } else { dir })
            .unwrap_or(".")
            .to_owned();
    }
    path.trim_end_matches('/').to_owned()
}
fn path_overlap(a: &str, b: &str) -> bool {
    if a.contains("://")
        || b.contains("://")
        || a.starts_with("artifact:")
        || b.starts_with("artifact:")
    {
        return false;
    }
    let a = ownership_prefix(a);
    let b = ownership_prefix(b);
    let a = if cfg!(windows) {
        a.to_ascii_lowercase()
    } else {
        a
    };
    let b = if cfg!(windows) {
        b.to_ascii_lowercase()
    } else {
        b
    };
    a == "." || b == "." || a == b || a.starts_with(&(b.clone() + "/")) || b.starts_with(&(a + "/"))
}
fn validate_plan(plan: &Value) -> Result<Vec<Task>, WorkflowError> {
    if plan
        .as_object()
        .is_none_or(|o| o.keys().any(|k| k != "nodes"))
    {
        return Err(err("invalid_request"));
    }
    let nodes = plan
        .get("nodes")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .ok_or(err("invalid_request"))?;
    let mut ids = BTreeSet::new();
    let mut tasks = Vec::new();
    for n in nodes {
        let obj = n.as_object().ok_or(err("invalid_request"))?;
        if obj.keys().any(|k| {
            ![
                "id",
                "objective",
                "depends_on",
                "requirements",
                "acceptance",
                "outputs",
                "write_paths",
                "role",
                "context_refs",
                "routing",
            ]
            .contains(&k.as_str())
        }) {
            return Err(err("invalid_request"));
        }
        let name = text(n, "id", 128)?;
        if !id(name) || !ids.insert(name.to_owned()) {
            return Err(err("invalid_request"));
        }
        text(n, "objective", 24000)?;
        if let Some(pin) = n.get("routing") {
            let pin = pin.as_object().ok_or(err("invalid_request"))?;
            if pin.is_empty()
                || pin
                    .keys()
                    .any(|k| !["candidate_id", "provider", "model", "effort"].contains(&k.as_str()))
            {
                return Err(err("invalid_request"));
            }
            for key in pin.keys() {
                text(
                    &n["routing"],
                    key,
                    if key == "candidate_id" { 128 } else { 256 },
                )?;
            }
        }
        for field in [
            "depends_on",
            "requirements",
            "acceptance",
            "outputs",
            "write_paths",
            "context_refs",
        ] {
            strings(n.get(field), 1000)?;
        }
        for output in strings(n.get("outputs"), 1000)?
            .into_iter()
            .chain(strings(n.get("write_paths"), 1000)?)
        {
            if output.starts_with("artifact:") || output.contains("://") {
                continue;
            }
            let normalized = output.replace('\\', "/");
            if normalized == "." {
                continue;
            }
            let normalized = normalized.trim_end_matches('/');
            if normalized.starts_with('/')
                || normalized.contains(':')
                || normalized
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(err("invalid_request"));
            }
        }
        if let Some(role) = n.get("role")
            && ![
                "exploration",
                "implementation",
                "mockup",
                "integration",
                "review",
            ]
            .contains(&role.as_str().unwrap_or(""))
        {
            return Err(err("invalid_request"));
        }
        tasks.push(Task {
            node: n.clone(),
            state: "pending".into(),
            candidate: None,
            prepared: None,
            execution_id: None,
            attempts: 0,
            context_gathered: false,
            context_summary: None,
            next_inspect_at_ms: 0,
            evidence: vec![],
        });
    }
    let mut done = BTreeSet::new();
    loop {
        let before = done.len();
        for t in &tasks {
            let deps = strings(t.node.get("depends_on"), 128)?;
            if deps.iter().any(|d| !ids.contains(d)) {
                return Err(err("invalid_request"));
            }
            if deps.iter().all(|d| done.contains(d)) {
                done.insert(t.node["id"].as_str().unwrap().to_string());
            }
        }
        if done.len() == tasks.len() {
            break;
        }
        if done.len() == before {
            return Err(err("invalid_request"));
        }
    }
    Ok(tasks)
}

#[cfg(unix)]
fn lock_file(file: &fs::File) -> Result<(), WorkflowError> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    if unsafe { flock(file.as_raw_fd(), 2) } == 0 {
        Ok(())
    } else {
        Err(err("storage_error"))
    }
}
#[cfg(windows)]
fn lock_file(file: &fs::File) -> Result<(), WorkflowError> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut std::ffi::c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFileEx(
            file: *mut std::ffi::c_void,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    let mut o = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: std::ptr::null_mut(),
    };
    if unsafe { LockFileEx(file.as_raw_handle(), 2, 0, 1, 0, &mut o) } != 0 {
        Ok(())
    } else {
        Err(err("storage_error"))
    }
}
