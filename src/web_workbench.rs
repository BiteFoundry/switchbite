//! Web submission uses the same durable workflow and native host as MCP.
use crate::{
    adapters::LocalHost, agent::ConfiguredJudge, catalog, web_coordinator, workflow::Workflow,
    workflow_runner,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

struct Run {
    root: PathBuf,
    id: String,
    candidate: Value,
    coordinator: Mutex<Value>,
    active_action: Mutex<Option<Value>>,
    latest_coordinator_action: Mutex<Option<Value>>,
    alive: AtomicBool,
    attempt: AtomicU64,
    tools: Vec<String>,
}

impl Run {
    fn checkpoint(&self) -> Result<(), Value> {
        let mut coordinator = self.coordinator.lock().map_err(|_| lock_error())?.clone();
        for key in ["provider", "model", "effort"] {
            coordinator[key] = self.candidate[key].clone();
        }
        let latest = self
            .latest_coordinator_action
            .lock()
            .map_err(|_| lock_error())?
            .clone();
        let action = latest.map(|a| json!({"id":a["id"],"kind":a["kind"],"task_id":a["task_id"],"coordinator_attempt":a["coordinator_attempt"]}));
        let value = json!({"candidate_id":self.candidate["id"],"owner_pid":std::process::id(),"attempt":self.attempt.load(Ordering::Acquire),"coordinator":coordinator,"trace":{"action":action,"candidate":{"provider":self.candidate["provider"]},"active":self.active_action.lock().map_err(|_|lock_error())?.is_some()}});
        catalog::write_cache(
            &self.root.join(&self.id).join("web-session.json"),
            &serde_json::to_vec(&value).map_err(|_| lock_error())?,
        )
        .map_err(|_| error("session_unwritable", "실행 상태를 저장할 수 없습니다."))
    }
}

pub(crate) fn recorded_state(workflow: &Workflow, run: &str) -> Result<Value, Value> {
    let mut state = workflow.status(run).map_err(|e| e.to_value())?;
    if let Some(record) = recorded(workflow.root(), run) {
        state["coordinator"] = record["coordinator"].clone();
        state["coordinator_trace"] = record["trace"].clone();
        if let Some(error) = record["coordinator"].get("error") {
            state["attention"] = error.clone();
        }
    }
    Ok(state)
}

fn recorded(root: &Path, run: &str) -> Option<Value> {
    let path = root.join(run).join("web-session.json");
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 128 * 1024 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

struct ToolState {
    inventory: Value,
    selected: Vec<String>,
}

pub struct Workbench {
    workspace: PathBuf,
    tools: Mutex<ToolState>,
    preferences: PathBuf,
    judge: Option<Arc<ConfiguredJudge>>,
    current: Mutex<Option<Arc<Run>>>,
}

fn error(code: &str, message: &str) -> Value {
    json!({"code":code,"message":message})
}
fn lock_error() -> Value {
    error("workbench_busy", "잠시 후 다시 시도하세요.")
}
fn selection(inventory: &Value, value: &Value, nonempty: bool) -> Result<Vec<String>, Value> {
    let input = value
        .as_array()
        .filter(|list| list.len() <= 6 && (!nonempty || !list.is_empty()))
        .ok_or_else(|| error("invalid_tools", "사용할 AI 도구를 하나 이상 선택하세요."))?;
    let mut selected = Vec::new();
    for item in input {
        let id = item
            .as_str()
            .ok_or_else(|| error("invalid_tools", "도구 목록을 확인하세요."))?;
        if selected.iter().any(|s| s == id)
            || !inventory["tools"].as_array().is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| tool["id"] == id && (!nonempty || tool["selectable"] == true))
            })
        {
            return Err(error(
                "tool_unavailable",
                "선택한 도구를 실행할 수 없습니다. 도구를 다시 감지하고 선택을 확인하세요.",
            ));
        }
        selected.push(id.to_owned());
    }
    Ok(selected)
}
fn friendly(value: &Value) -> Value {
    let code = value["code"]
        .as_str()
        .or_else(|| value["error"]["code"].as_str())
        .unwrap_or("execution_failed");
    let code = if code.len() <= 100 && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        code
    } else {
        "execution_failed"
    };
    let message = value["message"].as_str().unwrap_or(match code {
        "coordinator_artifact_invalid" | "coordinator_artifact_missing" | "invalid_receipt" => "조율자가 실행 가능한 작업 계획을 반환하지 못했습니다. 다시 시도해 계획을 새로 만드세요.",
        "coordinator_execution_failed" => "조율자 CLI가 완료되지 않았습니다. 해당 CLI의 로그인과 사용 한도를 확인한 뒤 다시 시도하세요.",
        "coordinator_context_too_large" => "이 작업의 문맥이 조율자 입력 한도를 넘었습니다. 더 작은 업무로 나눠 시작하세요.",
        "provider_key_missing" | "judge_unavailable" => "판단기 연결을 확인한 뒤 다시 시도하세요.",
        _ => "실행을 이어가지 못했습니다. 표시된 오류 코드와 프로젝트 상태를 확인한 뒤 다시 시도하세요.",
    });
    error(code, message)
}

impl Workbench {
    pub fn new(workspace: &Path, judge: Option<Arc<ConfiguredJudge>>) -> Result<Self, Value> {
        let inventory = catalog::inventory(workspace, false).map_err(|e| e.to_value())?;
        Self::from_inventory(workspace, inventory, judge)
    }

    pub fn with_candidates(
        workspace: &Path,
        candidates: Vec<Value>,
        judge: Option<Arc<ConfiguredJudge>>,
    ) -> Result<Self, Value> {
        let tools = ["codex","claude","cursor","opencode","grok","antigravity"].iter().map(|provider| {
            let models: std::collections::BTreeSet<_> = candidates.iter().filter(|c|c["provider"]==*provider).filter_map(|c|c["model"].as_str()).collect();
            let selectable = candidates.iter().any(|c|c["provider"]==*provider && c["available"]==true);
            json!({"id":provider,"name":provider,"installed":!models.is_empty(),"selectable":selectable,"model_count":models.len(),"connection_mode":"cli","status":{"code":"unverified","label":"실행 시 인증 확인"},"session_capabilities":{"new_session":selectable,"resume":false,"attach":false}})
        }).collect::<Vec<_>>();
        Self::from_inventory(
            workspace,
            json!({"snapshot":{"candidates":candidates},"tools":tools}),
            judge,
        )
    }

    fn from_inventory(
        workspace: &Path,
        inventory: Value,
        judge: Option<Arc<ConfiguredJudge>>,
    ) -> Result<Self, Value> {
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|_| error("invalid_workspace", "존재하는 프로젝트 폴더를 선택하세요."))?;
        let preferences = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Switchbite/web-tools.json"))
            .unwrap_or_else(|| workspace.join(".switchbite/web-tools.json"));
        let selected = if preferences.exists() {
            // An unreadable saved choice never broadens the set of allowed tools.
            std::fs::read(&preferences)
                .ok()
                .filter(|bytes| bytes.len() <= 4096)
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|value| value["tools"].as_array().cloned())
                .map(|tools| {
                    tools
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|id| {
                            inventory["tools"]
                                .as_array()
                                .is_some_and(|rows| rows.iter().any(|row| row["id"] == *id))
                        })
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            inventory["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|tool| tool["selectable"] == true)
                .filter_map(|tool| tool["id"].as_str().map(str::to_owned))
                .collect()
        };
        Ok(Self {
            workspace,
            tools: Mutex::new(ToolState {
                inventory,
                selected,
            }),
            preferences,
            judge,
            current: Mutex::new(None),
        })
    }

    pub fn info(&self) -> Result<Value, Value> {
        let state = self.status()?;
        let current = if state["status"] == "idle" {
            Value::Null
        } else {
            state
        };
        let tools = self.tools.lock().map_err(|_| lock_error())?;
        let mut candidates = tools.inventory["snapshot"]["candidates"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        candidates.retain(|candidate| candidate["available"] == true);
        candidates.sort_by_key(|candidate| {
            let preferred = candidate["provider"] == "codex"
                && candidate["model"] == "gpt-6.1-terra"
                && candidate["effort"] == "high";
            let order = match candidate["provider"].as_str() {
                Some("codex") => 0,
                Some("claude") => 1,
                Some("opencode") => 2,
                Some("antigravity") => 3,
                _ => 4,
            };
            (!preferred, order)
        });
        Ok(
            json!({"workspace":self.workspace,"judge":self.judge.as_ref().map(|j|j.model()).unwrap_or("Offline"),"tools":tools.inventory["tools"],"selected_tools":tools.selected,"connection":{"mode":"native_cli_new_session","label":"기존 CLI 로그인으로 새 작업 세션 실행"},"coordinators":candidates.iter().map(|c| json!({"id":c["id"],"provider":c["provider"],"model":c["model"],"effort":c["effort"],"label":format!("{} / {} / {}",c["provider"].as_str().unwrap_or(""),c["model"].as_str().unwrap_or(""),c["effort"].as_str().unwrap_or(""))})).collect::<Vec<_>>(),"current":current}),
        )
    }

    pub fn select_tools(&self, body: &Value) -> Result<Value, Value> {
        if body
            .as_object()
            .is_none_or(|o| o.len() != 1 || !o.contains_key("tools"))
        {
            return Err(error("invalid_request", "사용할 도구 목록을 입력하세요."));
        }
        let mut state = self.tools.lock().map_err(|_| lock_error())?;
        let selected = selection(&state.inventory, &body["tools"], false)?;
        let bytes =
            serde_json::to_vec(&json!({"version":1,"tools":selected})).map_err(|_| lock_error())?;
        let parent = self.preferences.parent().ok_or_else(lock_error)?;
        std::fs::create_dir_all(parent).map_err(|_| {
            error(
                "preferences_unwritable",
                "도구 선택을 저장할 수 없습니다. 폴더 접근 권한을 확인하세요.",
            )
        })?;
        catalog::write_cache(&self.preferences, &bytes).map_err(|_| {
            error(
                "preferences_unwritable",
                "도구 선택을 저장할 수 없습니다. 폴더 접근 권한을 확인하세요.",
            )
        })?;
        state.selected = selected;
        drop(state);
        self.info()
    }

    pub fn refresh_tools(&self) -> Result<Value, Value> {
        let inventory = catalog::inventory(&self.workspace, true).map_err(|e| e.to_value())?;
        self.tools.lock().map_err(|_| lock_error())?.inventory = inventory;
        self.info()
    }

    pub fn start(&self, body: &Value) -> Result<Value, Value> {
        let object = body
            .as_object()
            .ok_or_else(|| error("invalid_request", "프로젝트와 업무 내용을 입력하세요."))?;
        if object.keys().any(|k| {
            ![
                "workspace",
                "goal",
                "coordinator_id",
                "tools",
                "max_parallel",
            ]
            .contains(&k.as_str())
        }) {
            return Err(error("invalid_request", "지원하지 않는 입력입니다."));
        }
        let max_parallel = match body.get("max_parallel") {
            None => 3,
            Some(value) => value
                .as_u64()
                .filter(|n| (1..=16).contains(n))
                .ok_or_else(|| {
                    error(
                        "invalid_max_parallel",
                        "동시 작업 수는 1부터 16까지 입력하세요.",
                    )
                })?,
        };
        let goal = body["goal"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 24000)
            .ok_or_else(|| {
                error(
                    "invalid_goal",
                    "업무 내용을 24,000 바이트 이내로 입력하세요.",
                )
            })?;
        let inventory = self.tools.lock().map_err(|_| lock_error())?;
        let chosen = body
            .get("tools")
            .cloned()
            .unwrap_or_else(|| json!(inventory.selected));
        let selected = selection(&inventory.inventory, &chosen, true)?;
        let candidate = inventory.inventory["snapshot"]["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == body["coordinator_id"])
            .filter(|c| {
                c["available"] == true
                    && c["provider"]
                        .as_str()
                        .is_some_and(|id| selected.iter().any(|tool| tool == id))
            })
            .cloned()
            .ok_or_else(|| {
                error(
                    "coordinator_unavailable",
                    "설치된 조율자 CLI와 모델을 선택하세요.",
                )
            })?;
        drop(inventory);
        let workspace = body["workspace"]
            .as_str()
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .and_then(|p| std::fs::canonicalize(p).ok())
            .filter(|p| p.is_dir())
            .ok_or_else(|| {
                error(
                    "invalid_workspace",
                    "존재하는 프로젝트의 절대 경로를 입력하세요.",
                )
            })?;
        if workspace.join("ARCHIVED.json").exists() {
            return Err(error(
                "workspace_archived",
                "보관된 프로젝트를 먼저 복원하세요.",
            ));
        }
        let mut current = self.current.lock().map_err(|_| lock_error())?;
        if let Some(run) = current.as_ref() {
            let state = Workflow::new(run.root.clone())
                .status(&run.id)
                .map_err(|e| e.to_value())?;
            if run.alive.load(Ordering::Acquire)
                || !["completed", "cancelled"].contains(&state["status"].as_str().unwrap_or(""))
            {
                return Err(error(
                    "run_active",
                    "현재 작업을 완료하거나 취소한 뒤 새 작업을 시작하세요.",
                ));
            }
        }
        let root = workspace.join(".switchbite/workflow");
        // Check the existing Git base before starting a run or paid CLI call.
        LocalHost::new(
            &workspace,
            &workspace.join(".switchbite/web-host"),
            &json!({}),
        )
        .map_err(|_| {
            error(
                "project_not_ready",
                "커밋이 있는 Git 프로젝트 폴더를 선택하세요.",
            )
        })?;
        let state = Workflow::new(root.clone()).start(&json!({"workspace":workspace,"goal":goal,"catalog":{"providers":selected},"max_parallel":max_parallel,"host":{"timeout_seconds":900}})).map_err(|e| e.to_value())?;
        let run = Arc::new(Run {
            root,
            id: state["run_id"].as_str().unwrap().to_owned(),
            candidate,
            coordinator: Mutex::new(json!({"status":"planning"})),
            active_action: Mutex::new(None),
            latest_coordinator_action: Mutex::new(None),
            alive: AtomicBool::new(false),
            attempt: AtomicU64::new(0),
            tools: selected,
        });
        *current = Some(run.clone());
        drop(current);
        self.spawn(run);
        self.status()
    }

    pub fn status(&self) -> Result<Value, Value> {
        let current = self.current.lock().map_err(|_| lock_error())?.clone();
        let Some(run) = current else {
            return Ok(
                json!({"status":"idle","tasks":[],"actions":[],"progress":{"done":0,"total":0},"event_count":0}),
            );
        };
        let workflow = Workflow::new(run.root.clone());
        run.checkpoint()?;
        let mut state = workflow.status(&run.id).map_err(|e| e.to_value())?;
        let request = workflow.request(&run.id).map_err(|e| e.to_value())?;
        let mut coordinator = run.coordinator.lock().map_err(|_| lock_error())?.clone();
        for field in ["provider", "model", "effort"] {
            coordinator[field] = run.candidate[field].clone();
        }
        state["coordinator"] = coordinator.clone();
        state["workspace"] = request["workspace"].clone();
        state["goal"] = request["goal"].clone();
        state["max_parallel"] = request["max_parallel"].clone();
        state["tools"] = json!(run.tools);
        state["connection"] = json!({"mode":"native_cli_new_session","label":"기존 CLI 로그인으로 새 작업 세션 실행"});
        if let Some(detail) = coordinator.get("error") {
            state["attention"] = detail.clone();
        }
        if state["status"] == "completed"
            && let Some(task) = state["tasks"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .find(|t| {
                    t["status"] == "done" && t["prepared"]["integration_workspace"].is_string()
                })
        {
            let commit = task["prepared"]["integration_workspace"]
                .as_str()
                .and_then(|path| {
                    Command::new("git")
                        .current_dir(path)
                        .args(["rev-parse", "HEAD"])
                        .output()
                        .ok()
                })
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|head| head.trim().to_owned())
                .filter(|head| {
                    (head.len() == 40 || head.len() == 64)
                        && head.bytes().all(|b| b.is_ascii_hexdigit())
                });
            state["result"] = json!({"workspace":task["prepared"]["integration_workspace"],"branch":task["prepared"]["integration_branch"],"commit":commit});
        }
        Ok(state)
    }

    /// Reattach the web coordinator after its previous server has stopped.
    pub fn resume_existing(&self, id: &str, candidate_id: Option<&str>) -> Result<Value, Value> {
        let root = self.workspace.join(".switchbite/workflow");
        let workflow = Workflow::new(root.clone());
        let state = workflow.status(id).map_err(|e| e.to_value())?;
        let request = workflow.request(id).map_err(|e| e.to_value())?;
        let saved = recorded(&root, id).unwrap_or(Value::Null);
        let candidate_id = candidate_id
            .or_else(|| saved["candidate_id"].as_str())
            .ok_or_else(|| {
                error(
                    "coordinator_required",
                    "기존 실행의 조율자 모델을 지정하세요.",
                )
            })?;
        let allowed = request["catalog"]["providers"]
            .as_array()
            .ok_or_else(|| error("invalid_request", "웹에서 시작한 실행을 선택하세요."))?;
        let tools = self.tools.lock().map_err(|_| lock_error())?;
        let candidate = tools.inventory["snapshot"]["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| {
                c["id"] == candidate_id
                    && c["available"] == true
                    && allowed.contains(&c["provider"])
            })
            .cloned()
            .ok_or_else(|| {
                error(
                    "coordinator_unavailable",
                    "이 실행에서 허용한 조율자 모델을 선택하세요.",
                )
            })?;
        drop(tools);
        let mut current = self.current.lock().map_err(|_| lock_error())?;
        if current.is_some() {
            return Err(error("run_active", "현재 실행을 먼저 확인하세요."));
        }
        let run = Arc::new(Run {
            root,
            id: id.to_owned(),
            candidate,
            coordinator: Mutex::new(json!({"status":"coordinating"})),
            active_action: Mutex::new(None),
            latest_coordinator_action: Mutex::new(
                saved["trace"]["action"]
                    .as_object()
                    .map(|_| saved["trace"]["action"].clone()),
            ),
            alive: AtomicBool::new(false),
            attempt: AtomicU64::new(
                crate::agent::now_ms()
                    .max(saved["attempt"].as_u64().unwrap_or(0).saturating_add(1)),
            ),
            tools: allowed
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        });
        run.checkpoint()?;
        *current = Some(run.clone());
        drop(current);
        if state["status"] == "running" {
            self.spawn(run);
        }
        self.status()
    }

    pub fn activity(&self, selected: Option<&str>) -> Result<Value, Value> {
        let current = self.current.lock().map_err(|_| lock_error())?.clone();
        let Some(run) = current else {
            return Ok(
                json!({"run_id":null,"observed_at_ms":crate::agent::now_ms(),"health":{"status":"idle","label":"새 작업 대기"},"streams":[],"selected_stream":null}),
            );
        };
        let state = self.status()?;
        let coordinator = run.latest_coordinator_action.lock().map_err(|_|lock_error())?.clone().map(|action|json!({"action":action,"candidate":run.candidate,"active":run.active_action.lock().is_ok_and(|value|value.is_some())}));
        crate::activity::snapshot(
            &run.root,
            &run.id,
            &state,
            coordinator.as_ref(),
            selected,
            crate::agent::now_ms(),
        )
    }

    /// Interrupt only this run's current native worker; normal receipts drive repair.
    pub fn retry_task(&self, task_id: &str) -> Result<Value, Value> {
        let run = self
            .current
            .lock()
            .map_err(|_| lock_error())?
            .clone()
            .ok_or_else(|| error("run_missing", "먼저 업무를 시작하세요."))?;
        let workflow = Workflow::new(run.root.clone());
        let state = workflow.status(&run.id).map_err(|e| e.to_value())?;
        if state["status"] != "running" {
            return Err(error(
                "run_not_running",
                "진행 중인 작업에서 사용할 수 있습니다. 일시정지 상태라면 먼저 재개하세요.",
            ));
        }
        let task = state["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|task| task["id"] == task_id)
            .ok_or_else(|| error("task_missing", "작업을 다시 선택하세요."))?;
        let request = workflow.request(&run.id).map_err(|e| e.to_value())?;
        if task["repair_rounds"].as_u64().unwrap_or(0)
            >= request["repair_rounds"].as_u64().unwrap_or(2)
        {
            return Err(error(
                "repair_limit",
                "이 작업의 복구 횟수에 도달했습니다. 결과를 확인하고 새 업무로 이어가세요.",
            ));
        }
        let launch = state["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|action| action["kind"] == "launch" && action["task_id"] == task_id);
        let execution = launch
            .and_then(|action| action["id"].as_str())
            .or_else(|| task["execution_id"].as_str())
            .filter(|_| {
                ["launching", "prepared", "running", "unknown"]
                    .contains(&task["status"].as_str().unwrap_or(""))
            })
            .ok_or_else(|| error("task_not_running", "현재 실행 중인 작업자를 선택하세요."))?;
        let workspace = request["workspace"].as_str().ok_or_else(lock_error)?;
        let host = LocalHost::new(
            Path::new(workspace),
            &run.root.join(&run.id).join("host"),
            &request.get("host").cloned().unwrap_or(json!({})),
        )
        .map_err(|e| e.to_value())?;
        let result=host.perform(&json!({"id":format!("web-retry-{execution}"),"kind":"inspect","run_id":run.id,"payload":{"execution_id":execution,"operation":"cancel"}})).map_err(|e|e.to_value())?;
        if result["status"] == "completed" {
            return Err(error(
                "task_already_finished",
                "이 작업자는 이미 완료됐습니다. 최신 상태를 확인하세요.",
            ));
        }
        self.spawn(run);
        self.status()
    }

    pub fn set_parallelism(&self, limit: u64) -> Result<Value, Value> {
        let run = self
            .current
            .lock()
            .map_err(|_| lock_error())?
            .clone()
            .ok_or_else(|| error("run_missing", "먼저 업무를 시작하세요."))?;
        Workflow::new(run.root.clone())
            .set_parallelism(&run.id, limit)
            .map_err(|e| e.to_value())?;
        self.status()
    }

    pub fn control(&self, action: &str) -> Result<Value, Value> {
        let run = self
            .current
            .lock()
            .map_err(|_| lock_error())?
            .clone()
            .ok_or_else(|| error("run_missing", "먼저 업무를 시작하세요."))?;
        let workflow = Workflow::new(run.root.clone());
        let state = workflow.status(&run.id).map_err(|e| e.to_value())?;
        if action == "resume" && !run.alive.load(Ordering::Acquire) && state["status"] == "running"
        {
            run.attempt.fetch_add(1, Ordering::AcqRel);
            *run.coordinator.lock().map_err(|_| lock_error())? = json!({"status":"planning"});
            if workflow
                .request(&run.id)
                .map_err(|e| e.to_value())?
                .get("catalog")
                .is_some()
            {
                workflow
                    .refresh_catalog(&run.id)
                    .map_err(|e| e.to_value())?;
            }
            self.spawn(run);
            return self.status();
        }
        if action == "resume"
            && state["status"] == "paused"
            && workflow
                .request(&run.id)
                .map_err(|e| e.to_value())?
                .get("catalog")
                .is_some()
        {
            workflow
                .refresh_catalog(&run.id)
                .map_err(|e| e.to_value())?;
        }
        workflow
            .control(&run.id, action)
            .map_err(|e| e.to_value())?;
        if action == "cancel"
            && let Some(active) = run.active_action.lock().map_err(|_| lock_error())?.clone()
        {
            web_coordinator::cancel(&run.root, &run.id, &active)?;
        }
        if action == "resume" || action == "cancel" {
            self.spawn(run);
        }
        self.status()
    }

    fn spawn(&self, run: Arc<Run>) {
        if run.alive.swap(true, Ordering::AcqRel) {
            return;
        }
        let judge = self.judge.clone();
        thread::spawn(move || {
            if let Err(detail) = drive(&run, judge)
                && let Ok(mut state) = run.coordinator.lock()
            {
                *state = json!({"status":"needs_attention","error":friendly(&detail)});
            }
            run.alive.store(false, Ordering::Release);
            let _ = run.checkpoint();
        });
    }
}

fn drive(run: &Run, judge: Option<Arc<ConfiguredJudge>>) -> Result<(), Value> {
    let workflow = Workflow::new(run.root.clone());
    loop {
        let state = workflow_runner::advance(&run.root, &run.id, judge.clone())
            .map_err(|e| e.to_value())?;
        if let Some(detail) = state.get("attention") {
            return Err(detail.clone());
        }
        let active = state["driver"]["active_actions"].as_u64().unwrap_or(0);
        if (state["status"] == "completed" || state["status"] == "cancelled") && active == 0 {
            *run.coordinator.lock().map_err(|_| lock_error())? = json!({"status":state["status"]});
            return Ok(());
        }
        if state["status"] == "running" {
            if let Some(mut action) = state["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|a| a["kind"] == "plan" || a["kind"] == "repair")
                .cloned()
            {
                action["coordinator_attempt"] = json!(run.attempt.load(Ordering::Acquire));
                let id = action["id"].as_str().ok_or_else(lock_error)?;
                if workflow.claim(&run.id, id).map_err(|e| e.to_value())? {
                    *run.active_action.lock().map_err(|_| lock_error())? = Some(action.clone());
                    *run.latest_coordinator_action
                        .lock()
                        .map_err(|_| lock_error())? = Some(action.clone());
                    *run.coordinator.lock().map_err(|_| lock_error())? =
                        json!({"status":if action["kind"] == "plan" {"planning"}else{"repairing"}});
                    run.checkpoint()?;
                    // Recheck after publishing the cancellation target.
                    let result = if workflow.status(&run.id).map_err(|e| e.to_value())?["status"]
                        == "cancelled"
                    {
                        None
                    } else {
                        Some(web_coordinator::fulfill(
                            &run.root,
                            &run.id,
                            &action,
                            &run.candidate,
                        ))
                    };
                    *run.active_action.lock().map_err(|_| lock_error())? = None;
                    loop {
                        let status = workflow.status(&run.id).map_err(|e| e.to_value())?;
                        if status["status"] != "paused" {
                            break;
                        }
                        thread::sleep(Duration::from_millis(250));
                    }
                    if workflow.status(&run.id).map_err(|e| e.to_value())?["status"] != "cancelled"
                    {
                        let result = result.ok_or_else(lock_error)??;
                        workflow.observe(&run.id, &json!({"action_id":id,"event_id":format!("web:{id}"),"result":result})).map_err(|e| e.to_value())?;
                        *run.coordinator.lock().map_err(|_| lock_error())? =
                            json!({"status":"coordinating"});
                    }
                }
            } else if active == 0
                && state["actions"].as_array().is_some_and(|a| {
                    a.iter()
                        .any(|a| a["kind"] == "refresh_pool" || a["kind"] == "review")
                })
            {
                return Err(error(
                    "input_needed",
                    "사용 가능한 모델이나 작업 결과를 확인하세요.",
                ));
            } else if active == 0
                && state["actions"].as_array().is_some_and(Vec::is_empty)
                && state["tasks"].as_array().is_some_and(|t| {
                    t.iter()
                        .any(|t| t["status"] == "held" || t["status"] == "blocked")
                })
            {
                return Err(error(
                    "routing_held",
                    "배정 조건이나 사용 가능한 모델을 확인하세요.",
                ));
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
}
