//! Render portable MCP setup snippets without reading or writing configuration.
//!
//! All clients launch the typed CLI profile. Online snippets either forward the
//! named variable or use the client's normal process environment inheritance;
//! this module never reads its value. Offline snippets contain no key reference.
use serde_json::json;
use std::{error::Error, fmt, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    UnsupportedClient,
    InvalidExecutable,
    Encoding,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedClient => "Unsupported MCP client.",
            Self::InvalidExecutable => {
                "Executable must be an absolute Unicode path without control characters."
            }
            Self::Encoding => "Unable to encode MCP configuration.",
        })
    }
}

impl Error for ConfigError {}

/// Return a config snippet only; the caller decides where to display or save it.
///
/// Paths need not exist on this machine. No filesystem, environment, network,
/// shell interpolation, or client process is involved in rendering.
/// Codex/Grok use TOML; Claude/Cursor/Antigravity/generic use `mcpServers` JSON;
/// OpenCode uses the version 2 `mcp.servers` JSON structure.
pub fn render(client: &str, executable: &Path, offline: bool) -> Result<String, ConfigError> {
    let mut args = vec!["serve", "--profile", "workflow"];
    if offline {
        args.push("--offline");
    }
    render_launch(client, executable, &args, !offline)
}

/// Render an explicit launch contract. Local judges pass `forward_typesafe = false`.
/// This function never reads environment values.
pub fn render_launch(
    client: &str,
    executable: &Path,
    args: &[&str],
    forward_typesafe: bool,
) -> Result<String, ConfigError> {
    render_named_launch(client, executable, args, forward_typesafe, "switchbite")
}

/// A separate server name keeps project docking independent of user-level servers.
/// Only portable MCP names are accepted; no credential value is ever read.
pub fn render_named_launch(
    client: &str,
    executable: &Path,
    args: &[&str],
    forward_typesafe: bool,
    server_name: &str,
) -> Result<String, ConfigError> {
    if server_name.is_empty()
        || server_name.len() > 64
        || !server_name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(ConfigError::Encoding);
    }
    let executable_text = executable.to_str().ok_or(ConfigError::InvalidExecutable)?;
    if !executable.is_absolute()
        || executable_text.chars().any(char::is_control)
        || args.iter().any(|arg| arg.chars().any(char::is_control))
    {
        return Err(ConfigError::InvalidExecutable);
    }

    match client {
        "codex" | "grok" => {
            // JSON string syntax is a subset of TOML basic string syntax here:
            // control characters are rejected above, leaving quotes/backslashes.
            let command =
                serde_json::to_string(executable_text).map_err(|_| ConfigError::Encoding)?;
            let arguments = serde_json::to_string(&args).map_err(|_| ConfigError::Encoding)?;
            let mut snippet =
                format!("[mcp_servers.{server_name}]\ncommand = {command}\nargs = {arguments}\n");
            if client == "codex" {
                if forward_typesafe {
                    snippet.push_str("env_vars = [\"TYPESAFE_API_KEY\"]\n");
                }
                snippet.push_str("startup_timeout_sec = 15\ntool_timeout_sec = 45\n");
            } else {
                // Observed output of Grok 1.0.60 `mcp add --scope project`.
                snippet.push_str("enabled = true\n");
            }
            Ok(snippet)
        }
        "opencode" => {
            let mut command = vec![executable_text];
            command.extend(args.iter().copied());
            pretty(json!({"mcp": {"servers": {(server_name): {
                "type": "local", "command": command
            }}}}))
        }
        "claude" | "cursor" | "antigravity" | "generic" => {
            let mut server = json!({"type": "stdio", "command": executable_text, "args": args});
            if client == "claude" && forward_typesafe {
                // Claude expands this reference when it loads the configuration.
                server["env"] = json!({"TYPESAFE_API_KEY": "${TYPESAFE_API_KEY:-}"});
            } else if client == "cursor" && forward_typesafe {
                // Cursor's documented variable syntax differs from Claude's.
                server["env"] = json!({"TYPESAFE_API_KEY": "${env:TYPESAFE_API_KEY}"});
            }
            pretty(json!({"mcpServers": {(server_name): server}}))
        }
        _ => Err(ConfigError::UnsupportedClient),
    }
}

fn pretty(value: serde_json::Value) -> Result<String, ConfigError> {
    serde_json::to_string_pretty(&value)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|_| ConfigError::Encoding)
}
