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
    alive: AtomicBool,
    attempt: AtomicU64,
}

pub struct Workbench {
    workspace: PathBuf,
    candidates: Vec<Value>,
    judge: Option<Arc<ConfiguredJudge>>,
    current: Mutex<Option<Arc<Run>>>,
}

fn error(code: &str, message: &str) -> Value {
    json!({"code":code,"message":message})
}
fn lock_error() -> Value {
    error("workbench_busy", "잠시 후 다시 시도하세요.")
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
        let snapshot = catalog::snapshot(workspace, &json!({"providers":["codex","claude"]}))
            .map_err(|e| e.to_value())?;
        Self::with_candidates(
            workspace,
            snapshot["candidates"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            judge,
        )
    }

    pub fn with_candidates(
        workspace: &Path,
        mut candidates: Vec<Value>,
        judge: Option<Arc<ConfiguredJudge>>,
    ) -> Result<Self, Value> {
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|_| error("invalid_workspace", "존재하는 프로젝트 폴더를 선택하세요."))?;
        candidates.retain(|c| {
            c["available"] == true
                && ["codex", "claude"].contains(&c["provider"].as_str().unwrap_or(""))
        });
        candidates.sort_by_key(|c| {
            let preferred =
                c["provider"] == "codex" && c["model"] == "gpt-6.1-terra" && c["effort"] == "high";
            (
                !preferred,
                c["provider"].as_str().unwrap_or("").to_owned(),
                c["model"].as_str().unwrap_or("").to_owned(),
                c["effort"].as_str().unwrap_or("").to_owned(),
            )
        });
        Ok(Self {
            workspace,
            candidates,
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
        Ok(
            json!({"workspace":self.workspace,"judge":self.judge.as_ref().map(|j|j.model()).unwrap_or("Offline"),"coordinators":self.candidates.iter().map(|c| json!({"id":c["id"],"provider":c["provider"],"model":c["model"],"effort":c["effort"],"label":format!("{} / {} / {}",c["provider"].as_str().unwrap_or(""),c["model"].as_str().unwrap_or(""),c["effort"].as_str().unwrap_or(""))})).collect::<Vec<_>>(),"current":current}),
        )
    }

    pub fn start(&self, body: &Value) -> Result<Value, Value> {
        let object = body
            .as_object()
            .ok_or_else(|| error("invalid_request", "프로젝트와 업무 내용을 입력하세요."))?;
        if object
            .keys()
            .any(|k| !["workspace", "goal", "coordinator_id"].contains(&k.as_str()))
        {
            return Err(error("invalid_request", "지원하지 않는 입력입니다."));
        }
        let goal = body["goal"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 24000)
            .ok_or_else(|| {
                error(
                    "invalid_goal",
                    "업무 내용을 24,000 바이트 이내로 입력하세요.",
                )
            })?;
        let candidate = self
            .candidates
            .iter()
            .find(|c| c["id"] == body["coordinator_id"])
            .cloned()
            .ok_or_else(|| {
                error(
                    "coordinator_unavailable",
                    "설치된 조율자 CLI와 모델을 선택하세요.",
                )
            })?;
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
        let state = Workflow::new(root.clone()).start(&json!({"workspace":workspace,"goal":goal,"catalog":{},"max_parallel":2,"host":{"timeout_seconds":900}})).map_err(|e| e.to_value())?;
        let run = Arc::new(Run {
            root,
            id: state["run_id"].as_str().unwrap().to_owned(),
            candidate,
            coordinator: Mutex::new(json!({"status":"planning"})),
            active_action: Mutex::new(None),
            alive: AtomicBool::new(false),
            attempt: AtomicU64::new(0),
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
        let mut state = workflow.status(&run.id).map_err(|e| e.to_value())?;
        let request = workflow.request(&run.id).map_err(|e| e.to_value())?;
        let mut coordinator = run.coordinator.lock().map_err(|_| lock_error())?.clone();
        for field in ["provider", "model", "effort"] {
            coordinator[field] = run.candidate[field].clone();
        }
        state["coordinator"] = coordinator.clone();
        state["workspace"] = request["workspace"].clone();
        state["goal"] = request["goal"].clone();
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
                    *run.coordinator.lock().map_err(|_| lock_error())? =
                        json!({"status":if action["kind"] == "plan" {"planning"}else{"repairing"}});
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
