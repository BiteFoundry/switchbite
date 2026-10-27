use super::{HostError, configure_codex_shell, path_string};
use serde_json::Value;
use std::{path::Path, process::Command};

pub(super) fn verified_receipt(
    provider: &str,
    output: &[u8],
    session: &str,
) -> (bool, Option<String>) {
    if provider == "cursor" || provider == "opencode" {
        return native_receipt(provider, output);
    }
    let mut init = false;
    let mut turn = false;
    let mut terminal = false;
    let mut model = None;
    let mut valid = true;
    for line in output.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
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
                            .is_some_and(Vec::is_empty)
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

pub(super) fn command(
    provider: &str,
    executable: &str,
    model: &str,
    effort: &str,
    session: &str,
    message: &Path,
    dir: &Path,
    prompt: &str,
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
                "--ignore-user-config",
                "--ephemeral",
                "--output-last-message",
                path_string(message)?,
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "windows.sandbox=\"unelevated\"",
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
                "--restricted",
                "--safe-mode",
                "--no-session-persistence",
                "--disable-slash-commands",
                "--permission-mode",
                "acceptEdits",
                "--permission-prompts",
                "none",
                "--allowedTools",
                "Read",
                "Write",
                "Edit",
                "--tools",
                "Read,Write,Edit",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--settings",
                "{\"disableAllHooks\":true}",
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

pub(super) fn message(provider: &str, output: &[u8]) -> Option<String> {
    let mut text = String::new();
    for line in output.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let v: Value = serde_json::from_slice(line).ok()?;
        let part = match provider {
            "cursor" if v["type"] == "result" => v["result"].as_str(),
            "opencode" if v["type"] == "text" => v["part"]["text"].as_str(),
            _ => None,
        };
        if let Some(part) = part {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(part);
        }
    }
    (!text.is_empty()).then_some(text)
}

fn native_receipt(provider: &str, output: &[u8]) -> (bool, Option<String>) {
    let mut session: Option<String> = None;
    let mut init = false;
    let mut terminal = false;
    let mut model = None;
    let mut valid = true;
    let mut projected_text = false;
    let mut saw_step = false;
    for line in output.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
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
                    saw_step = true;
                    terminal = false;
                }
                "step_finish" => {
                    if !init {
                        valid = false;
                    }
                    // Tool-call steps can be followed by another step; only a stop completes the turn.
                    terminal = v["part"]["reason"] == "stop";
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
    let completed = if provider == "opencode" && !saw_step {
        projected_text
    } else {
        init && terminal
    };
    (valid && completed && session.is_some(), model)
}

#[cfg(test)]
mod tests {
    use super::*;
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
                provider, "test", "model", "default", "session", path, dir, "prompt",
            )
            .unwrap();
            assert!(
                !cmd.get_args()
                    .any(|a| a.to_string_lossy().contains("effort"))
            );
            if ["claude", "codex"].contains(&provider) {
                assert!(
                    command(
                        provider, "test", "model", "invalid", "session", path, dir, "prompt"
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
}
