//! Loopback-only viewer for one workflow run.
use crate::{agent::ConfiguredJudge, web_workbench::Workbench, workflow::Workflow};
use serde_json::{Value, json};
use std::{
    error::Error,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::Arc,
    time::Duration,
};

const HTML: &str = include_str!("../assets/workflow.html");
const MAX_REQUEST: usize = 40 * 1024;

fn asset(path: &str) -> Option<(&'static str, &'static str)> {
    Some(match path {
        "/ui/styles.css" => (
            "text/css; charset=utf-8",
            include_str!("../assets/workflow-ui/styles.css"),
        ),
        "/ui/graph-output.css" => (
            "text/css; charset=utf-8",
            include_str!("../assets/workflow-ui/graph-output.css"),
        ),
        "/ui/live.css" => (
            "text/css; charset=utf-8",
            include_str!("../assets/workflow-ui/live.css"),
        ),
        "/ui/app.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../assets/workflow-ui/app.js"),
        ),
        "/ui/model.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../assets/workflow-ui/model.js"),
        ),
        "/ui/graph.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../assets/workflow-ui/graph.js"),
        ),
        "/ui/worker-output.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../assets/workflow-ui/worker-output.js"),
        ),
        _ => return None,
    })
}

type AnyError = Box<dyn Error + Send + Sync>;

#[cfg(windows)]
fn random_token() -> Result<String, AnyError> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            len: u32,
            flags: u32,
        ) -> i32;
    }
    let mut bytes = [0u8; 32];
    // BCRYPT_USE_SYSTEM_PREFERRED_RNG selects the operating system CSPRNG.
    if unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            2,
        )
    } != 0
    {
        return Err("Session initialization failed".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
#[cfg(not(windows))]
fn random_token() -> Result<String, AnyError> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn safe_string(value: &Value, max: usize) -> String {
    value
        .as_str()
        .unwrap_or("")
        .chars()
        .take(max)
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}
fn safe_wait_reason(value: &Value) -> Value {
    let Some(code) = value["code"]
        .as_str()
        .filter(|code| ["resource_capacity", "write_conflict", "parallel_limit"].contains(code))
    else {
        return Value::Null;
    };
    let mut reason = json!({"code": code});
    for field in ["resource_key", "task_id"] {
        if value[field].is_string() {
            reason[field] = json!(safe_string(&value[field], 128));
        }
    }
    if let Some(limit) = value["limit"].as_u64().filter(|n| (1..=16).contains(n)) {
        reason["limit"] = json!(limit);
    }
    reason
}
fn view(state: &Value) -> Value {
    let actions = state["actions"].as_array().cloned().unwrap_or_default();
    let tasks: Vec<Value> = state["tasks"].as_array().into_iter().flatten().map(|t| {
        let id = safe_string(&t["id"], 128);
        let action = actions.iter().find(|a| a["task_id"].as_str() == Some(id.as_str()));
        let candidate = &t["candidate"];
        json!({"id":id,"objective":safe_string(&t["task"]["objective"], 500),"role":safe_string(&t["task"]["role"],64),"status":safe_string(&t["status"],64),"model":safe_string(&candidate["model"],128),"effort":safe_string(&candidate["effort"],32),"provider":safe_string(&candidate["provider"],128),"candidate_id":safe_string(&candidate["id"],128),"observed":{"model":safe_string(&t["observed"]["model"],128),"effort":safe_string(&t["observed"]["effort"],32)},"depends_on":t["task"]["depends_on"].as_array().into_iter().flatten().map(|v| safe_string(v,128)).collect::<Vec<_>>(),"wait_reason":safe_wait_reason(&t["wait_reason"]),"next_action":action.map(|a| safe_string(&a["kind"],64)).unwrap_or_default(),"repair_rounds":t["repair_rounds"].as_u64().unwrap_or(0),"acceptance_count":t["task"]["acceptance"].as_array().map_or(0,Vec::len),"evidence":t["evidence"].as_array().into_iter().flatten().take(64).map(|e| { let stage=e["stage"].as_str().filter(|s| ["route","review_route","prepare","launch","inspect","verify","integrate","gather_context","repair"].contains(s)).unwrap_or("other"); let status=e["status"].as_str().filter(|s| ["ready","hold","prepared","running","unknown","completed","failed","cancelled","passed","integrated","gathered","accepted"].contains(s)).unwrap_or("accepted"); let code=safe_string(&e["error_code"],128); let commit=safe_string(&e["commit"],64); json!({"stage":stage,"status":status,"error_code":if code.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b)){code}else{String::new()},"checks_passed":e["checks_passed"].as_u64(),"checks_failed":e["checks_failed"].as_u64(),"evidence_count":e["evidence_count"].as_u64(),"permission_denial_count":e["permission_denial_count"].as_u64(),"commit":if (commit.len()==40||commit.len()==64)&&commit.bytes().all(|b|b.is_ascii_hexdigit()){commit}else{String::new()}}) }).collect::<Vec<_>>()})
    }).collect();
    let mut result = json!({"run_id":safe_string(&state["run_id"],128),"status":safe_string(&state["status"],64),"summary":safe_string(&state["summary"],200),"progress":{"done":state["progress"]["done"].as_u64().unwrap_or(0),"total":state["progress"]["total"].as_u64().unwrap_or(0)},"tasks":tasks,"actions":actions.iter().map(|a|json!({"kind":safe_string(&a["kind"],64),"task_id":safe_string(&a["task_id"],128)})).collect::<Vec<_>>(),"event_count":state["event_count"].as_u64().unwrap_or_else(|| state["events"].as_array().map_or(0, Vec::len) as u64)});
    for (row, task) in result["tasks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(state["tasks"].as_array().into_iter().flatten())
    {
        row["outputs"] = json!(
            task["task"]["outputs"]
                .as_array()
                .into_iter()
                .flatten()
                .take(128)
                .map(|value| safe_string(value, 2048))
                .collect::<Vec<_>>()
        );
    }
    for field in [
        "workspace",
        "goal",
        "coordinator",
        "attention",
        "result",
        "tools",
        "connection",
        "max_parallel",
        "startup_submission",
        "feedback",
        "plan_revision",
        "dependency_overrides",
        "editable_task_ids",
        "plan_review_pending",
    ] {
        if let Some(value) = state.get(field) {
            result[field] = value.clone();
        }
    }
    if let Some(url) = state["handoff"]["url"].as_str()
        && let Ok(parsed) = reqwest::Url::parse(url)
        && parsed.scheme() == "http"
        && parsed.host_str() == Some("127.0.0.1")
        && parsed.port().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
    {
        result["handoff"] = json!({"url":parsed.as_str(),"bundle_id":safe_string(&state["handoff"]["bundle_id"],96)});
    }
    if state["runtime"].is_object() {
        let runtime = &state["runtime"];
        result["runtime"] = json!({
            "switchbite":{"version":safe_string(&runtime["switchbite"]["version"],128),"commit":safe_string(&runtime["switchbite"]["commit"],64)},
            "backend":{"kind":safe_string(&runtime["backend"]["kind"],128),"version":safe_string(&runtime["backend"]["version"],128),"commit":safe_string(&runtime["backend"]["commit"],64),"protocol_version":runtime["backend"]["protocol_version"].as_u64().map(|n| n.to_string()).unwrap_or_else(|| safe_string(&runtime["backend"]["protocol_version"],128))},
            "project":{"workspace":safe_string(&runtime["project"]["workspace"],1024),"base_commit":safe_string(&runtime["project"]["base_commit"],64)}
        });
    }
    result
}

fn response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body.as_bytes())
}
fn handle(
    mut stream: TcpStream,
    workflow: &Workflow,
    run_id: &str,
    host: &str,
    token: &str,
    app: Option<&Workbench>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    let (head_end, length) = loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        data.extend_from_slice(&buf[..n]);
        if data.len() > MAX_REQUEST {
            return response(
                &mut stream,
                "413 Payload Too Large",
                "text/plain",
                "Request too large",
            );
        }
        if let Some(pos) = data.windows(4).position(|x| x == b"\r\n\r\n") {
            let head = match std::str::from_utf8(&data[..pos]) {
                Ok(h) => h,
                Err(_) => {
                    return response(
                        &mut stream,
                        "400 Bad Request",
                        "text/plain",
                        "Invalid request",
                    );
                }
            };
            let mut length = 0usize;
            for line in head.split("\r\n").skip(1) {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = match value.trim().parse() {
                            Ok(n) if n <= 32 * 1024 => n,
                            _ => {
                                return response(
                                    &mut stream,
                                    "400 Bad Request",
                                    "text/plain",
                                    "Invalid request",
                                );
                            }
                        };
                    }
                    if name.eq_ignore_ascii_case("transfer-encoding") {
                        return response(
                            &mut stream,
                            "400 Bad Request",
                            "text/plain",
                            "Invalid request",
                        );
                    }
                }
            }
            break (pos + 4, length);
        }
    };
    while data.len() < head_end + length {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        data.extend_from_slice(&buf[..n]);
        if data.len() > MAX_REQUEST {
            return response(
                &mut stream,
                "413 Payload Too Large",
                "text/plain",
                "Request too large",
            );
        }
    }
    let head = match std::str::from_utf8(&data[..head_end - 4]) {
        Ok(h) => h,
        Err(_) => {
            return response(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                "Invalid request",
            );
        }
    };
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or("");
    let headers: Vec<(&str, &str)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k, v.trim())))
        .collect();
    let header = |name: &str| -> Option<&str> {
        let mut values = headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v);
        let value = values.next()?;
        if values.next().is_some() {
            None
        } else {
            Some(value)
        }
    };
    if header("host") != Some(host) {
        return response(&mut stream, "403 Forbidden", "text/plain", "Forbidden");
    }
    if line.starts_with("POST ")
        && (header("origin") != Some(format!("http://{host}").as_str())
            || header("x-switchbite-session") != Some(token)
            || header("content-type") != Some("application/json")
            || header("sec-fetch-site").is_some_and(|v| v != "same-origin"))
    {
        return response(&mut stream, "403 Forbidden", "text/plain", "Forbidden");
    }
    match line {
        line if line.starts_with("GET /ui/") => {
            let path = line
                .strip_prefix("GET ")
                .and_then(|s| s.strip_suffix(" HTTP/1.1"))
                .unwrap_or("");
            match asset(path) {
                Some((mime, content)) => response(&mut stream, "200 OK", mime, content),
                None => response(&mut stream, "404 Not Found", "text/plain", "Not found"),
            }
        }
        line if line.starts_with("GET /api/artifacts")
            || line == "POST /api/artifacts/preview HTTP/1.1" =>
        {
            if header("x-switchbite-session") != Some(token)
                || header("origin").is_some_and(|origin| origin != format!("http://{host}"))
                || header("sec-fetch-site").is_some_and(|site| site != "same-origin")
            {
                return response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    "{\"error\":{\"code\":\"forbidden\"}}",
                );
            }
            let state = app.map_or_else(
                || crate::web_workbench::recorded_state(workflow, run_id),
                Workbench::status,
            );
            let result = state.map_err(|_| "run_unavailable").and_then(|state| {
                if line == "GET /api/artifacts HTTP/1.1" {
                    return crate::workflow_artifacts::list(&state);
                }
                if line == "POST /api/artifacts/preview HTTP/1.1" {
                    let body: Value = serde_json::from_slice(&data[head_end..head_end + length])
                        .map_err(|_| "invalid_request")?;
                    if body.as_object().is_none_or(|o| o.len() != 1) {
                        return Err("invalid_request");
                    }
                    return crate::workflow_artifacts::preview(
                        &state,
                        body["path"].as_str().ok_or("invalid_path")?,
                    );
                }
                let target = line
                    .strip_prefix("GET ")
                    .and_then(|s| s.strip_suffix(" HTTP/1.1"))
                    .ok_or("invalid_request")?;
                let url = reqwest::Url::parse(&format!("http://{host}{target}"))
                    .map_err(|_| "invalid_request")?;
                let pairs: Vec<_> = url.query_pairs().collect();
                if url.path() != "/api/artifacts/text" || pairs.len() != 1 || pairs[0].0 != "path" {
                    return Err("invalid_request");
                }
                crate::workflow_artifacts::text(&state, &pairs[0].1)
            });
            match result {
                Ok(value) => response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &value.to_string(),
                ),
                Err(code) => response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"error":{"code":code}}).to_string(),
                ),
            }
        }
        line if line.starts_with("GET /api/activity") => {
            if header("x-switchbite-session") != Some(token)
                || header("origin").is_some_and(|origin| origin != format!("http://{host}"))
                || header("sec-fetch-site").is_some_and(|site| site != "same-origin")
            {
                return response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    "{\"error\":{\"code\":\"forbidden\"}}",
                );
            }
            let target = line
                .strip_prefix("GET ")
                .and_then(|s| s.strip_suffix(" HTTP/1.1"))
                .unwrap_or("");
            let url = reqwest::Url::parse(&format!("http://{host}{target}"));
            let selected = match url {
                Ok(url) if url.path() == "/api/activity" => {
                    let pairs: Vec<_> = url
                        .query_pairs()
                        .map(|(key, value)| (key.into_owned(), value.into_owned()))
                        .collect();
                    if pairs.len() > 1
                        || pairs.iter().any(|(key, value)| {
                            key != "stream" || value.len() > 256 || value.is_empty()
                        })
                    {
                        return response(
                            &mut stream,
                            "400 Bad Request",
                            "application/json",
                            "{\"error\":{\"code\":\"invalid_stream\"}}",
                        );
                    }
                    pairs.first().map(|(_, value)| value.clone())
                }
                _ => return response(&mut stream, "404 Not Found", "text/plain", "Not found"),
            };
            let result = if let Some(app) = app {
                app.activity(selected.as_deref())
            } else {
                crate::web_workbench::recorded_state(workflow, run_id).and_then(|state| {
                    crate::activity::snapshot(
                        workflow.root(),
                        run_id,
                        &state,
                        None,
                        selected.as_deref(),
                        crate::agent::now_ms(),
                    )
                })
            };
            match result {
                Ok(mut value) => {
                    // A standalone viewer does not own the Workbench repair driver.
                    if app.is_none() {
                        for stream in value["streams"].as_array_mut().into_iter().flatten() {
                            stream["can_retry"] = json!(false);
                        }
                        if let Some(selected) = value["selected_stream"].as_object_mut() {
                            selected.insert("can_retry".into(), json!(false));
                        }
                    }
                    response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &value.to_string(),
                    )
                }
                Err(error) => response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "GET / HTTP/1.1" => response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            &HTML
                .replace("__SESSION_TOKEN__", token)
                .replace("__APP_MODE__", if app.is_some() { "true" } else { "false" }),
        ),
        "GET /api/app HTTP/1.1" if app.is_some() => {
            let result = app.unwrap().info();
            match result {
                Ok(mut value) => {
                    value["app_version"] = json!(env!("CARGO_PKG_VERSION"));
                    if !value["current"].is_null() {
                        value["current"] = view(&value["current"]);
                    }
                    response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &value.to_string(),
                    )
                }
                Err(error) => response(
                    &mut stream,
                    "409 Conflict",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "GET /api/runtime HTTP/1.1" if app.is_some() => {
            let value = crate::engine_updates::status()
                .unwrap_or_else(|_| json!({"status":"error","error":"runtime_status_unavailable"}));
            response(
                &mut stream,
                "200 OK",
                "application/json",
                &value.to_string(),
            )
        }
        "POST /api/start HTTP/1.1" if app.is_some() => {
            let body =
                serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
            match app.unwrap().start(&body) {
                Ok(state) => response(
                    &mut stream,
                    "202 Accepted",
                    "application/json",
                    &view(&state).to_string(),
                ),
                Err(error) => response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "POST /api/dependencies HTTP/1.1" if app.is_some() => {
            let body =
                serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
            match app.unwrap().set_dependencies(&body) {
                Ok(state) => response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &view(&state).to_string(),
                ),
                Err(error) => response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "POST /api/feedback HTTP/1.1" if app.is_some() => {
            let body =
                serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
            match app.unwrap().report(&body) {
                Ok(state) => response(
                    &mut stream,
                    "202 Accepted",
                    "application/json",
                    &view(&state).to_string(),
                ),
                Err(error) => response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "POST /api/parallelism HTTP/1.1" if app.is_some() => {
            let body: Value =
                serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
            let limit = body["max_parallel"]
                .as_u64()
                .filter(|n| (1..=16).contains(n));
            if body.as_object().is_none_or(|o| o.len() != 1) || limit.is_none() {
                return response(&mut stream, "400 Bad Request", "application/json",
                    &json!({"error":{"code":"invalid_max_parallel","message":"동시 작업 수는 1부터 16까지 입력하세요."}}).to_string());
            }
            match app.unwrap().set_parallelism(limit.unwrap()) {
                Ok(state) => response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &view(&state).to_string(),
                ),
                Err(error) => response(
                    &mut stream,
                    "409 Conflict",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "POST /api/task/retry HTTP/1.1" if app.is_some() => {
            let body: Value =
                serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
            let task = body["task_id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 128);
            if body.as_object().is_none_or(|object| object.len() != 1) || task.is_none() {
                return response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"error\":{\"code\":\"invalid_request\",\"message\":\"작업을 선택하세요.\"}}",
                );
            }
            match app.unwrap().retry_task(task.unwrap()) {
                Ok(state) => response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &view(&state).to_string(),
                ),
                Err(error) => response(
                    &mut stream,
                    "409 Conflict",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "POST /api/tools HTTP/1.1" if app.is_some() => {
            let body =
                serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
            match app.unwrap().select_tools(&body) {
                Ok(mut value) => {
                    if !value["current"].is_null() {
                        value["current"] = view(&value["current"]);
                    }
                    response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &value.to_string(),
                    )
                }
                Err(error) => response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "POST /api/tools/refresh HTTP/1.1" if app.is_some() => {
            if serde_json::from_slice::<Value>(&data[head_end..head_end + length]).ok()
                != Some(json!({}))
            {
                return response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"error\":\"Invalid refresh\"}",
                );
            }
            match app.unwrap().refresh_tools() {
                Ok(mut value) => {
                    if !value["current"].is_null() {
                        value["current"] = view(&value["current"]);
                    }
                    response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &value.to_string(),
                    )
                }
                Err(error) => response(
                    &mut stream,
                    "409 Conflict",
                    "application/json",
                    &json!({"error":error}).to_string(),
                ),
            }
        }
        "GET /api/state HTTP/1.1" => match app.map_or_else(
            || crate::web_workbench::recorded_state(workflow, run_id),
            Workbench::status,
        ) {
            Ok(v) => response(
                &mut stream,
                "200 OK",
                "application/json",
                &view(&v).to_string(),
            ),
            Err(_) => response(
                &mut stream,
                "404 Not Found",
                "application/json",
                "{\"error\":\"Run unavailable\"}",
            ),
        },
        "POST /api/control HTTP/1.1" => {
            if header("origin") != Some(format!("http://{host}").as_str())
                || header("x-switchbite-session") != Some(token)
                || header("content-type") != Some("application/json")
                || header("sec-fetch-site").is_some_and(|v| v != "same-origin")
            {
                return response(&mut stream, "403 Forbidden", "text/plain", "Forbidden");
            }
            let body: Value =
                serde_json::from_slice(&data[head_end..head_end + length]).unwrap_or(Value::Null);
            let action = body.get("action").and_then(Value::as_str).unwrap_or("");
            if body.as_object().is_none_or(|o| o.len() != 1)
                || !["pause", "resume", "cancel"].contains(&action)
            {
                return response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    "{\"error\":\"Invalid control\"}",
                );
            }
            match app.map_or_else(
                || workflow.control(run_id, action).map_err(|e| e.to_value()),
                |app| app.control(action),
            ) {
                Ok(v) => response(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &view(&v).to_string(),
                ),
                Err(_) => response(
                    &mut stream,
                    "409 Conflict",
                    "application/json",
                    "{\"error\":\"Control unavailable in current state\"}",
                ),
            }
        }
        _ => response(&mut stream, "404 Not Found", "text/plain", "Not found"),
    }
}

/// Serve a single run on 127.0.0.1. This call blocks until the process is stopped.
pub fn serve(workspace: &Path, run_id: &str, port: u16) -> Result<(), AnyError> {
    let workflow = Workflow::new(workspace.join(".switchbite").join("workflow"));
    workflow.status(run_id)?;
    let token = random_token()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
    let host = format!("127.0.0.1:{}", listener.local_addr()?.port());
    eprintln!("Workflow viewer: http://{host}/");
    for stream in listener.incoming().flatten() {
        let _ = handle(stream, &workflow, run_id, &host, &token, None);
    }
    Ok(())
}

/// Serve the local task composer and its current workflow.
pub fn serve_app(
    workspace: &Path,
    port: u16,
    judge: Option<Arc<ConfiguredJudge>>,
) -> Result<(), AnyError> {
    serve_app_resume(workspace, port, judge, None, None)
}

pub fn serve_app_resume(
    workspace: &Path,
    port: u16,
    judge: Option<Arc<ConfiguredJudge>>,
    resume: Option<&str>,
    coordinator: Option<&str>,
) -> Result<(), AnyError> {
    let _engine_updates = crate::engine_updates::watch()?;
    let app = Arc::new(Workbench::new(workspace, judge).map_err(|value| value.to_string())?);
    let workflow = Workflow::new(workspace.join(".switchbite/workflow"));
    let token = random_token()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
    if let Some(run) = resume {
        app.resume_existing(run, coordinator)
            .map_err(|value| value.to_string())?;
    }
    let initial = if resume.is_none() {
        crate::runtime_bundle::initial_submission()?
    } else {
        None
    };
    let host = format!("127.0.0.1:{}", listener.local_addr()?.port());
    crate::runtime_bundle::publish_web_ready(&host)?;
    if let Some(body) = initial {
        let app = app.clone();
        std::thread::spawn(move || {
            if let Err(error) = app.start(&body) {
                app.record_start_error(error, &body);
            }
        });
    }
    eprintln!("Switchbite app: http://{host}/");
    for stream in listener.incoming().flatten() {
        let _ = handle(stream, &workflow, "", &host, &token, Some(&app));
    }
    Ok(())
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    #[test]
    fn startup_failure_keeps_recovery_submission_in_view() {
        let state = json!({"status":"idle","attention":{"code":"startup_failed"},"startup_submission":{"workspace":"C:/project","goal":"work","tools":["codex"],"coordinator_id":"candidate","max_parallel":3}});
        let output = view(&state);
        assert_eq!(output["startup_submission"], state["startup_submission"]);
        assert_eq!(output["attention"]["code"], "startup_failed");
    }
    #[test]
    fn handoff_allows_only_a_loopback_http_endpoint() {
        let state = |url| json!({"status":"handoff","handoff":{"url":url,"bundle_id":"release-1","secret":"hidden"}});
        let accepted = view(&state("http://127.0.0.1:2468/"));
        assert_eq!(
            accepted["handoff"],
            json!({"url":"http://127.0.0.1:2468/","bundle_id":"release-1"})
        );
        for url in [
            "https://127.0.0.1:2468/",
            "http://example.com:2468/",
            "http://127.0.0.1/",
            "http://user@127.0.0.1:2468/",
            "http://127.0.0.1:2468/?token=x",
        ] {
            assert!(view(&state(url)).get("handoff").is_none(), "{url}");
        }
    }
    #[test]
    fn runtime_and_observed_identity_are_explicit_and_bounded() {
        let state = json!({"runtime":{"switchbite":{"version":"1.2","commit":"abc","secret":"hidden"},"backend":{"kind":"codex","version":"3","protocol_version":1},"project":{"workspace":"project","base_commit":"def"}},"tasks":[{"id":"a","candidate":{"model":"requested","effort":"high"},"observed":{"model":"actual"}}]});
        let result = view(&state);
        assert_eq!(result["tasks"][0]["model"], "requested");
        assert_eq!(result["tasks"][0]["observed"]["model"], "actual");
        assert_eq!(result["tasks"][0]["observed"]["effort"], "");
        assert_eq!(result["runtime"]["backend"]["protocol_version"], "1");
        assert!(result["runtime"]["switchbite"].get("secret").is_none());
    }
    #[test]
    fn wait_reason_is_bounded_in_summary() {
        let state = json!({"max_parallel":3,"tasks":[{"id":"a","wait_reason":{"code":"resource_capacity","resource_key":"codex","limit":3,"secret":"hidden"}},{"id":"b","wait_reason":{"code":"unexpected"}}]});
        let result = view(&state);
        assert_eq!(result["max_parallel"], 3);
        assert_eq!(
            result["tasks"][0]["wait_reason"],
            json!({"code":"resource_capacity","resource_key":"codex","limit":3})
        );
        assert!(result["tasks"][1]["wait_reason"].is_null());
    }
}
