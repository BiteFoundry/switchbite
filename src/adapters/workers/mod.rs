use super::{HostError, configure_codex_shell, path_string};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    process::Command,
};

#[cfg(test)]
use std::io::Cursor;

#[cfg(test)]
pub(super) fn verified_receipt(
    provider: &str,
    output: &[u8],
    session: &str,
) -> (bool, Option<String>) {
    verified_receipt_lines(provider, lines(Cursor::new(output)), session)
}

fn verified_receipt_lines(
    provider: &str,
    output: impl Iterator<Item = Vec<u8>>,
    session: &str,
) -> (bool, Option<String>) {
    if provider == "antigravity" {
        let receipt = antigravity_receipt_lines(output);
        return (receipt.is_some(), receipt.and_then(|r| r.1));
    }
    if provider == "cursor" || provider == "opencode" {
        return native_receipt_lines(provider, output);
    }
    let mut init = false;
    let mut turn = false;
    let mut terminal = false;
    let mut model = None;
    let mut valid = true;
    for line in output {
        let Ok(v) = serde_json::from_slice::<Value>(&line) else {
            return (false, None);
        };
        let Some(kind) = v.get("type").and_then(Value::as_str) else {
            return (false, None);
        };
        if terminal {
            valid = false;
        }
        if provider == "claude" {
            if v.get("session_id")
                .is_some_and(|x| x.as_str() != Some(session))
            {
                valid = false;
            }
            match kind {
                "system" if v["subtype"] == "init" => {
                    if init || v["session_id"] != session {
                        valid = false;
                    }
                    init = true;
                    model = v
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty() && s.len() <= 256)
                        .map(str::to_owned);
                }
                "result" => {
                    if !init
                        || v["session_id"] != session
                        || v["subtype"] != "success"
                        || v["is_error"] != false
                        || !v
                            .get("permission_denials")
                            .and_then(Value::as_array)
                            .is_some()
                    {
                        valid = false;
                    }
                    terminal = true;
                }
                "error" => valid = false,
                "assistant" | "user" => {
                    if !init
                        || v.get("error").is_some_and(|x| !x.is_null())
                        || v["message"]["stop_reason"] == "refusal"
                        || v["message"]["content"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|x| x["type"] == "refusal"))
                    {
                        valid = false;
                    }
                }
                _ => {
                    if !init {
                        valid = false;
                    }
                }
            }
        } else {
            match kind {
                "thread.started" => {
                    if init || v["thread_id"].as_str().is_none_or(str::is_empty) {
                        valid = false;
                    }
                    init = true;
                }
                "item.completed" if init && !turn && v["item"]["type"] == "error" => {
                    // Codex can report MCP startup failures before starting the turn.
                }
                "turn.started" => {
                    if !init || turn {
                        valid = false;
                    }
                    turn = true;
                }
                "turn.completed" => {
                    if !init || !turn {
                        valid = false;
                    }
                    terminal = true;
                }
                "error" | "turn.failed" => valid = false,
                "item.failed" => {
                    if !turn {
                        valid = false;
                    }
                }
                _ => {
                    if !turn {
                        valid = false;
                    }
                }
            }
        }
    }
    (valid && init && terminal, model)
}

#[cfg(test)]
pub(super) fn permission_denial_count(output: &[u8]) -> Option<usize> {
    permission_denial_count_lines(lines(Cursor::new(output)))
}
fn permission_denial_count_lines(output: impl Iterator<Item = Vec<u8>>) -> Option<usize> {
    output
        .filter_map(|line| serde_json::from_slice::<Value>(&line).ok())
        .find_map(|value| {
            if value["type"] != "result" {
                return None;
            }
            value["permission_denials"].as_array().map(Vec::len)
        })
}

// Keep memory bounded even when a provider emits a malformed oversized JSONL record.
const MAX_RECORD: usize = 16 * 1024 * 1024;
fn lines<R: BufRead>(mut reader: R) -> impl Iterator<Item = Vec<u8>> {
    let mut failed = false;
    std::iter::from_fn(move || {
        loop {
            if failed {
                return None;
            }
            let mut line = Vec::new();
            loop {
                let buf = match reader.fill_buf() {
                    Ok(b) => b,
                    Err(_) => {
                        failed = true;
                        return Some(b"invalid-json".to_vec());
                    }
                };
                if buf.is_empty() {
                    return if line.is_empty() { None } else { Some(line) };
                }
                let len = buf
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(buf.len(), |i| i + 1);
                if line.len() + len > MAX_RECORD {
                    failed = true;
                    return Some(b"invalid-json".to_vec());
                }
                let end = buf[len - 1] == b'\n';
                line.extend_from_slice(&buf[..len]);
                reader.consume(len);
                if end {
                    break;
                }
            }
            if line != b"\n" {
                return Some(line);
            }
        }
    })
}
fn file_lines(path: &Path) -> Result<impl Iterator<Item = Vec<u8>>, HostError> {
    Ok(lines(BufReader::new(
        std::fs::File::open(path).map_err(|_| HostError("output_read_failed"))?,
    )))
}
// Read one JSONL record through a streaming deserializer. Unused fields (including
// command/MCP output) are skipped by serde rather than materialized in memory.
struct RecordReader<'a, R> {
    reader: &'a mut R,
    ended: bool,
}
impl<R: BufRead> Read for RecordReader<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.ended || out.is_empty() {
            return Ok(0);
        }
        let buf = self.reader.fill_buf()?;
        if buf.is_empty() {
            self.ended = true;
            return Ok(0);
        }
        let window = &buf[..buf.len().min(out.len())];
        let n = window
            .iter()
            .position(|b| *b == b'\n')
            .unwrap_or(window.len());
        if n == 0 {
            self.reader.consume(1);
            self.ended = true;
            return Ok(0);
        }
        out[..n].copy_from_slice(&buf[..n]);
        self.reader.consume(n);
        Ok(n)
    }
}

struct ReceiptValue;
impl<'de> serde::de::DeserializeSeed<'de> for ReceiptValue {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(ReceiptVisitor)
    }
}
struct ReceiptVisitor;
impl<'de> serde::de::Visitor<'de> for ReceiptVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("receipt JSON")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null))
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(v) = seq.next_element_seed(ReceiptValue)? {
            values.push(v);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if matches!(
                key.as_str(),
                "type"
                    | "subtype"
                    | "session_id"
                    | "sessionID"
                    | "thread_id"
                    | "event"
                    | "conversation_id"
                    | "model"
                    | "is_error"
                    | "permission_denials"
                    | "error"
                    | "message"
                    | "stop_reason"
                    | "content"
                    | "part"
                    | "reason"
                    | "text"
                    | "result"
                    | "status"
                    | "response"
                    | "step_update"
                    | "item"
                    | "state"
            ) {
                values.insert(key, map.next_value_seed(ReceiptValue)?);
            } else {
                map.next_value::<serde::de::IgnoredAny>()?;
            }
        }
        Ok(Value::Object(values))
    }
}
fn receipt_file_lines(path: &Path) -> Result<impl Iterator<Item = Vec<u8>>, HostError> {
    use serde::de::DeserializeSeed;
    let mut reader =
        BufReader::new(std::fs::File::open(path).map_err(|_| HostError("output_read_failed"))?);
    let mut failed = false;
    Ok(std::iter::from_fn(move || {
        if failed {
            return None;
        }
        loop {
            match reader.fill_buf() {
                Ok([]) => return None,
                Ok(buf) if buf[0] == b'\n' => {
                    reader.consume(1);
                }
                Ok(_) => break,
                Err(_) => {
                    failed = true;
                    return Some(b"invalid-json".to_vec());
                }
            }
        }
        let record = RecordReader {
            reader: &mut reader,
            ended: false,
        };
        let mut de = serde_json::Deserializer::from_reader(record);
        match ReceiptValue.deserialize(&mut de).and_then(|v| {
            de.end()?;
            Ok(v)
        }) {
            Ok(value) => Some(serde_json::to_vec(&value).expect("receipt values serialize")),
            Err(_) => {
                failed = true;
                Some(b"invalid-json".to_vec())
            }
        }
    }))
}
pub(super) fn verified_receipt_file(
    provider: &str,
    path: &Path,
    session: &str,
) -> Result<(bool, Option<String>), HostError> {
    Ok(verified_receipt_lines(
        provider,
        receipt_file_lines(path)?,
        session,
    ))
}
pub(super) fn permission_denial_count_file(path: &Path) -> Result<Option<usize>, HostError> {
    Ok(permission_denial_count_lines(file_lines(path)?))
}
pub(super) fn message_file(provider: &str, path: &Path) -> Result<Option<String>, HostError> {
    Ok(message_lines(provider, file_lines(path)?))
}
pub(super) fn antigravity_session_file(path: &Path) -> Result<Option<String>, HostError> {
    Ok(antigravity_receipt_lines(file_lines(path)?).map(|r| r.0))
}

#[allow(clippy::too_many_arguments)] // Keep native argv construction at one protocol boundary.
pub(super) fn command(
    provider: &str,
    executable: &str,
    model: &str,
    effort: &str,
    session: &str,
    message: &Path,
    dir: &Path,
    prompt: &str,
    schema: Option<&Path>,
) -> Result<Command, HostError> {
    let mut cmd = native_command(provider, executable)?;
    match provider {
        "codex" => {
            configure_codex_shell(&mut cmd);
            cmd.args([
                "exec",
                "--json",
                "--model",
                model,
                "--sandbox",
                "workspace-write",
                "--ephemeral",
                "--output-last-message",
                path_string(message)?,
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "windows.sandbox=\"unelevated\"",
                "-c",
                "allow_login_shell=false",
            ]);
            if effort != "default" {
                if ![
                    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
                ]
                .contains(&effort)
                {
                    return Err(HostError("unsupported_effort"));
                }
                cmd.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
            }
            if let Some(schema) = schema {
                cmd.args(["--output-schema", path_string(schema)?]);
            }
            cmd.arg(prompt);
        }
        "claude" => {
            cmd.args([
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                model,
            ]);
            if effort != "default" {
                if !["low", "medium", "high", "xhigh", "max"].contains(&effort) {
                    return Err(HostError("unsupported_effort"));
                }
                cmd.args(["--effort", effort]);
            }
            cmd.args([
                "--session-id",
                session,
                "--no-session-persistence",
                "--permission-mode",
                "acceptEdits",
                "--permission-prompts",
                "none",
                "--allowedTools",
                "Bash(cargo build)",
                "Bash(cargo build *)",
                "Bash(cargo check)",
                "Bash(cargo check *)",
                "Bash(cargo test)",
                "Bash(cargo test *)",
                "Bash(npm test)",
                "Bash(npm test *)",
                "Bash(npm run build)",
                "Bash(npm run build *)",
                "Bash(pnpm test)",
                "Bash(pnpm test *)",
                "Bash(pnpm run build)",
                "Bash(pnpm run build *)",
                "--",
                prompt,
            ]);
        }
        "cursor" => {
            let selected = if effort == "default" {
                model.to_owned()
            } else {
                if model.contains(['[', ']']) {
                    return Err(HostError("unsupported_effort"));
                }
                format!("{model}[effort={effort}]")
            };
            cmd.args([
                "--print",
                "--output-format",
                "stream-json",
                "--trust",
                "--force",
                "--workspace",
                path_string(dir)?,
                "--model",
                &selected,
                "--",
                prompt,
            ]);
        }
        "antigravity" => {
            cmd.args([
                "--print",
                prompt,
                "--output-format",
                "stream-json",
                "--model",
                model,
                "--sandbox",
                "--mode",
                "accept-edits",
            ]);
            if effort != "default" {
                if !["low", "medium", "high", "xhigh", "max"].contains(&effort) {
                    return Err(HostError("unsupported_effort"));
                }
                cmd.args(["--effort", effort]);
            }
        }
        "opencode" => {
            let selected = if effort == "default" {
                model.to_owned()
            } else {
                if model.contains('#') {
                    return Err(HostError("unsupported_effort"));
                }
                format!("{model}#{effort}")
            };
            cmd.args([
                "run",
                "--standalone",
                "--format",
                "json",
                "--auto",
                "--model",
                &selected,
                "--",
                prompt,
            ]);
        }
        _ => return Err(HostError("unsupported_provider")),
    }
    Ok(cmd)
}

fn native_command(provider: &str, executable: &str) -> Result<Command, HostError> {
    #[cfg(windows)]
    {
        let path = Path::new(executable);
        if provider == "cursor"
            && path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("cursor-agent.cmd"))
        {
            let root = path.parent().ok_or(HostError("invalid_path"))?;
            let versions = root.join("versions");
            let mut dirs: Vec<_> = std::fs::read_dir(versions)
                .map_err(|_| HostError("invalid_path"))?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                            let Some((date, suffix)) = n.split_once('-') else {
                                return false;
                            };
                            let parts: Vec<_> = date.split('.').collect();
                            parts.len() == 3
                                && parts[0].len() == 4
                                && parts[1..].iter().all(|p| !p.is_empty() && p.len() <= 2)
                                && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
                                && !suffix.is_empty()
                                && suffix.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                        })
                        && p.join("node.exe").is_file()
                        && p.join("index.js").is_file()
                })
                .collect();
            dirs.sort_by_key(|p| {
                let name = p.file_name().unwrap().to_string_lossy();
                let mut date = name
                    .split('-')
                    .next()
                    .unwrap_or("")
                    .split('.')
                    .filter_map(|n| n.parse::<u32>().ok());
                (
                    date.next().unwrap_or(0),
                    date.next().unwrap_or(0),
                    date.next().unwrap_or(0),
                    name.to_string(),
                )
            });
            let selected = dirs.pop().ok_or(HostError("invalid_path"))?;
            let mut cmd = Command::new(selected.join("node.exe"));
            cmd.arg(selected.join("index.js"));
            return Ok(cmd);
        }
        if provider == "opencode"
            && path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("opencode.cmd"))
        {
            let native = path
                .parent()
                .ok_or(HostError("invalid_path"))?
                .join("node_modules")
                .join("@opencode")
                .join("cli")
                .join("bin")
                .join("opencode.exe");
            if native.is_file() {
                return Ok(Command::new(native));
            }
        }
    }
    let _ = provider;
    Ok(Command::new(executable))
}

#[cfg(test)]
pub(super) fn message(provider: &str, output: &[u8]) -> Option<String> {
    message_lines(provider, lines(Cursor::new(output)))
}
fn message_lines(provider: &str, output: impl Iterator<Item = Vec<u8>>) -> Option<String> {
    if provider == "antigravity" {
        return antigravity_receipt_lines(output).map(|r| r.2);
    }
    let mut text = String::new();
    for line in output {
        let v: Value = serde_json::from_slice(&line).ok()?;
        let part = match provider {
            "cursor" if v["type"] == "result" => v["result"].as_str(),
            "opencode" if v["type"] == "text" => v["part"]["text"].as_str(),
            _ => None,
        };
        if let Some(part) = part {
            if !text.is_empty() {
                text.push('\n');
            }
            if text.len() + part.len() > MAX_RECORD {
                return None;
            }
            text.push_str(part);
        }
    }
    (!text.is_empty()).then_some(text)
}

// The observed stream has one init and one matching successful result. The
// native conversation ID is allocated by agy, not the host's session UUID.
#[cfg(test)]
fn antigravity_receipt(output: &[u8]) -> Option<(String, Option<String>, String)> {
    antigravity_receipt_lines(lines(Cursor::new(output)))
}
fn antigravity_receipt_lines(
    output: impl Iterator<Item = Vec<u8>>,
) -> Option<(String, Option<String>, String)> {
    let mut session: Option<String> = None;
    let mut result: Option<(String, Option<String>, String)> = None;
    for line in output {
        let v: Value = serde_json::from_slice(&line).ok()?;
        let event = v.get("event")?.as_str()?;
        if result.is_some() {
            return None;
        }
        match event {
            "init" => {
                if session.is_some() {
                    return None;
                }
                let id = v["conversation_id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)?;
                session = Some(id.to_owned());
            }
            "result" => {
                let id = session.as_deref()?;
                let terminal = v.get("result")?.as_object()?;
                if terminal.get("conversation_id")?.as_str()? != id
                    || terminal.get("status")?.as_str()? != "SUCCESS"
                {
                    return None;
                }
                let response = terminal.get("response")?.as_str()?.to_owned();
                let model = terminal
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .map(str::to_owned);
                result = Some((id.to_owned(), model, response));
            }
            "error" | "cancelled" | "canceled" => return None,
            _ => {
                let id = session.as_deref()?;
                if v.get("conversation_id")
                    .is_some_and(|x| x.as_str() != Some(id))
                {
                    return None;
                }
                if event == "step_update" {
                    let step = v.get("step_update")?.as_object()?;
                    if step.get("conversation_id").and_then(Value::as_str) != Some(id) {
                        return None;
                    }
                    if step
                        .get("state")
                        .and_then(Value::as_str)
                        .is_some_and(|s| ["ERROR", "FAILED", "CANCELLED", "CANCELED"].contains(&s))
                    {
                        return None;
                    }
                }
            }
        }
    }
    result
}
#[cfg(test)]
pub(super) fn antigravity_session(output: &[u8]) -> Option<String> {
    antigravity_receipt(output).map(|r| r.0)
}

fn native_receipt_lines(
    provider: &str,
    output: impl Iterator<Item = Vec<u8>>,
) -> (bool, Option<String>) {
    let mut session: Option<String> = None;
    let mut init = false;
    let mut terminal = false;
    let mut model = None;
    let mut valid = true;
    let mut projected_text = false;
    for line in output {
        let Ok(v) = serde_json::from_slice::<Value>(&line) else {
            return (false, None);
        };
        let Some(kind) = v["type"].as_str() else {
            return (false, None);
        };
        if terminal && provider == "cursor" {
            valid = false;
        }
        let key = if provider == "cursor" {
            "session_id"
        } else {
            "sessionID"
        };
        if provider == "opencode" && v.get(key).and_then(Value::as_str).is_none_or(str::is_empty) {
            valid = false;
        }
        if let Some(id) = v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            if session.as_deref().is_some_and(|previous| previous != id) {
                valid = false;
            }
            session = Some(id.to_owned());
        }
        if provider == "cursor" {
            match kind {
                "system" if v["subtype"] == "init" => {
                    if init {
                        valid = false;
                    }
                    init = true;
                    model = v["model"]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 256)
                        .map(str::to_owned);
                }
                "result" => {
                    terminal = true;
                    if !init || v["subtype"] != "success" || v["is_error"] != false {
                        valid = false;
                    }
                }
                "error" => valid = false,
                _ => {
                    if !init {
                        valid = false;
                    }
                }
            }
        } else {
            match kind {
                "step_start" => {
                    init = true;
                    projected_text = false;
                    terminal = false;
                }
                "step_finish" => {
                    if !init {
                        valid = false;
                    }
                    // Tool-call steps can be followed by another step; only a stop completes the turn.
                    terminal = v["part"]["reason"] == "stop";
                    if !terminal {
                        projected_text = false;
                    }
                }
                "error" => valid = false,
                "text" => {
                    if v["part"]["text"].as_str().is_none_or(str::is_empty) {
                        valid = false;
                    } else {
                        projected_text = true;
                    }
                }
                "tool_use" => {
                    projected_text = false;
                    terminal = false;
                    if !v["part"].is_object() {
                        valid = false;
                    }
                }
                _ => {
                    if !init {
                        valid = false;
                    }
                }
            }
        }
    }
    let completed = if provider == "opencode" {
        projected_text || (init && terminal)
    } else {
        init && terminal
    };
    (valid && completed && session.is_some(), model)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_schema_is_optional_and_user_config_is_preserved() {
        for schema in [None, Some(Path::new("schema.json"))] {
            let cmd = command(
                "codex",
                "codex",
                "model",
                "default",
                "s",
                Path::new("message"),
                Path::new("."),
                "-",
                schema,
            )
            .unwrap();
            let args: Vec<_> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect();
            assert!(!args.iter().any(|a| a == "--ignore-user-config"));
            assert!(
                args.windows(2)
                    .any(|a| a == ["--sandbox", "workspace-write"])
            );
            assert!(args.iter().any(|a| a == "approval_policy=\"never\""));
            assert_eq!(
                args.iter().any(|a| a == "--output-schema"),
                schema.is_some()
            );
            if schema.is_some() {
                assert!(
                    args.windows(2)
                        .any(|a| a == ["--output-schema", "schema.json"])
                );
            }
            assert_eq!(args.last().unwrap(), "-");
        }
    }
    #[test]
    fn claude_loads_user_configuration_and_allows_build_tests_without_bypass() {
        let cmd = command(
            "claude",
            "claude",
            "model",
            "default",
            "s",
            Path::new("message"),
            Path::new("."),
            "prompt",
            None,
        )
        .unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        for omitted in [
            "--restricted",
            "--safe-mode",
            "--strict-mcp-config",
            "--dangerously-skip-permissions",
            "--tools",
        ] {
            assert!(!args.iter().any(|a| a == omitted));
        }
        assert!(args.iter().any(|a| a == "Bash(cargo test *)"));
        assert!(
            args.windows(2)
                .any(|a| a == ["--permission-prompts", "none"])
        );
    }
    #[test]
    fn codex_startup_diagnostics_do_not_hide_turn_failures_or_bad_order() {
        let start = "{\"type\":\"thread.started\",\"thread_id\":\"s\"}\n";
        let diagnostic = "{\"type\":\"item.completed\",\"item\":{\"type\":\"error\",\"message\":\"MCP startup failed\"}}\n";
        let turn = "{\"type\":\"turn.started\"}\n{\"type\":\"turn.completed\"}\n";
        let valid = format!("{start}{diagnostic}{diagnostic}{turn}");
        let file = tempfile::NamedTempFile::new().unwrap();
        let check = |stream: &str| {
            std::fs::write(file.path(), stream).unwrap();
            let slice = verified_receipt("codex", stream.as_bytes(), "").0;
            assert_eq!(
                verified_receipt_file("codex", file.path(), "").unwrap().0,
                slice
            );
            slice
        };
        assert!(check(&valid));
        assert!(!check(&format!("{diagnostic}{start}{turn}")));
        assert!(!check(&format!(
            "{start}{}{turn}",
            diagnostic.replace("item.completed", "item.started")
        )));
        assert!(!check(&format!(
            "{start}{}{turn}",
            diagnostic.replace("\"error\"", "\"agent_message\"")
        )));
        assert!(!check(&format!("{start}{{\"type\":\"error\"}}\n{turn}")));
        assert!(!check(&format!(
            "{start}{diagnostic}{{\"type\":\"turn.started\"}}\n{{\"type\":\"turn.failed\"}}\n"
        )));
        assert!(!check(&format!(
            "{start}{diagnostic}{{\"type\":\"turn.completed\"}}\n"
        )));
    }
    #[test]
    fn replay_codex_startup_diagnostics_when_fixture_is_supplied() {
        if let Some(path) = std::env::var_os("SWITCHBITE_RECEIPT_REPLAY") {
            assert!(
                verified_receipt_file("codex", Path::new(&path), "")
                    .unwrap()
                    .0
            );
        }
    }
    #[test]
    fn file_receipts_handle_large_stream_and_errors() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "{{\"type\":\"thread.started\",\"thread_id\":\"s\"}}").unwrap();
        writeln!(file, "{{\"type\":\"turn.started\"}}").unwrap();
        for _ in 0..20000 {
            writeln!(
                file,
                "{{\"type\":\"item.completed\",\"payload\":\"{}\"}}",
                "x".repeat(100)
            )
            .unwrap();
        }
        writeln!(file, "{{\"type\":\"turn.completed\"}}").unwrap();
        assert!(verified_receipt_file("codex", file.path(), "s").unwrap().0);
        writeln!(file, "{{\"type\":\"error\"}}").unwrap();
        assert!(!verified_receipt_file("codex", file.path(), "s").unwrap().0);
        assert!(
            verified_receipt_file("codex", &file.path().with_extension("missing"), "s").is_err()
        );
    }
    #[test]
    fn receipt_ignores_large_single_command_output_record() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "{{\"type\":\"thread.started\",\"thread_id\":\"s\"}}\n{{\"type\":\"turn.started\"}}"
        )
        .unwrap();
        write!(
            file,
            "{{\"type\":\"item.completed\",\"item\":{{\"aggregated_output\":\""
        )
        .unwrap();
        for _ in 0..17 {
            file.write_all(&vec![b'x'; 1024 * 1024]).unwrap();
        }
        writeln!(file, "\"}}}}\n{{\"type\":\"turn.completed\"}}").unwrap();
        assert!(verified_receipt_file("codex", file.path(), "s").unwrap().0);
    }
    #[test]
    fn file_receipts_match_native_memory_receipts() {
        let samples: [(&str, &str, &str); 4] = [
            (
                "claude",
                "s",
                "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\",\"model\":\"m\"}\n{\"type\":\"result\",\"session_id\":\"s\",\"subtype\":\"success\",\"is_error\":false,\"permission_denials\":[]}\n",
            ),
            (
                "cursor",
                "",
                "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}\n{\"type\":\"result\",\"session_id\":\"s\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\"}\n",
            ),
            (
                "opencode",
                "",
                "{\"type\":\"step_start\",\"sessionID\":\"s\"}\n{\"type\":\"text\",\"sessionID\":\"s\",\"part\":{\"text\":\"ok\"}}\n",
            ),
            (
                "antigravity",
                "",
                "{\"event\":\"init\",\"conversation_id\":\"s\"}\n{\"event\":\"result\",\"result\":{\"conversation_id\":\"s\",\"status\":\"SUCCESS\",\"response\":\"ok\"}}\n",
            ),
        ];
        let file = tempfile::NamedTempFile::new().unwrap();
        for (provider, session, sample) in samples {
            std::fs::write(file.path(), sample).unwrap();
            assert_eq!(
                verified_receipt_file(provider, file.path(), session).unwrap(),
                verified_receipt(provider, sample.as_bytes(), session)
            );
            assert_eq!(
                message_file(provider, file.path()).unwrap(),
                message(provider, sample.as_bytes())
            );
            std::fs::write(file.path(), format!("{sample}invalid-json\n")).unwrap();
            assert!(
                !verified_receipt_file(provider, file.path(), session)
                    .unwrap()
                    .0
            );
        }
    }
    #[test]
    fn antigravity_observed_receipt_requires_matching_successful_terminal() {
        // Derived from the observed successful stream; identifiers and response are sanitized.
        let success = b"{\"event\":\"init\",\"conversation_id\":\"s\",\"init\":{}}\n{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"s\",\"step_index\":1,\"state\":\"DONE\",\"step_type\":\"agent_response\",\"text_delta\":\"OK\"}}\n{\"event\":\"result\",\"result\":{\"conversation_id\":\"s\",\"status\":\"SUCCESS\",\"response\":\"OK\\n\"}}\n";
        assert_eq!(
            verified_receipt("antigravity", success, "host-session"),
            (true, None)
        );
        assert_eq!(antigravity_session(success).as_deref(), Some("s"));
        assert_eq!(message("antigravity", success).as_deref(), Some("OK\n"));
        // Synthetic failure, incomplete, foreign-session and contradictory-terminal streams.
        let input = String::from_utf8(success.to_vec()).unwrap();
        assert!(
            !verified_receipt(
                "antigravity",
                input.replace("SUCCESS", "ERROR").as_bytes(),
                ""
            )
            .0
        );
        assert!(
            !verified_receipt(
                "antigravity",
                input
                    .lines()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .as_bytes(),
                ""
            )
            .0
        );
        assert!(
            !verified_receipt(
                "antigravity",
                input
                    .replace(
                        "\"conversation_id\":\"s\",\"status\"",
                        "\"conversation_id\":\"foreign\",\"status\""
                    )
                    .as_bytes(),
                ""
            )
            .0
        );
        assert!(
            !verified_receipt(
                "antigravity",
                format!("{input}{{\"event\":\"error\"}}\n").as_bytes(),
                ""
            )
            .0
        );
    }
    #[test]
    fn antigravity_command_uses_sandbox_and_checked_effort() {
        let cmd = command(
            "antigravity",
            "agy",
            "model",
            "high",
            "ignored",
            Path::new("message"),
            Path::new("."),
            "prompt",
            None,
        )
        .unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            args,
            [
                "--print",
                "prompt",
                "--output-format",
                "stream-json",
                "--model",
                "model",
                "--sandbox",
                "--mode",
                "accept-edits",
                "--effort",
                "high"
            ]
        );
        assert!(
            command(
                "antigravity",
                "agy",
                "model",
                "invalid",
                "",
                Path::new("message"),
                Path::new("."),
                "prompt",
                None
            )
            .is_err()
        );
    }
    #[test]
    fn native_receipts_distinguish_recovered_tool_errors_from_turn_failures() {
        let cursor = b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\",\"model\":\"m\"}\n{\"type\":\"tool_call\",\"session_id\":\"s\",\"subtype\":\"completed\"}\n{\"type\":\"result\",\"session_id\":\"s\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"done\"}\n";
        assert_eq!(
            verified_receipt("cursor", cursor, "unused"),
            (true, Some("m".into()))
        );
        assert_eq!(message("cursor", cursor).as_deref(), Some("done"));
        let open = b"{\"type\":\"step_start\",\"sessionID\":\"s\"}\n{\"type\":\"tool_use\",\"sessionID\":\"s\",\"part\":{\"state\":{\"status\":\"error\"}}}\n{\"type\":\"step_finish\",\"sessionID\":\"s\",\"part\":{\"reason\":\"tool-calls\"}}\n{\"type\":\"step_start\",\"sessionID\":\"s\"}\n{\"type\":\"text\",\"sessionID\":\"s\",\"part\":{\"text\":\"done\"}}\n{\"type\":\"step_finish\",\"sessionID\":\"s\",\"part\":{\"reason\":\"stop\"}}\n";
        assert!(verified_receipt("opencode", open, "unused").0);
        assert_eq!(message("opencode", open).as_deref(), Some("done"));
        let mut failed = open.to_vec();
        failed.extend_from_slice(b"{\"type\":\"error\",\"sessionID\":\"s\"}\n");
        assert!(!verified_receipt("opencode", &failed, "unused").0);
        assert!(
            !verified_receipt(
                "opencode",
                b"{\"type\":\"step_start\",\"sessionID\":\"s\"}\n",
                "unused"
            )
            .0
        );
    }
    #[test]
    fn effort_default_is_omitted_and_unknown_effort_rejected() {
        let path = Path::new("message");
        let dir = Path::new(".");
        for provider in ["claude", "codex", "cursor", "opencode"] {
            let cmd = command(
                provider, "test", "model", "default", "session", path, dir, "prompt", None,
            )
            .unwrap();
            assert!(
                !cmd.get_args()
                    .any(|a| a.to_string_lossy().contains("effort"))
            );
            if ["claude", "codex"].contains(&provider) {
                assert!(
                    command(
                        provider, "test", "model", "invalid", "session", path, dir, "prompt", None
                    )
                    .is_err()
                );
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn cursor_launcher_resolves_latest_native_version() {
        let temp = tempfile::tempdir().unwrap();
        for version in [
            "2026.9.8-abcdef0",
            "2026.10.22-abcdef1",
            "2026.10.22-abcdef1.previous",
        ] {
            let dir = temp.path().join("versions").join(version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("node.exe"), b"").unwrap();
            std::fs::write(dir.join("index.js"), b"").unwrap();
        }
        let launcher = temp.path().join("cursor-agent.cmd");
        let cmd = native_command("cursor", launcher.to_str().unwrap()).unwrap();
        assert_eq!(
            cmd.get_program(),
            temp.path().join("versions/2026.10.22-abcdef1/node.exe")
        );
        assert_eq!(
            cmd.get_args().next().unwrap(),
            temp.path().join("versions/2026.10.22-abcdef1/index.js")
        );
    }
    #[test]
    fn opencode_projected_output_requires_text_and_consistent_session() {
        let projected = b"{\"type\":\"tool_use\",\"sessionID\":\"s\",\"part\":{}}\n{\"type\":\"text\",\"sessionID\":\"s\",\"part\":{\"text\":\"done\"}}\n";
        assert!(verified_receipt("opencode", projected, "unused").0);
        let mut error = projected.to_vec();
        error.extend_from_slice(b"{\"type\":\"error\",\"sessionID\":\"s\"}\n");
        assert!(!verified_receipt("opencode", &error, "unused").0);
        let mut foreign = projected.to_vec();
        foreign.extend_from_slice(
            b"{\"type\":\"text\",\"sessionID\":\"other\",\"part\":{\"text\":\"x\"}}\n",
        );
        assert!(!verified_receipt("opencode", &foreign, "unused").0);
        assert!(
            !verified_receipt(
                "opencode",
                b"{\"type\":\"text\",\"sessionID\":\"s\",\"part\":{}}\n",
                "unused"
            )
            .0
        );
        assert!(
            !verified_receipt(
                "opencode",
                b"{\"type\":\"tool_use\",\"sessionID\":\"s\",\"part\":{}}\n",
                "unused"
            )
            .0
        );
    }
    #[test]
    fn newer_efforts_are_forwarded() {
        for (provider, effort) in [
            ("codex", "none"),
            ("codex", "max"),
            ("codex", "ultra"),
            ("claude", "xhigh"),
        ] {
            let cmd = command(
                provider,
                "test",
                "model",
                effort,
                "session",
                Path::new("message"),
                Path::new("."),
                "prompt",
                None,
            )
            .unwrap();
            assert!(cmd.get_args().any(|a| a.to_string_lossy().contains(effort)));
        }
    }
    #[cfg(windows)]
    #[test]
    fn cursor_resolves_a_single_unpadded_version() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("versions/2026.9.8-abcdef0");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.exe"), b"").unwrap();
        std::fs::write(dir.join("index.js"), b"").unwrap();
        let cmd = native_command(
            "cursor",
            temp.path().join("cursor-agent.cmd").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(cmd.get_program(), dir.join("node.exe"));
    }
    #[test]
    fn opencode_final_text_after_tool_steps_completes_successful_process() {
        let stream = b"{\"type\":\"step_start\",\"sessionID\":\"s\"}\n{\"type\":\"tool_use\",\"sessionID\":\"s\",\"part\":{}}\n{\"type\":\"step_finish\",\"sessionID\":\"s\",\"part\":{\"reason\":\"tool-calls\"}}\n{\"type\":\"step_start\",\"sessionID\":\"s\"}\n{\"type\":\"tool_use\",\"sessionID\":\"s\",\"part\":{}}\n{\"type\":\"step_finish\",\"sessionID\":\"s\",\"part\":{\"reason\":\"tool-calls\"}}\n{\"type\":\"step_start\",\"sessionID\":\"s\"}\n{\"type\":\"text\",\"sessionID\":\"s\",\"part\":{\"text\":\"Created artifact.\"}}\n";
        assert!(verified_receipt("opencode", stream, "unused").0);
        let mut followed_by_tool = stream.to_vec();
        followed_by_tool
            .extend_from_slice(b"{\"type\":\"tool_use\",\"sessionID\":\"s\",\"part\":{}}\n");
        assert!(!verified_receipt("opencode", &followed_by_tool, "unused").0);
        let mut error = stream.to_vec();
        error.extend_from_slice(b"{\"type\":\"error\",\"sessionID\":\"s\"}\n");
        assert!(!verified_receipt("opencode", &error, "unused").0);
    }
}
