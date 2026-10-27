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
fn view(state: &Value) -> Value {
    let actions = state["actions"].as_array().cloned().unwrap_or_default();
    let tasks: Vec<Value> = state["tasks"].as_array().into_iter().flatten().map(|t| {
        let id = safe_string(&t["id"], 128);
        let action = actions.iter().find(|a| a["task_id"].as_str() == Some(id.as_str()));
        let candidate = &t["candidate"];
        json!({"id":id,"objective":safe_string(&t["task"]["objective"], 500),"role":safe_string(&t["task"]["role"],64),"status":safe_string(&t["status"],64),"model":safe_string(&candidate["model"],128),"provider":safe_string(&candidate["provider"],128),"candidate_id":safe_string(&candidate["id"],128),"depends_on":t["task"]["depends_on"].as_array().into_iter().flatten().map(|v| safe_string(v,128)).collect::<Vec<_>>(),"next_action":action.map(|a| safe_string(&a["kind"],64)).unwrap_or_default(),"repair_rounds":t["repair_rounds"].as_u64().unwrap_or(0),"acceptance_count":t["task"]["acceptance"].as_array().map_or(0,Vec::len),"evidence":t["evidence"].as_array().into_iter().flatten().take(64).map(|e| { let stage=e["stage"].as_str().filter(|s| ["route","prepare","launch","inspect","verify","integrate","gather_context","repair"].contains(s)).unwrap_or("other"); let status=e["status"].as_str().filter(|s| ["ready","hold","prepared","running","unknown","completed","failed","cancelled","passed","integrated","gathered","accepted"].contains(s)).unwrap_or("accepted"); let code=safe_string(&e["error_code"],128); let commit=safe_string(&e["commit"],64); json!({"stage":stage,"status":status,"error_code":if code.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b)){code}else{String::new()},"checks_passed":e["checks_passed"].as_u64(),"checks_failed":e["checks_failed"].as_u64(),"evidence_count":e["evidence_count"].as_u64(),"commit":if (commit.len()==40||commit.len()==64)&&commit.bytes().all(|b|b.is_ascii_hexdigit()){commit}else{String::new()}}) }).collect::<Vec<_>>()})
    }).collect();
    let mut result = json!({"run_id":safe_string(&state["run_id"],128),"status":safe_string(&state["status"],64),"summary":safe_string(&state["summary"],200),"progress":{"done":state["progress"]["done"].as_u64().unwrap_or(0),"total":state["progress"]["total"].as_u64().unwrap_or(0)},"tasks":tasks,"actions":actions.iter().map(|a|json!({"kind":safe_string(&a["kind"],64),"task_id":safe_string(&a["task_id"],128)})).collect::<Vec<_>>(),"event_count":state["event_count"].as_u64().unwrap_or_else(|| state["events"].as_array().map_or(0, Vec::len) as u64)});
    for field in [
        "workspace",
        "goal",
        "coordinator",
        "attention",
        "result",
        "tools",
        "connection",
    ] {
        if let Some(value) = state.get(field) {
            result[field] = value.clone();
        }
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
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n",
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
            || workflow.status(run_id).map_err(|e| e.to_value()),
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
    let app = Workbench::new(workspace, judge).map_err(|value| value.to_string())?;
    let workflow = Workflow::new(workspace.join(".switchbite/workflow"));
    let token = random_token()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
    let host = format!("127.0.0.1:{}", listener.local_addr()?.port());
    eprintln!("Switchbite app: http://{host}/");
    for stream in listener.incoming().flatten() {
        let _ = handle(stream, &workflow, "", &host, &token, Some(&app));
    }
    Ok(())
}
