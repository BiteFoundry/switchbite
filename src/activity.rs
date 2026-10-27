//! Bounded, sanitized snapshots of owned workflow process output.
use crate::workflow::Workflow;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::LazyLock,
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
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer\s+)[a-z0-9._~+/-]{8,}").unwrap());
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b((?:api[_-]?key|access[_-]?token|refresh[_-]?token|auth(?:orization|[_-]?token)?|token|password|passwd|secret)['\"]?\s*[:=]\s*['\"]?)[^\s'\",;]{4,}"#)
        .unwrap()
});
static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(sk-[A-Za-z0-9_-]{12,})\b").unwrap());
fn clean(s: &str) -> String {
    let v: String = ANSI
        .replace_all(s, "")
        .chars()
        .filter(|c| !c.is_control() || *c == '\t' || *c == '\n')
        .collect();
    let v = BEARER.replace_all(&v, "${1}[REDACTED]");
    let v = ASSIGNMENT.replace_all(&v, "${1}[REDACTED]");
    KEY.replace_all(&v, "[REDACTED]")
        .chars()
        .take(1000)
        .collect()
}
fn get<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut p = v;
    for k in path {
        p = p.get(*k)?;
    }
    p.as_str()
}
fn event(v: &Value) -> Vec<(&'static str, String)> {
    if matches!(
        v["type"].as_str(),
        Some("permission_request" | "approval_required" | "input_required")
    ) {
        return vec![(
            "error",
            "Permission required: provider is waiting for input".into(),
        )];
    }
    if matches!(v["type"].as_str(), Some("auth_required" | "login_required")) {
        return vec![("error", "Please log in to continue".into())];
    }
    let typ = v["type"]
        .as_str()
        .or_else(|| v["event"].as_str())
        .unwrap_or("");
    if typ.contains("thinking")
        || typ.contains("reasoning")
        || typ.contains("init")
        || typ.contains("config")
    {
        return vec![];
    }
    let mut visible = Vec::new();
    if ["item.completed", "item.updated", "item.started"].contains(&typ) {
        let item = &v["item"];
        match item["type"].as_str().unwrap_or("") {
            "agent_message" => {
                if let Some(s) = get(item, &["text"]) {
                    visible.push(("text", s.to_owned()));
                }
            }
            "command_execution" => {
                if let Some(s) = get(item, &["command"]) {
                    visible.push(("command", format!("Command: {s}")));
                }
                if let Some(s) = get(item, &["aggregated_output"]) {
                    visible.push(("command_output", s.to_owned()));
                }
            }
            "file_change" => visible.push(("tool", "File changes".into())),
            _ => {}
        }
        return visible;
    }
    if typ == "assistant" || typ == "user" {
        if let Some(content) = v["message"]["content"].as_array() {
            for c in content.iter().take(MAX_ENTRIES) {
                match c["type"].as_str().unwrap_or("") {
                    "text" if typ == "assistant" => {
                        if let Some(s) = c["text"].as_str() {
                            visible.push(("text", s.into()));
                        }
                    }
                    "tool_use" => visible.push((
                        "tool",
                        format!("Tool: {}", c["name"].as_str().unwrap_or("tool")),
                    )),
                    "tool_result" => {
                        if let Some(s) = c["content"].as_str() {
                            visible.push(("tool", s.into()));
                        }
                    }
                    _ => {}
                }
            }
        }
        return visible;
    }
    if typ == "text" {
        if let Some(s) = get(v, &["part", "text"]).or_else(|| get(v, &["text"])) {
            visible.push(("text", s.into()));
        }
    } else if typ == "tool_use" {
        visible.push((
            "tool",
            format!(
                "Tool: {}",
                get(v, &["part", "tool"])
                    .or_else(|| get(v, &["name"]))
                    .unwrap_or("tool")
            ),
        ));
    } else if typ == "step_update" {
        if v["step_update"]["step_type"] == "agent_response"
            && let Some(s) = get(v, &["step_update", "text_delta"])
        {
            visible.push(("text", s.into()));
        }
    } else if typ == "result" {
        if let Some(s) = get(v, &["result", "response"]) {
            visible.push(("text", s.into()));
        }
    } else if typ == "error" {
        visible.push((
            "error",
            get(v, &["message"]).unwrap_or("Provider error").into(),
        ));
    } else if !typ.is_empty()
        && typ.len() <= 64
        && typ
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        visible.push(("event", format!("Event: {typ}")));
    }
    visible
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
    let mut entries: Vec<Value> = Vec::new();
    let mut command_entries: HashMap<(String, &'static str), usize> = HashMap::new();
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
            let structured = if trimmed.starts_with('{') || trimmed.starts_with('[') {
                serde_json::from_str::<Value>(trimmed).ok()
            } else {
                None
            };
            let command_id = structured.as_ref().and_then(|v| {
                (v["item"]["type"] == "command_execution")
                    .then(|| v["item"]["id"].as_str())
                    .flatten()
                    .filter(|id| !id.is_empty() && id.len() <= 256)
            });
            let parsed = if let Some(v) = structured.as_ref() {
                event(v)
            } else if trimmed.starts_with('{') || trimmed.starts_with('[') {
                vec![]
            } else if !trimmed.is_empty() {
                vec![(
                    if source == "stderr" { "error" } else { "text" },
                    line.to_owned(),
                )]
            } else {
                vec![]
            };
            for (index, (kind, text)) in parsed.into_iter().enumerate() {
                let text = clean(&text);
                if !text.is_empty() {
                    if let Some(command_id) = command_id
                        && ["command", "command_output"].contains(&kind)
                    {
                        let key = (command_id.to_owned(), kind);
                        if let Some(&slot) = command_entries.get(&key) {
                            entries[slot]["text"] = json!(text);
                            continue;
                        }
                        command_entries.insert(key, entries.len());
                    }
                    entries.push(json!({"id":format!("{id}:{source}:{}:{index}",start + offset as u64),"source":source,"kind":kind,"text":text}));
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
    started: Option<u64>,
}
fn input_request(
    out: &[Value],
    out_time: Option<u64>,
    err: &[Value],
    err_time: Option<u64>,
) -> bool {
    let last_out = out
        .iter()
        .rev()
        .find(|entry| entry["kind"] != "event")
        .map(|entry| (out_time.unwrap_or(0), entry));
    let last_err = err
        .iter()
        .rev()
        .find(|entry| entry["kind"] != "event")
        .map(|entry| (err_time.unwrap_or(0), entry));
    let latest = match (last_out, last_err) {
        (Some(a), Some(b)) => Some(if b.0 >= a.0 { b } else { a }),
        (a, b) => a.or(b),
    };
    latest.is_some_and(|(_, entry)| {
        let text = entry["text"].as_str().unwrap_or("").to_ascii_lowercase();
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
        .any(|phrase| text.contains(phrase))
    })
}
fn stdout_path(dir: &Path, hash: &str) -> PathBuf {
    let output = dir.join("output").join(format!("{hash}.out"));
    if file(&output).is_some() {
        output
    } else {
        dir.join("messages").join(format!("{hash}.jsonl"))
    }
}
#[allow(clippy::too_many_arguments)] // Assemble one owned stream at the filesystem boundary.
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
    let out = stdout_path(&dir, &hash);
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
                .ok()
                .filter(|metadata| metadata.len() > 0)?
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        })
        .max();
    let start = record["started_unix_ms"].as_u64();
    let status = record["status"].as_str().unwrap_or(fallback);
    let finished = record["finished_unix_ms"]
        .as_u64()
        .or_else(|| {
            if alive != Some(true) && !["running", "launching"].contains(&status) {
                modified
            } else {
                None
            }
        })
        .unwrap_or(now);
    let silence = modified.or(start).map(|t| now.saturating_sub(t));
    let id = format!("{stage}-{hash}");
    let needs_input = if alive == Some(true) {
        let (out, _, out_time, _) = tail(&stdout_path(&dir, &hash), &id, "stdout");
        let (err, _, err_time, _) = tail(
            &dir.join("output").join(format!("{hash}.err")),
            &id,
            "stderr",
        );
        input_request(&out, out_time, &err, err_time)
    } else {
        false
    };
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
    } else if alive == Some(false) && status == "running" {
        "settling"
    } else {
        status
    };
    let value = json!({"id":id,"task_id":task,"provider":provider,"stage":stage,"status":status,"health":health,"label":health,"elapsed_ms":start.map(|t| finished.saturating_sub(t)),"silence_ms":silence,"last_output_at_ms":modified,"process_alive":alive,"stdout_bytes":record["stdout_bytes"].as_u64().unwrap_or_else(|| fs::metadata(stdout_path(&dir, &hash)).map(|m|m.len()).unwrap_or(0)),"stderr_bytes":record["stderr_bytes"].as_u64().unwrap_or_else(|| fs::metadata(dir.join("output").join(format!("{hash}.err"))).map(|m|m.len()).unwrap_or(0)),"can_retry":retry && alive == Some(true) && status == "running"});
    streams.push(Stream {
        value,
        dir,
        hash,
        started: start,
    });
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
    if let Some(c) = coordinator
        && let Some(id) = c["action"]["id"].as_str()
    {
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
    let mut tasks = state["tasks"].as_array().cloned().unwrap_or_default();
    tasks.sort_by_key(|task| {
        let active = state["actions"].as_array().and_then(|a| {
            a.iter()
                .rposition(|a| a["kind"] == "integrate" && a["task_id"] == task["id"])
        });
        let recent = state["events"].as_array().and_then(|a| {
            a.iter()
                .rposition(|e| e["stage"] == "integrate" && e["task_id"] == task["id"])
        });
        std::cmp::Reverse((
            active.is_some(),
            active.unwrap_or(0),
            recent.is_some(),
            recent.unwrap_or(0),
        ))
    });
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
                let name = s.rsplit(['/', '\\']).next()?;
                let base = name
                    .strip_suffix(".jsonl")
                    .or_else(|| name.strip_suffix(".out"))?;
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
            let action_id = state["actions"]
                .as_array()
                .and_then(|actions| {
                    actions
                        .iter()
                        .rev()
                        .find(|a| a["kind"] == stage && a["task_id"] == task["id"])
                        .and_then(|a| a["id"].as_str())
                })
                .or_else(|| {
                    task["evidence"].as_array().and_then(|evidence| {
                        evidence
                            .iter()
                            .rev()
                            .find(|e| e["stage"] == stage)
                            .and_then(|e| e["action_id"].as_str())
                    })
                });
            for i in 0..count {
                let tag = if let Some(action_id) = action_id {
                    format!("check-{action_id}-{i}")
                } else if let Some(dir) = task["prepared"][key].as_str() {
                    format!("check-{}-{i}", Path::new(dir).display())
                } else {
                    continue;
                };
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
    let chosen = if let Some(id) = selected {
        Some(
            streams
                .iter()
                .find(|s| s.value["id"] == id)
                .ok_or_else(|| json!({"code":"unknown_stream"}))?,
        )
    } else {
        let newest = |s: &&Stream| {
            (
                s.started.unwrap_or(0),
                s.value["last_output_at_ms"].as_u64().unwrap_or(0),
            )
        };
        streams
            .iter()
            .filter(|s| s.value["process_alive"] == true)
            .max_by_key(newest)
            .or_else(|| {
                streams
                    .iter()
                    .filter(|s| s.value["stage"] == "worker")
                    .max_by_key(|s| {
                        (
                            s.value["last_output_at_ms"].as_u64().unwrap_or(0),
                            s.started.unwrap_or(0),
                        )
                    })
            })
            .or_else(|| streams.iter().max_by_key(newest))
    };
    let selected_value = chosen.map(|s| {
        let id = s.value["id"].as_str().unwrap_or("");
        let (mut out, a, _, _) = tail(&stdout_path(&s.dir, &s.hash), id, "stdout");
        let (mut err, b, _, _) = tail(
            &s.dir.join("output").join(format!("{}.err", s.hash)),
            id,
            "stderr",
        );
        let truncated = a || b || out.len() + err.len() > MAX_ENTRIES;
        let out_keep = out.len().min(if err.is_empty() {
            MAX_ENTRIES
        } else {
            MAX_ENTRIES / 2
        });
        let err_keep = err.len().min(MAX_ENTRIES - out_keep);
        let out_keep = out.len().min(MAX_ENTRIES - err_keep);
        out.drain(..out.len() - out_keep);
        err.drain(..err.len() - err_keep);
        out.extend(err);
        let mut value = s.value.clone();
        value["entries"] = json!(out);
        value["truncated"] = json!(truncated);
        value
    });
    let active = streams.iter().find(|s| s.value["process_alive"] == true);
    let (health, action) = if state["status"] == "paused" {
        ("paused", Some("resume"))
    } else if (!state["coordinator"]["error"].is_null() && state["coordinator"]["error"] != false)
        || (!state["attention"].is_null() && state["attention"] != false)
    {
        ("needs_attention", Some("resume"))
    } else if let Some(s) = active {
        (s.value["health"].as_str().unwrap_or("active"), None)
    } else if state["status"] == "running" && tasks.iter().any(|task| task["status"] == "blocked") {
        ("blocked", None)
    } else {
        (state["status"].as_str().unwrap_or("unknown"), None)
    };
    Ok(
        json!({"run_id":run,"observed_at_ms":now_ms,"health":{"status":health,"label":health,"action":action},"streams":streams.iter().map(|s| &s.value).collect::<Vec<_>>(),"selected_stream":selected_value}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recent_progress_resolves_an_old_input_request() {
        let prompt = json!({"kind":"error","text":"Permission required: approve this command"});
        let progress = json!({"kind":"text","text":"Build completed"});
        assert!(input_request(
            &[],
            None,
            std::slice::from_ref(&prompt),
            Some(20)
        ));
        assert!(!input_request(
            std::slice::from_ref(&progress),
            Some(30),
            std::slice::from_ref(&prompt),
            Some(20)
        ));
        assert!(!input_request(
            &[prompt.clone(), progress],
            Some(30),
            &[],
            None
        ));
        assert!(input_request(
            &[json!({"kind":"event","text":"heartbeat"})],
            Some(30),
            &[prompt],
            Some(20)
        ));
    }
}
