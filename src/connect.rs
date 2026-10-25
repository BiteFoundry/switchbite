//! Explicit, native CLI configuration writes. Never called by MCP tools.
//! Judge selection and credentials are deliberately outside this module.
use crate::connection;
use jsonc_parser::cst::{CstInputValue, CstNode, CstObject, CstRootNode};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt, fs,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const CLIENTS: &[&str] = &[
    "codex",
    "claude",
    "cursor",
    "grok",
    "antigravity",
    "opencode",
];
const MAX_CONFIG: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectError {
    UnsupportedClient,
    InvalidConfig,
    Conflict,
    Io,
    Busy,
    Changed,
    Override,
    Environment,
}
impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedClient => "Unsupported client; use codex, claude, cursor, grok, antigravity or opencode.",
            Self::InvalidConfig => "Configuration is invalid, ambiguous, too large or has an incompatible structure. No settings changed.",
            Self::Conflict => "The switchbite name belongs to a different server or transport. No settings changed; use config --client for manual setup.",
            Self::Io => "Unable to read or save connection files. Check file permissions; the original configuration is preserved on write failure.",
            Self::Busy => "Another connection update is active. Retry after it finishes; if interrupted, check the adjacent .switchbite-lock file.",
            Self::Changed => "Configuration changed during setup. No settings replaced; retry the command.",
            Self::Override => "An OpenCode configuration override needs explicit source selection. Update the override at its source, or use --config-file with its effective file. No settings changed.",
            Self::Environment => "Unable to resolve an absolute user configuration or application-data directory.",
        })
    }
}
impl std::error::Error for ConnectError {}

#[derive(Debug, Serialize)]
pub struct Report {
    pub client: String,
    pub status: &'static str,
    pub configuration: PathBuf,
    pub executable: PathBuf,
    pub backup: Option<PathBuf>,
    pub judge_configuration: &'static str,
    pub next_action: &'static str,
}

/// Resolve only documented user-level locations; never enumerate settings values.
pub fn config_path(client: &str, home: &Path) -> Result<PathBuf, ConnectError> {
    let env_path = |name| {
        std::env::var_os(name)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    let path = match client {
        "codex" => env_path("CODEX_HOME")
            .unwrap_or_else(|| home.join(".codex"))
            .join("config.toml"),
        "claude" => env_path("CLAUDE_CONFIG_DIR")
            .unwrap_or_else(|| home.to_path_buf())
            .join(".claude.json"),
        "cursor" => home.join(".cursor/mcp.json"),
        "grok" => home.join(".grok/config.toml"),
        "antigravity" => home.join(".gemini/config/mcp_config.json"),
        "opencode" => {
            if ["OPENCODE_CLI_CONFIG_CONTENT", "OPENCODE_CONFIG_CONTENT"]
                .iter()
                .any(|n| std::env::var_os(n).is_some())
            {
                return Err(ConnectError::Override);
            }
            if let Some(path) = env_path("OPENCODE_CONFIG") {
                return absolute(path);
            }
            // v2 does not document the legacy directory override's precedence.
            // Do not silently write a global file that may be shadowed.
            if env_path("OPENCODE_CONFIG_DIR").is_some() {
                return Err(ConnectError::Override);
            }
            let dir = env_path("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("opencode");
            // OpenCode reads JSONC after JSON; update the higher-precedence file.
            if dir.join("opencode.jsonc").exists() {
                dir.join("opencode.jsonc")
            } else {
                dir.join("opencode.json")
            }
        }
        _ => return Err(ConnectError::UnsupportedClient),
    };
    absolute(path)
}
fn absolute(path: PathBuf) -> Result<PathBuf, ConnectError> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(ConnectError::Environment)
    }
}

fn input(v: &Value) -> CstInputValue {
    match v {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(input).collect()),
        Value::Object(o) => {
            CstInputValue::Object(o.iter().map(|(k, v)| (k.clone(), input(v))).collect())
        }
    }
}
fn validate_json(node: &CstNode, depth: usize) -> Result<(), ConnectError> {
    if depth > 64 {
        return Err(ConnectError::InvalidConfig);
    }
    if let Some(object) = node.as_object() {
        let mut seen = BTreeSet::new();
        for prop in object.properties() {
            if !seen.insert(prop.decoded_name().ok_or(ConnectError::InvalidConfig)?) {
                return Err(ConnectError::InvalidConfig);
            }
            validate_json(&prop.value().ok_or(ConnectError::InvalidConfig)?, depth + 1)?;
        }
    } else if let Some(array) = node.as_array() {
        for item in array.elements() {
            validate_json(&item, depth + 1)?;
        }
    }
    Ok(())
}
fn set(object: &CstObject, key: &str, v: &Value) {
    match object.get(key) {
        Some(prop) => {
            if prop.value().and_then(|n| n.to_serde_value()).as_ref() != Some(v) {
                prop.set_value(input(v));
            }
        }
        None => {
            object.append(key, input(v));
        }
    }
}
fn ours(command: &str) -> bool {
    // Also recognize Windows paths when examining fixtures on another platform.
    command
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|s| s.eq_ignore_ascii_case("switchbite.exe") || s == "switchbite")
}
fn compatible(value: &Value, client: &str) -> bool {
    let command = if client == "opencode" {
        value
            .get("command")
            .and_then(|v| v.get(0))
            .and_then(Value::as_str)
    } else {
        value.get("command").and_then(Value::as_str)
    };
    command.is_some_and(ours)
        && value.get("url").is_none()
        && value.get("type").is_none_or(|t| {
            t == if client == "opencode" {
                "local"
            } else {
                "stdio"
            }
        })
}

/// Pure, format-preserving merge. Never reads credentials or changes a judge.
pub fn merge(
    client: &str,
    existing: &str,
    executable: &Path,
    offline: bool,
) -> Result<String, ConnectError> {
    let snippet = connection::render(client, executable, offline).map_err(config_error)?;
    merge_snippet(client, existing, &snippet, false)
}

fn config_error(error: connection::ConfigError) -> ConnectError {
    match error {
        connection::ConfigError::UnsupportedClient => ConnectError::UnsupportedClient,
        _ => ConnectError::InvalidConfig,
    }
}

fn typesafe_key(name: &str) -> bool {
    // Windows environment names are case-insensitive. Remove equivalent grants
    // on every platform so a portable config cannot reintroduce the cloud key.
    name.eq_ignore_ascii_case("TYPESAFE_API_KEY")
}

/// Use the same ownership and format-preserving merge for an explicit local launch.
/// Remove the cloud judge's explicit credential grants when changing to a local judge.
pub fn merge_launch(
    client: &str,
    existing: &str,
    executable: &Path,
    args: &[&str],
) -> Result<String, ConnectError> {
    let snippet =
        connection::render_launch(client, executable, args, false).map_err(config_error)?;
    merge_snippet(client, existing, &snippet, true)
}

fn merge_snippet(
    client: &str,
    existing: &str,
    snippet: &str,
    local_judge: bool,
) -> Result<String, ConnectError> {
    if existing.len() as u64 > MAX_CONFIG {
        return Err(ConnectError::InvalidConfig);
    }
    if matches!(client, "codex" | "grok") {
        let mut document = existing
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| ConnectError::InvalidConfig)?;
        let desired = snippet
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| ConnectError::InvalidConfig)?;
        if document.get("mcp_servers").is_none() {
            document["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let servers = document["mcp_servers"]
            .as_table_like_mut()
            .ok_or(ConnectError::InvalidConfig)?;
        if let Some(old) = servers.get("switchbite") {
            let table = old.as_table_like().ok_or(ConnectError::InvalidConfig)?;
            if !table
                .get("command")
                .and_then(toml_edit::Item::as_str)
                .is_some_and(ours)
                || table.get("url").is_some()
            {
                return Err(ConnectError::Conflict);
            }
        } else {
            servers.insert(
                "switchbite",
                toml_edit::Item::Table(toml_edit::Table::new()),
            );
        }
        let table = servers
            .get_mut("switchbite")
            .and_then(toml_edit::Item::as_table_like_mut)
            .ok_or(ConnectError::InvalidConfig)?;
        if local_judge {
            if let Some(grants) = table.get_mut("env_vars") {
                let grants = grants.as_array_mut().ok_or(ConnectError::InvalidConfig)?;
                grants.retain(|value| {
                    !value.as_str().is_some_and(typesafe_key)
                        && !value
                            .as_inline_table()
                            .and_then(|entry| entry.get("name"))
                            .and_then(toml_edit::Value::as_str)
                            .is_some_and(typesafe_key)
                });
            }
            if let Some(env) = table.get_mut("env") {
                let env = env.as_table_like_mut().ok_or(ConnectError::InvalidConfig)?;
                let keys: Vec<_> = env
                    .iter()
                    .filter(|(key, _)| typesafe_key(key))
                    .map(|(key, _)| key.to_owned())
                    .collect();
                for key in keys {
                    env.remove(&key);
                }
            }
        }
        for (key, value) in desired["mcp_servers"]["switchbite"]
            .as_table()
            .ok_or(ConnectError::InvalidConfig)?
            .iter()
        {
            if key == "env_vars" {
                // Keep any additional, explicitly configured variable grants.
                if let Some(old) = table.get_mut(key) {
                    let array = old.as_array_mut().ok_or(ConnectError::InvalidConfig)?;
                    if !array.iter().any(|v| {
                        v.as_str() == Some("TYPESAFE_API_KEY")
                            || v.as_inline_table()
                                .and_then(|t| t.get("name"))
                                .and_then(toml_edit::Value::as_str)
                                == Some("TYPESAFE_API_KEY")
                    }) {
                        array.push("TYPESAFE_API_KEY");
                    }
                    continue;
                }
            }
            let mut replacement = value.clone();
            if let (Some(old), Some(new)) = (
                table.get(key).and_then(toml_edit::Item::as_value),
                replacement.as_value_mut(),
            ) {
                *new.decor_mut() = old.decor().clone();
            }
            if table.get(key).map(ToString::to_string).as_deref() != Some(&replacement.to_string())
            {
                table.insert(key, replacement);
            }
        }
        if table.get("enabled").and_then(toml_edit::Item::as_bool) == Some(false) {
            table.insert("enabled", toml_edit::value(true));
        }
        let result = document.to_string();
        return if result == format!("{existing}\n") {
            Ok(existing.to_owned())
        } else {
            Ok(result)
        };
    }
    let root = CstRootNode::parse(existing.trim_start_matches('\u{feff}'), &Default::default())
        .map_err(|_| ConnectError::InvalidConfig)?;
    if let Some(value) = root.value() {
        validate_json(&value, 0)?;
    }
    let mut object = root
        .object_value_or_create()
        .ok_or(ConnectError::InvalidConfig)?;
    let desired: Value = serde_json::from_str(snippet).map_err(|_| ConnectError::InvalidConfig)?;
    let keys: &[&str] = if client == "opencode" {
        &["mcp", "servers"]
    } else {
        &["mcpServers"]
    };
    let mut server_value = &desired;
    for key in keys {
        object = object
            .object_value_or_create(key)
            .ok_or(ConnectError::InvalidConfig)?;
        if client == "opencode"
            && *key == "mcp"
            && object.get("servers").is_none()
            && !object.properties().is_empty()
        {
            // v1 uses flat `mcp.NAME` entries. Do not mix incompatible dialects.
            return Err(ConnectError::InvalidConfig);
        }
        server_value = &server_value[key];
    }
    if let Some(old) = object.get("switchbite")
        && !old
            .value()
            .and_then(|v| v.to_serde_value())
            .is_some_and(|v| compatible(&v, client))
    {
        return Err(ConnectError::Conflict);
    }
    let server = object
        .object_value_or_create("switchbite")
        .ok_or(ConnectError::InvalidConfig)?;
    if local_judge && let Some(property) = server.get("env") {
        let env = property
            .value()
            .and_then(|node| node.as_object())
            .ok_or(ConnectError::InvalidConfig)?;
        for property in env.properties() {
            if property
                .decoded_name()
                .is_some_and(|key| typesafe_key(&key))
            {
                property.remove();
            }
        }
    }
    for (key, value) in server_value["switchbite"]
        .as_object()
        .ok_or(ConnectError::InvalidConfig)?
    {
        if key == "env" {
            let env = server
                .object_value_or_create("env")
                .ok_or(ConnectError::InvalidConfig)?;
            for (name, reference) in value.as_object().ok_or(ConnectError::InvalidConfig)? {
                // Never replace an explicitly configured credential with a reference.
                if env.get(name).is_none() {
                    set(&env, name, reference);
                }
            }
        } else {
            set(&server, key, value);
        }
    }
    if server
        .get("disabled")
        .and_then(|p| p.value())
        .and_then(|n| n.to_serde_value())
        == Some(Value::Bool(true))
    {
        set(&server, "disabled", &Value::Bool(false));
    }
    if server
        .get("enabled")
        .and_then(|p| p.value())
        .and_then(|n| n.to_serde_value())
        == Some(Value::Bool(false))
    {
        set(&server, "enabled", &Value::Bool(true));
    }
    let result = root.to_string();
    Ok(if existing.starts_with('\u{feff}') {
        format!("\u{feff}{result}")
    } else {
        result
    })
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, ConnectError> {
    match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAX_CONFIG + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ConnectError::Io)?;
            if bytes.len() as u64 > MAX_CONFIG {
                return Err(ConnectError::InvalidConfig);
            }
            Ok(Some(bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ConnectError::Io),
    }
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn adjacent(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Transactional update: serialize writers, back up exact original bytes, stage
/// on the same volume, and rename only after checking for concurrent edits.
pub fn connect_to(
    client: &str,
    path: &Path,
    executable: &Path,
    offline: bool,
    dry_run: bool,
) -> Result<Report, ConnectError> {
    connect_to_inner(client, path, executable, dry_run, |text| {
        merge(client, text, executable, offline)
    })
}

/// Register an explicit local launch using the same lock, backup and atomic rename.
pub fn connect_launch_to(
    client: &str,
    path: &Path,
    executable: &Path,
    args: &[&str],
    dry_run: bool,
) -> Result<Report, ConnectError> {
    let mut report = connect_to_inner(client, path, executable, dry_run, |text| {
        merge_launch(client, text, executable, args)
    })?;
    report.judge_configuration = "local_laya";
    Ok(report)
}

/// Save an owned plugin descriptor through the connection transaction engine.
pub(crate) fn save_descriptor(
    path: &Path,
    executable: &Path,
    text: &str,
    dry_run: bool,
    validate: impl FnOnce(&str) -> Result<(), ConnectError>,
) -> Result<Report, ConnectError> {
    connect_to_inner("claude", path, executable, dry_run, |existing| {
        validate(existing)?;
        Ok(text.to_owned())
    })
}

fn connect_to_inner(
    client: &str,
    path: &Path,
    executable: &Path,
    dry_run: bool,
    merge: impl FnOnce(&str) -> Result<String, ConnectError>,
) -> Result<Report, ConnectError> {
    if !CLIENTS.contains(&client) {
        return Err(ConnectError::UnsupportedClient);
    }
    if !path.is_absolute() || !executable.is_absolute() {
        return Err(ConnectError::Environment);
    }
    let parent = path.parent().ok_or(ConnectError::Environment)?;
    let lock = if !dry_run {
        fs::create_dir_all(parent).map_err(|_| ConnectError::Io)?;
        let lock = adjacent(path, ".switchbite-lock");
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    ConnectError::Busy
                } else {
                    ConnectError::Io
                }
            })?;
        let guard = Temporary(lock);
        writeln!(handle, "{}", std::process::id()).map_err(|_| ConnectError::Io)?;
        Some(guard)
    } else {
        None
    };
    let original = read(path)?;
    let text = std::str::from_utf8(original.as_deref().unwrap_or_default())
        .map_err(|_| ConnectError::InvalidConfig)?;
    let updated = merge(text)?;
    let changed = original.as_deref() != Some(updated.as_bytes());
    let mut report = Report {
        client: client.to_owned(),
        status: if dry_run {
            "preview"
        } else if changed {
            "configured"
        } else {
            "already_configured"
        },
        configuration: path.to_owned(),
        executable: executable.to_owned(),
        backup: None,
        judge_configuration: "unchanged",
        next_action: if dry_run {
            "run_without_dry_run"
        } else {
            "restart_client"
        },
    };
    if changed && !dry_run {
        // Refuse to replace a symlink/reparse target; the operator can choose its real file.
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ConnectError::Io);
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ConnectError::Io)?
            .as_nanos();
        let stage = Temporary(adjacent(path, &format!(".switchbite-{nonce}.tmp")));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage.0)
            .map_err(|_| ConnectError::Io)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())
                .map_err(|_| ConnectError::Io)?;
        }
        file.write_all(updated.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| ConnectError::Io)?;
        drop(file);
        if let Some(bytes) = &original {
            let backup = adjacent(path, &format!(".switchbite-{nonce}.bak"));
            let mut copy = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&backup)
                .map_err(|_| ConnectError::Io)?;
            if let Ok(metadata) = fs::metadata(path) {
                copy.set_permissions(metadata.permissions())
                    .map_err(|_| ConnectError::Io)?;
            }
            copy.write_all(bytes)
                .and_then(|_| copy.sync_all())
                .map_err(|_| ConnectError::Io)?;
            report.backup = Some(backup);
        }
        if read(path)? != original {
            return Err(ConnectError::Changed);
        }
        fs::rename(&stage.0, path).map_err(|_| ConnectError::Io)?;
    }
    drop(lock);
    Ok(report)
}

/// Content-addressed executable survives moving/deleting the extracted ZIP.
/// No PATH, shell profile, worker pool or judge setting is modified.
pub fn managed_executable(
    source: &Path,
    data_dir: &Path,
    dry_run: bool,
) -> Result<PathBuf, ConnectError> {
    let bytes = fs::read(source).map_err(|_| ConnectError::Io)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let name = if cfg!(windows) {
        "switchbite.exe"
    } else {
        "switchbite"
    };
    let destination = data_dir.join("Switchbite/bin").join(&digest).join(name);
    if !dry_run {
        fs::create_dir_all(destination.parent().ok_or(ConnectError::Environment)?)
            .map_err(|_| ConnectError::Io)?;
        match fs::read(&destination) {
            Ok(existing) if existing == bytes => return Ok(destination),
            Ok(_) => return Err(ConnectError::Conflict),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ConnectError::Io),
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ConnectError::Io)?
            .as_nanos();
        let stage = Temporary(adjacent(&destination, &format!(".{nonce}.tmp")));
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage.0)
            .map_err(|_| ConnectError::Io)?;
        target
            .write_all(&bytes)
            .and_then(|_| target.sync_all())
            .map_err(|_| ConnectError::Io)?;
        fs::set_permissions(
            &stage.0,
            fs::metadata(source)
                .map_err(|_| ConnectError::Io)?
                .permissions(),
        )
        .map_err(|_| ConnectError::Io)?;
        drop(target);
        fs::rename(&stage.0, &destination).map_err(|_| ConnectError::Io)?;
    }
    Ok(destination)
}
