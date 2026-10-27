//! Bounded, sanitized snapshots of owned workflow process output.
use crate::workflow::Workflow;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

const TAIL: u64 = 64 * 1024;
const MAX_STREAMS: usize = 128;
const MAX_ENTRIES: usize = 128;

fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))[..24].to_string()
}
fn safe_run(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}
fn clean(s: &str) -> String {
    let ansi = Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap();
    let mut v: String = ansi
        .replace_all(s, "")
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect();
    for pattern in [
        r"(?i)\b(bearer\s+)[a-z0-9._~+/-]{8,}",
        r#"(?i)\b((?:api[_-]?key|token|password|secret)\s*[:=]\s*['\"]?)[^\s'\",;]{4,}"#,
        r"\b(sk-[A-Za-z0-9_-]{12,})\b",
    ] {
        let re = Regex::new(pattern).unwrap();
        v = if pattern.contains("sk-") {
            re.replace_all(&v, "[REDACTED]").into_owned()
        } else {
            re.replace_all(&v, "${1}[REDACTED]").into_owned()
        };
    }
    v.chars().take(1000).collect()
}
fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut p = v;
    for k in path {
        p = p.get(*k)?;
    }
    p.as_str()
}
fn event(v: &Value) -> Option<(&'static str, String)> {
    let typ = v["type"].as_str().unwrap_or("");
    if typ.contains("thinking")
        || typ.contains("reasoning")
        || typ.contains("init")
        || typ.contains("config")
    {
        return None;
    }
    if typ == "item.completed" || typ == "item.updated" || typ == "item.started" {
        let item = &v["item"];
        match item["type"].as_str().unwrap_or("") {
            "agent_message" => return get(item, &["text"]).map(|s| ("text", s.to_owned())),
            "command_execution" => {
                return get(item, &["command"]).map(|s| ("tool", format!("Command: {s}")));
            }
            "file_change" => return Some(("tool", "File changes".into())),
            "reasoning" => return None,
            _ => {}
        }
    }
    if typ == "assistant" || typ == "user" {
        if let Some(content) = v["message"]["content"].as_array() {
            for c in content {
                match c["type"].as_str().unwrap_or("") {
                    "text" if typ == "assistant" => {
                        if let Some(s) = c["text"].as_str() {
                            return Some(("text", s.into()));
                        }
                    }
                    "tool_use" => {
                        return Some((
                            "tool",
                            format!("Tool: {}", c["name"].as_str().unwrap_or("tool")),
                        ));
                    }
                    "tool_result" => {
                        if let Some(s) = c["content"].as_str() {
                            return Some(("tool", s.into()));
                        }
                    }
                    _ => {}
                }
            }
        }
        return None;
    }
    if typ == "text" {
        return get(v, &["part", "text"])
            .or_else(|| get(v, &["text"]))
            .map(|s| ("text", s.to_owned()));
    }
    if typ == "tool_use" {
        return Some((
            "tool",
            format!(
                "Tool: {}",
                get(v, &["part", "tool"])
                    .or_else(|| get(v, &["name"]))
                    .unwrap_or("tool")
            ),
        ));
    }
    if typ == "step_update" {
        return get(v, &["step_update", "agent_response", "text_delta"])
            .or_else(|| get(v, &["agent_response", "text_delta"]))
            .map(|s| ("text", s.into()));
    }
    if typ == "result" {
        return get(v, &["result", "response"]).map(|s| ("text", s.into()));
    }
    if typ == "error" {
        return Some((
            "error",
            clean(get(v, &["message"]).unwrap_or("Provider error")),
        ));
    }
    if !typ.is_empty()
        && typ.len() <= 64
        && typ
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Some(("event", format!("Event: {typ}")));
    }
    None
}
fn file(path: &Path) -> Option<File> {
    if !fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    File::open(path).ok()
}
fn tail(path: &Path, id: &str, source: &str) -> (Vec<Value>, bool, Option<u64>, u64) {
    let Some(mut f) = file(path) else {
        return (vec![], false, None, 0);
    };
    let Ok(meta) = f.metadata() else {
        return (vec![], false, None, 0);
    };
    let len = meta.len();
    let start = len.saturating_sub(TAIL);
    let mut bytes = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.take(TAIL).read_to_end(&mut bytes).is_err() {
        return (vec![], false, None, len);
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64);
    let mut entries = Vec::new();
    let mut offset = 0usize;
    if start > 0 {
        offset = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(bytes.len());
    }
    while offset < bytes.len() {
        let Some(end) = bytes[offset..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| offset + n)
        else {
            break;
        };
        if let Ok(line) = std::str::from_utf8(&bytes[offset..end]) {
            let line = line.trim_end_matches('\r');
            let trimmed = line.trim();
            let parsed = if trimmed.starts_with('{') || trimmed.starts_with('[') {
                serde_json::from_str::<Value>(trimmed)
                    .ok()
                    .and_then(|v| event(&v))
            } else if !trimmed.is_empty() {
                Some((
                    if source == "stderr" { "error" } else { "text" },
                    line.to_owned(),
                ))
            } else {
                None
            };
            if let Some((kind, text)) = parsed {
                let text = clean(&text);
                if !text.is_empty() {
                    entries.push(json!({"id":format!("{id}:{source}:{}",start + offset as u64),"source":source,"kind":kind,"text":text}));
                }
            }
        }
        offset = end + 1;
    }
    (entries, start > 0, modified, len)
}
struct Stream {
    value: Value,
    dir: PathBuf,
    hash: String,
}
fn add(
    streams: &mut Vec<Stream>,
    dir: PathBuf,
    hash: String,
    task: Option<&str>,
    provider: Option<&str>,
    stage: &str,
    fallback: &str,
    now: u64,
    retry: bool,
) {
    if streams.len() >= MAX_STREAMS || streams.iter().any(|s| s.dir == dir && s.hash == hash) {
        return;
    }
    let record_path = dir.join("executions").join(format!("{hash}.json"));
    let record = file(&record_path)
        .and_then(|mut f| {
            let mut b = Vec::new();
            f.by_ref().take(64 * 1024).read_to_end(&mut b).ok()?;
            serde_json::from_slice::<Value>(&b).ok()
        })
        .unwrap_or(Value::Null);
    let out = dir.join("output").join(format!("{hash}.out"));
    let err = dir.join("output").join(format!("{hash}.err"));
    if record.is_null() && !out.exists() && !err.exists() {
        return;
    }
    let alive = if record.is_null() {
        None
    } else {
        crate::adapters::execution_alive(&record)
    };
    let modified = [out, err]
        .iter()
        .filter_map(|p| {
            fs::metadata(p)
                .ok()?
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        })
        .max();
    let start = record["started_unix_ms"].as_u64();
    let status = record["status"].as_str().unwrap_or(fallback);
    let silence = modified.or(start).map(|t| now.saturating_sub(t));
    let id = format!("{stage}-{hash}");
    let needs_input = alive == Some(true)
        && ["out", "err"].iter().any(|suffix| {
            let (entries, _, _, _) = tail(
                &dir.join("output").join(format!("{hash}.{suffix}")),
                &id,
                suffix,
            );
            entries.iter().any(|e| {
                let text = e["text"].as_str().unwrap_or("").to_ascii_lowercase();
                [
                    "permission required",
                    "permission to continue",
                    "approve this command",
                    "press enter to continue",
                    "please log in",
                    "please login",
                    "sign in to continue",
                ]
                .iter()
                .any(|p| text.contains(p))
            })
        });
    let health = if needs_input {
        "needs_input"
    } else if status == "failed" || status == "timeout" || status == "output_limit" {
        "failed"
    } else if alive == Some(true) {
        if silence.unwrap_or(0) >= 120_000 {
            "quiet_long"
        } else if silence.unwrap_or(0) >= 30_000 {
            "quiet"
        } else {
            "active"
        }
    } else {
        status
    };
    let value = json!({"id":id,"task_id":task,"provider":provider,"stage":stage,"status":status,"health":health,"label":health,"elapsed_ms":start.map(|t| record["finished_unix_ms"].as_u64().unwrap_or(now).saturating_sub(t)),"silence_ms":silence,"last_output_at_ms":modified,"process_alive":alive,"stdout_bytes":record["stdout_bytes"].as_u64().unwrap_or_else(|| fs::metadata(dir.join("output").join(format!("{hash}.out"))).map(|m|m.len()).unwrap_or(0)),"stderr_bytes":record["stderr_bytes"].as_u64().unwrap_or_else(|| fs::metadata(dir.join("output").join(format!("{hash}.err"))).map(|m|m.len()).unwrap_or(0)),"can_retry":retry && alive == Some(true) && status == "running"});
    streams.push(Stream { value, dir, hash });
}
/// Snapshot only filenames derived from workflow-owned execution IDs.
pub fn snapshot(
    root: &Path,
    run: &str,
    state: &Value,
    coordinator: Option<&Value>,
    selected: Option<&str>,
    now_ms: u64,
) -> Result<Value, Value> {
    if !safe_run(run) {
        return Err(json!({"code":"invalid_request"}));
    }
    let request = Workflow::new(root.to_path_buf())
        .request(run)
        .map_err(|e| e.to_value())?;
    let run_dir = root.join(run);
    let host = run_dir.join("host");
    let mut streams = Vec::new();
    if let Some(c) = coordinator {
        if let Some(id) = c["action"]["id"].as_str() {
            let attempt = c["action"]["coordinator_attempt"].as_u64().unwrap_or(0);
            let launch = crate::web_coordinator::key(id, attempt, "launch");
            add(
                &mut streams,
                run_dir.join("coordinator"),
                hash(&launch),
                None,
                c["candidate"]["provider"].as_str(),
                "coordinator",
                "unknown",
                now_ms,
                false,
            );
        }
    }
    let tasks = state["tasks"].as_array().cloned().unwrap_or_default();
    for task in &tasks {
        let tid = task["id"].as_str();
        let provider = task["candidate"]["provider"].as_str();
        let status = task["status"].as_str().unwrap_or("unknown");
        let launch = state["actions"]
            .as_array()
            .and_then(|a| {
                a.iter()
                    .rev()
                    .find(|a| a["kind"] == "launch" && a["task_id"] == task["id"])
            })
            .and_then(|a| a["id"].as_str())
            .or_else(|| task["execution_id"].as_str());
        let fallback_hash = task["evidence"].as_array().and_then(|es| {
            es.iter().rev().find_map(|e| {
                if e["stage"] != "launch" {
                    return None;
                }
                let s = e["stream_ref"].as_str()?;
                let base = s.rsplit(['/', '\\']).next()?.strip_suffix(".out")?;
                if base.len() == 24
                    && base
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    Some(base.to_owned())
                } else {
                    None
                }
            })
        });
        if let Some(h) = launch.map(hash).or(fallback_hash) {
            let retry = state["status"] == "running"
                && task["repair_rounds"].as_u64().unwrap_or(0)
                    < request["repair_rounds"].as_u64().unwrap_or(2);
            add(
                &mut streams,
                host.clone(),
                h,
                tid,
                provider,
                "worker",
                status,
                now_ms,
                retry,
            );
        }
        let count = request["checks"].as_array().map_or(0, |a| a.len()).min(64);
        for (stage, key) in [
            ("verify", "workspace"),
            ("integrate", "integration_workspace"),
        ] {
            if let Some(dir) = task["prepared"][key].as_str() {
                for i in 0..count {
                    let tag = format!("check-{}-{i}", Path::new(dir).display());
                    add(
                        &mut streams,
                        host.clone(),
                        hash(&tag),
                        tid,
                        provider,
                        stage,
                        status,
                        now_ms,
                        false,
                    );
                }
            }
        }
    }
    let chosen = if let Some(id) = selected {
        Some(
            streams
                .iter()
                .find(|s| s.value["id"] == id)
                .ok_or_else(|| json!({"code":"unknown_stream"}))?,
        )
    } else {
        streams
            .iter()
            .find(|s| s.value["process_alive"] == true)
            .or_else(|| streams.first())
    };
    let selected_value = chosen.map(|s| {
        let id = s.value["id"].as_str().unwrap_or("");
        let (mut out, a, _, _) = tail(
            &s.dir.join("output").join(format!("{}.out", s.hash)),
            id,
            "stdout",
        );
        let (err, b, _, _) = tail(
            &s.dir.join("output").join(format!("{}.err", s.hash)),
            id,
            "stderr",
        );
        out.extend(err);
        let truncated = a || b || out.len() > MAX_ENTRIES;
        if out.len() > MAX_ENTRIES {
            out.drain(..out.len() - MAX_ENTRIES);
        }
        let mut value = s.value.clone();
        value["entries"] = json!(out);
        value["truncated"] = json!(truncated);
        value
    });
    let active = streams.iter().find(|s| s.value["process_alive"] == true);
    let (health, action) = if state["status"] == "paused" {
        ("paused", Some("resume"))
    } else if coordinator.is_some_and(|c| c["error"].is_object() || c["error"].is_string()) {
        ("needs_attention", Some("resume"))
    } else if let Some(s) = active {
        (s.value["health"].as_str().unwrap_or("active"), None)
    } else {
        (state["status"].as_str().unwrap_or("unknown"), None)
    };
    Ok(
        json!({"run_id":run,"observed_at_ms":now_ms,"health":{"status":health,"label":health,"action":action},"streams":streams.iter().map(|s| &s.value).collect::<Vec<_>>(),"selected_stream":selected_value}),
    )
}
