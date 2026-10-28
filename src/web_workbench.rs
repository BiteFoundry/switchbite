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
    coordinator_feedback: Mutex<Option<Value>>,
    alive: AtomicBool,
    attempt: AtomicU64,
    tools: Vec<String>,
}

static CHECKPOINT_WRITE: Mutex<()> = Mutex::new(());
static CONTROL: Mutex<()> = Mutex::new(());

impl Run {
    fn checkpoint(&self) -> Result<(), Value> {
        let _write = CHECKPOINT_WRITE.lock().map_err(|_| lock_error())?;
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
        let feedback = self
            .coordinator_feedback
            .lock()
            .map_err(|_| lock_error())?
            .clone();
        let value = json!({"candidate_id":self.candidate["id"],"owner_pid":std::process::id(),"attempt":self.attempt.load(Ordering::Acquire),"coordinator":coordinator,"coordinator_feedback":feedback,"trace":{"action":action,"candidate":{"provider":self.candidate["provider"]},"active":self.active_action.lock().map_err(|_|lock_error())?.is_some()}});
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
    if !meta.is_file() || meta.len() > 256 * 1024 {
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
    startup_error: Mutex<Option<Value>>,
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
        "coordinator_plan_invalid" | "coordinator_artifact_invalid" | "invalid_receipt" => "계획 형식을 확인하지 못했습니다. 표시된 항목을 수정해 기존 계획으로 이어갑니다.",
        "coordinator_replan_invalid" => "계획 변경을 적용하지 못했습니다. 표시된 항목을 확인하고 조율자에게 보완 내용을 전달하세요.",
        "coordinator_artifact_missing" => "CLI가 종료됐지만 계획 결과를 찾지 못했습니다. 조율자 출력을 확인하세요.",
        "coordinator_review_invalid" | "coordinator_review_candidate_invalid" | "coordinator_review_snapshot_invalid" => "조율자가 유효한 모델 배정을 반환하지 못했습니다. 다시 시도하세요.",
        "coordinator_execution_failed" => "조율자 CLI 실행이 완료되지 않았습니다. 실행 출력을 확인하세요.",
        "coordinator_context_too_large" => "이 작업의 문맥이 조율자 입력 한도를 넘었습니다. 더 작은 업무로 나눠 시작하세요.",
        "provider_key_missing" | "judge_unavailable" => "판단기 연결을 확인한 뒤 다시 시도하세요.",
        _ => "실행을 이어가지 못했습니다. 표시된 오류 코드와 프로젝트 상태를 확인한 뒤 다시 시도하세요.",
    });
    let stage = value["stage"].as_str().unwrap_or(match code {
        "coordinator_plan_invalid" | "invalid_receipt" => "plan_validation",
        "coordinator_artifact_invalid" | "coordinator_artifact_missing" => "result_collection",
        "coordinator_execution_failed"
        | "timeout"
        | "process_failed"
        | "provider_receipt_invalid" => "cli_execution",
        "routing_held" | "input_needed" | "judge_unavailable" | "provider_key_missing" => "routing",
        _ => "workflow",
    });
    let mut result = error(code, message);
    for (key, text) in std::iter::once(("stage", Some(stage)))
        .chain(["node_id", "field", "expected"].map(|key| (key, value[key].as_str())))
    {
        if let Some(text) = text {
            result[key] = json!(text.chars().take(1000).collect::<String>());
        }
    }
    result
}

// Keep the previous plan private; only the concrete diagnostic reaches browser status.
fn coordinator_failure(run: &Run, action: &Value, detail: Value) -> Result<bool, Value> {
    let mut saved = run.coordinator_feedback.lock().map_err(|_| lock_error())?;
    let previous = saved
        .as_ref()
        .filter(|old| old["action_id"] == action["id"]);
    let retries = previous
        .and_then(|old| old["automatic_retries"].as_u64())
        .unwrap_or(0);
    let prior = detail
        .get("prior_result")
        .cloned()
        .or_else(|| previous.and_then(|old| old.get("prior_result").cloned()));
    let retry = ["coordinator_plan_invalid", "coordinator_replan_invalid"]
        .contains(&detail["code"].as_str().unwrap_or(""))
        && prior.is_some()
        && retries == 0;
    *saved = Some(
        json!({"action_id":action["id"],"error":friendly(&detail),"prior_result":prior,"automatic_retries":retries + u64::from(retry)}),
    );
    drop(saved);
    if retry {
        run.attempt.fetch_add(1, Ordering::AcqRel);
        *run.coordinator.lock().map_err(|_| lock_error())? = json!({"status":"repairing_plan","stage":"plan_validation","recovery":friendly(&detail)});
        run.checkpoint()?;
    }
    Ok(retry)
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
            startup_error: Mutex::new(None),
        })
    }

    pub fn info(&self) -> Result<Value, Value> {
        let state = self.status()?;
        let current = if state["status"] == "idle" && state["attention"].is_null() {
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
        if let Some(handoff) = crate::runtime_bundle::handoff_new_run(body, &self.workspace)
            .map_err(|_| {
                error(
                    "runtime_update_failed",
                    "엔진 업데이트를 완료하지 못했습니다. 다시 시작해 주세요.",
                )
            })?
        {
            return Ok(handoff);
        }
        let state = Workflow::new(root.clone()).start(&json!({"workspace":workspace,"goal":goal,"catalog":{"providers":selected},"max_parallel":max_parallel,"host":{}})).map_err(|e| e.to_value())?;
        let run = Arc::new(Run {
            root,
            id: state["run_id"].as_str().unwrap().to_owned(),
            candidate,
            coordinator: Mutex::new(json!({"status":"planning"})),
            active_action: Mutex::new(None),
            latest_coordinator_action: Mutex::new(None),
            coordinator_feedback: Mutex::new(None),
            alive: AtomicBool::new(false),
            attempt: AtomicU64::new(0),
            tools: selected,
        });
        *current = Some(run.clone());
        *self.startup_error.lock().map_err(|_| lock_error())? = None;
        drop(current);
        self.spawn(run);
        self.status()
    }

    pub fn status(&self) -> Result<Value, Value> {
        let current = self.current.lock().map_err(|_| lock_error())?.clone();
        let Some(run) = current else {
            let mut state = json!({"status":"idle","tasks":[],"actions":[],"progress":{"done":0,"total":0},"event_count":0});
            if let Some(detail) = self.startup_error.lock().map_err(|_| lock_error())?.clone() {
                state["attention"] = detail["error"].clone();
                state["startup_submission"] = detail["submission"].clone();
            }
            return Ok(state);
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
        state["connection"] = if request["host"]["backend"]["type"] == "stackbite" {
            json!({"mode":"stackbite_engine","label":"StackBite 엔진 · 로그인된 CLI로 실행"})
        } else {
            json!({"mode":"native_cli_new_session","label":"기존 CLI 로그인으로 새 작업 세션 실행"})
        };
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

    pub(crate) fn record_start_error(&self, error: Value, body: &Value) {
        let bounded = |key: &str, max: usize| {
            body[key]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(max)
                .filter(|c| !c.is_control() || *c == '\n')
                .collect::<String>()
        };
        let tools: Vec<String> = body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .take(6)
            .filter_map(|v| v.as_str())
            .filter(|v| v.len() <= 64)
            .map(str::to_owned)
            .collect();
        let submission = json!({"workspace":bounded("workspace",4096),"goal":bounded("goal",24000),"coordinator_id":bounded("coordinator_id",256),"tools":tools,"max_parallel":body["max_parallel"].as_u64().filter(|n| (1..=16).contains(n)).unwrap_or(3)});
        if let Ok(mut slot) = self.startup_error.lock() {
            *slot = Some(json!({"error":error,"submission":submission}));
        }
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
        if state["status"] == "running"
            && state["actions"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|a| a["kind"] == "refresh_pool"))
        {
            workflow.refresh_catalog(id).map_err(|e| e.to_value())?;
        }
        let run = Arc::new(Run {
            root,
            id: id.to_owned(),
            candidate,
            coordinator: Mutex::new(if saved["coordinator"]["error"].is_object() {
                let mut coordinator = saved["coordinator"].clone();
                coordinator["error"] = friendly(&coordinator["error"]);
                coordinator
            } else {
                json!({"status":"coordinating"})
            }),
            active_action: Mutex::new(None),
            latest_coordinator_action: Mutex::new(
                saved["trace"]["action"]
                    .as_object()
                    .map(|_| saved["trace"]["action"].clone()),
            ),
            coordinator_feedback: Mutex::new(
                saved
                    .get("coordinator_feedback")
                    .filter(|v| !v.is_null())
                    .cloned(),
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
        if state["status"] == "running" && !saved["coordinator"]["error"].is_object() {
            workflow
                .clear_legacy_web_deadline(id)
                .map_err(|e| e.to_value())?;
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

    pub fn report(&self, body: &Value) -> Result<Value, Value> {
        let _control = CONTROL.lock().map_err(|_| lock_error())?;
        if body.as_object().is_none_or(|obj| {
            obj.keys()
                .any(|key| !["id", "task_id", "message"].contains(&key.as_str()))
        }) {
            return Err(error("invalid_feedback", "전달할 내용을 입력하세요."));
        }
        let run = self
            .current
            .lock()
            .map_err(|_| lock_error())?
            .clone()
            .ok_or_else(|| error("run_missing", "먼저 업무를 시작하세요."))?;
        let mut report = json!({"id":body["id"],"source":"user","kind":"request_change","message":body["message"]});
        if let Some(task) = body.get("task_id") {
            report["task_id"] = task.clone();
        }
        let workflow = Workflow::new(run.root.clone());
        let state = workflow.report(&run.id, &report).map_err(|e| {
            let mut detail = e.to_value();
            detail["message"] =
                json!("실행 중인 작업을 선택하고 내용을 8,000바이트 이내로 입력하세요.");
            detail
        })?;
        if state["status"] == "running" && !run.alive.load(Ordering::Acquire) {
            run.attempt.fetch_add(1, Ordering::AcqRel);
            *run.coordinator.lock().map_err(|_| lock_error())? = json!({"status":"replanning"});
            self.spawn(run);
        }
        self.status()
    }

    pub fn control(&self, action: &str) -> Result<Value, Value> {
        let _control = CONTROL.lock().map_err(|_| lock_error())?;
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
            workflow
                .clear_legacy_web_deadline(&run.id)
                .map_err(|e| e.to_value())?;
            run.attempt.fetch_add(1, Ordering::AcqRel);
            if let Some(feedback) = run
                .coordinator_feedback
                .lock()
                .map_err(|_| lock_error())?
                .as_mut()
            {
                feedback["automatic_retries"] = json!(0);
            }
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

fn has_new_feedback(state: &Value, seen: &mut std::collections::BTreeSet<String>) -> bool {
    let mut added = false;
    for report in state["feedback"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|report| report["status"] == "pending")
    {
        if let Some(id) = report["id"].as_str() {
            added |= seen.insert(id.to_owned());
        }
    }
    added
}

fn drive(run: &Run, judge: Option<Arc<ConfiguredJudge>>) -> Result<(), Value> {
    let workflow = Workflow::new(run.root.clone());
    let mut coordinator_job: Option<(Value, thread::JoinHandle<Result<Value, Value>>)> = None;
    let mut attention = None;
    let mut feedback_seen = std::collections::BTreeSet::new();
    loop {
        let state =
            workflow_runner::advance_inner(&run.root, &run.id, judge.clone(), attention.is_some())
                .map_err(|e| e.to_value())?;
        if attention.is_none() {
            attention = state.get("attention").cloned();
        }
        if has_new_feedback(&state, &mut feedback_seen) && attention.take().is_some() {
            run.attempt.fetch_add(1, Ordering::AcqRel);
        }
        let active = state["driver"]["active_actions"].as_u64().unwrap_or(0);
        if coordinator_job
            .as_ref()
            .is_some_and(|(_, job)| job.is_finished())
            && state["status"] != "paused"
        {
            let (action, job) = coordinator_job.take().ok_or_else(lock_error)?;
            *run.active_action.lock().map_err(|_| lock_error())? = None;
            if state["status"] != "cancelled" {
                let id = action["id"].as_str().ok_or_else(lock_error)?;
                let result = job
                    .join()
                    .map_err(|_| error("coordinator_panic", "조율자 작업이 중단되었습니다."))
                    .and_then(|result| result);
                match result {
                    Ok(result) => {
                        let accepted = workflow.observe(
                            &run.id,
                            &json!({"action_id":id,"event_id":format!("web:{id}"),"result":result}),
                        );
                        match accepted {
                            Ok(_) => {
                                *run.coordinator_feedback.lock().map_err(|_| lock_error())? = None;
                                *run.coordinator.lock().map_err(|_| lock_error())? =
                                    json!({"status":"coordinating"});
                            }
                            Err(error) => {
                                let detail = if action["kind"] == "plan"
                                    || action["kind"] == "replan"
                                {
                                    json!({"code":if action["kind"] == "replan" {"coordinator_replan_invalid"} else {"coordinator_plan_invalid"},"stage":"plan_validation","field":"plan","expected":"valid workflow DAG preserving completed and active tasks","prior_result":result,"cause":error.to_value()})
                                } else {
                                    error.to_value()
                                };
                                if !coordinator_failure(run, &action, detail.clone())?
                                    && attention.is_none()
                                {
                                    attention = Some(detail);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if !coordinator_failure(run, &action, error.clone())? && attention.is_none()
                        {
                            attention = Some(error);
                        }
                    }
                }
            } else {
                let _ = job.join();
            }
            continue;
        }
        if coordinator_job.is_none()
            && active == 0
            && state["status"] != "cancelled"
            && let Some(detail) = attention.take()
        {
            return Err(detail);
        }
        if (state["status"] == "completed" || state["status"] == "cancelled")
            && active == 0
            && coordinator_job.is_none()
        {
            *run.coordinator.lock().map_err(|_| lock_error())? = json!({"status":state["status"]});
            return Ok(());
        }
        if state["status"] == "running" && coordinator_job.is_none() && attention.is_none() {
            if let Some(mut action) = state["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| {
                    ["replan", "plan", "repair", "review_route"]
                        .iter()
                        .any(|kind| a["kind"] == *kind)
                })
                .min_by_key(|action| if action["kind"] == "replan" { 0 } else { 1 })
                .cloned()
            {
                action["coordinator_attempt"] = json!(run.attempt.load(Ordering::Acquire));
                if let Some(feedback) = run
                    .coordinator_feedback
                    .lock()
                    .map_err(|_| lock_error())?
                    .as_ref()
                    && feedback["action_id"] == action["id"]
                {
                    action["payload"]["coordinator_feedback"] = feedback.clone();
                }
                let id = action["id"].as_str().ok_or_else(lock_error)?;
                if workflow.claim(&run.id, id).map_err(|e| e.to_value())? {
                    *run.active_action.lock().map_err(|_| lock_error())? = Some(action.clone());
                    *run.latest_coordinator_action
                        .lock()
                        .map_err(|_| lock_error())? = Some(action.clone());
                    *run.coordinator.lock().map_err(|_| lock_error())? = json!({"status":if action["kind"] == "replan" {"replanning"} else if action["kind"] == "plan" {"planning"} else if action["kind"] == "review_route" {"routing"} else {"repairing"}});
                    run.checkpoint()?;
                    // Recheck after publishing the cancellation target.
                    if workflow.status(&run.id).map_err(|e| e.to_value())?["status"] != "cancelled"
                    {
                        let root = run.root.clone();
                        let run_id = run.id.clone();
                        let candidate = run.candidate.clone();
                        let job_action = action.clone();
                        coordinator_job = Some((
                            action,
                            thread::spawn(move || {
                                web_coordinator::fulfill(&root, &run_id, &job_action, &candidate)
                            }),
                        ));
                    } else {
                        *run.active_action.lock().map_err(|_| lock_error())? = None;
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

#[cfg(test)]
mod parallel_tests {
    use super::*;
    use std::{
        fs, thread,
        time::{Duration, Instant},
    };

    #[test]
    fn rejected_replan_is_not_retried_forever_by_its_own_pending_report() {
        let mut seen = std::collections::BTreeSet::new();
        let state =
            json!({"actions":[{"kind":"replan"}],"feedback":[{"id":"one","status":"pending"}]});
        assert!(has_new_feedback(&state, &mut seen));
        assert!(!has_new_feedback(&state, &mut seen));
        let newer =
            json!({"feedback":[{"id":"one","status":"pending"},{"id":"two","status":"pending"}]});
        assert!(has_new_feedback(&newer, &mut seen));
        assert!(!has_new_feedback(&newer, &mut seen));
    }

    #[test]
    fn plan_retry_keeps_existing_result_and_stops_after_one_automatic_repair() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("test-run")).unwrap();
        let run = Run {
            root: tmp.path().to_owned(),
            id: "test-run".into(),
            candidate: json!({"id":"coordinator"}),
            coordinator: Mutex::new(json!({})),
            active_action: Mutex::new(None),
            latest_coordinator_action: Mutex::new(None),
            coordinator_feedback: Mutex::new(None),
            alive: AtomicBool::new(true),
            attempt: AtomicU64::new(0),
            tools: vec![],
        };
        let action = json!({"id":"plan-1","kind":"plan"});
        let prior = json!({"plan":{"nodes":[{"id":"audit","objective":"Keep this work"}]}});
        let detail = json!({"code":"coordinator_plan_invalid","stage":"plan_validation","node_id":"audit","field":"depends_on","expected":"existing task IDs","prior_result":prior});
        assert!(coordinator_failure(&run, &action, detail.clone()).unwrap());
        assert_eq!(run.attempt.load(Ordering::Acquire), 1);
        let saved = recorded(tmp.path(), "test-run").unwrap();
        assert_eq!(saved["coordinator_feedback"]["prior_result"], prior);
        assert_eq!(
            saved["coordinator_feedback"]["error"]["field"],
            "depends_on"
        );
        assert_eq!(run.coordinator.lock().unwrap()["status"], "repairing_plan");
        assert!(friendly(&detail).get("prior_result").is_none());
        assert!(!coordinator_failure(&run, &action, detail).unwrap());
        assert_eq!(run.attempt.load(Ordering::Acquire), 1);
        assert_eq!(
            run.coordinator_feedback.lock().unwrap().as_ref().unwrap()["automatic_retries"],
            1
        );
    }

    #[test]
    fn failed_initial_submission_survives_idle_info() {
        let tmp = tempfile::tempdir().unwrap();
        let app = Workbench {
            workspace: tmp.path().to_path_buf(),
            tools: Mutex::new(ToolState {
                inventory: json!({"tools":[],"snapshot":{"candidates":[]}}),
                selected: vec![],
            }),
            preferences: tmp.path().join("prefs.json"),
            judge: None,
            current: Mutex::new(None),
            startup_error: Mutex::new(None),
        };
        let body = json!({"workspace":"C:/project","goal":"do the work","tools":["codex"],"coordinator_id":"candidate","max_parallel":4,"secret":"not copied"});
        app.record_start_error(error("startup_failed", "Start failed"), &body);
        let state = app.info().unwrap()["current"].clone();
        assert_eq!(state["status"], "idle");
        assert_eq!(state["attention"]["code"], "startup_failed");
        assert_eq!(
            state["startup_submission"],
            json!({"workspace":"C:/project","goal":"do the work","tools":["codex"],"coordinator_id":"candidate","max_parallel":4})
        );
    }

    #[test]
    fn coordinator_review_does_not_block_independent_worker() {
        coordinator_overlap(false);
    }

    #[test]
    fn coordinator_failure_drains_independent_worker() {
        coordinator_overlap(true);
    }

    fn coordinator_overlap(fail_coordinator: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@localhost",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        ] {
            assert!(
                Command::new("git")
                    .current_dir(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let source = tmp.path().join("fake.rs");
        let exe = tmp
            .path()
            .join(if cfg!(windows) { "fake.exe" } else { "fake" });
        let coordinator_started = tmp.path().join("coordinator-started");
        let worker_started = tmp.path().join("worker-started");
        let release = tmp.path().join("release");
        fs::write(&source, format!(r##"use std::{{fs,thread,time::Duration}};
fn main() {{
 let args:Vec<String>=std::env::args().collect();
 let mut input=String::new();
 let prompt=if args.last().map(String::as_str)==Some("-") {{ use std::io::Read; std::io::stdin().read_to_string(&mut input).unwrap(); input }} else {{args.join(" ")}};
 println!("{{{{\"type\":\"thread.started\",\"thread_id\":\"test\"}}}}"); println!("{{{{\"type\":\"turn.started\"}}}}");
 if prompt.contains("Review the proposed worker assignment") {{
  fs::write({coordinator_started:?}, "started").unwrap();
  if {fail_coordinator} {{
   while !std::path::Path::new({worker_started:?}).exists() {{thread::sleep(Duration::from_millis(20));}}
   std::process::exit(1);
  }}
  while !std::path::Path::new({release:?}).exists() {{thread::sleep(Duration::from_millis(20));}}
  fs::write("switchbite-observation.json",r#"{{"status":"ready","candidate_id":"worker","reason_codes":["coordinator_review"],"summary":"Eligible"}}"#).unwrap();
 }} else {{
  fs::write({worker_started:?}, "started").unwrap();
  if {fail_coordinator} {{ while !std::path::Path::new({release:?}).exists() {{thread::sleep(Duration::from_millis(20));}} }}
  fs::write("two.txt","done").unwrap();
 }}
 println!("{{{{\"type\":\"turn.completed\"}}}}");
}}"##, coordinator_started=coordinator_started.to_str().unwrap(), worker_started=worker_started.to_str().unwrap(), release=release.to_str().unwrap())).unwrap();
        assert!(
            Command::new("rustc")
                .arg(&source)
                .arg("-o")
                .arg(&exe)
                .status()
                .unwrap()
                .success()
        );
        let candidate = json!({"id":"worker","description":"worker","capabilities":[],"available":true,"provider":"codex","executable":exe,"model":"test","effort":"high"});
        let root = tmp.path().join("state");
        let workflow = Workflow::new(root.clone());
        let state = workflow.start(&json!({"workspace":repo,"goal":"parallel test","pool":[candidate],"max_parallel":2,"plan":{"nodes":[
            {"id":"one","objective":"one","role":"implementation","depends_on":[],"acceptance":["done"],"write_paths":["one.txt"]},
            {"id":"two","objective":"two","role":"implementation","depends_on":[],"acceptance":["done"],"write_paths":["two.txt"],"routing":{"provider":"codex","model":"test","effort":"high"}}
        ]}})).unwrap();
        let id = state["run_id"].as_str().unwrap().to_owned();
        let route = state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == "route" && a["task_id"] == "one")
            .unwrap();
        workflow.observe(&id, &json!({"action_id":route["id"],"event_id":"held","result":{"status":"hold","reason_codes":["low_confidence"],"next_action":"request_review"}})).unwrap();
        let run = Arc::new(Run {
            root,
            id: id.clone(),
            candidate,
            coordinator: Mutex::new(json!({})),
            active_action: Mutex::new(None),
            latest_coordinator_action: Mutex::new(None),
            coordinator_feedback: Mutex::new(None),
            alive: AtomicBool::new(true),
            attempt: AtomicU64::new(0),
            tools: vec![],
        });
        let task = run.clone();
        let driver = thread::spawn(move || drive(&task, None));
        let deadline = Instant::now() + Duration::from_secs(20);
        if fail_coordinator {
            while Instant::now() < deadline {
                if coordinator_started.exists()
                    && worker_started.exists()
                    && run.active_action.lock().unwrap().is_none()
                {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            let draining = coordinator_started.exists()
                && worker_started.exists()
                && run.active_action.lock().unwrap().is_none()
                && !driver.is_finished();
            fs::write(&release, "release").unwrap();
            let result = driver.join().unwrap();
            let state = workflow.status(&id).unwrap();
            assert!(draining, "driver stopped before the running worker settled");
            assert_eq!(result.unwrap_err()["code"], "coordinator_execution_failed");
            assert!(
                state["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|event| event["kind"] == "launch" && event["task_id"] == "two")
            );
            return;
        }
        let mut completed = false;
        while Instant::now() < deadline {
            let state = workflow.status(&id).unwrap();
            completed = state["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|task| task["id"] == "two" && task["status"] == "done");
            if coordinator_started.exists() && completed {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let overlapped = coordinator_started.exists() && worker_started.exists() && completed;
        fs::write(&release, "release").unwrap();
        workflow.control(&id, "cancel").unwrap();
        let _ = driver.join();
        assert!(
            overlapped,
            "independent worker did not complete while coordinator was blocked"
        );
    }
}
