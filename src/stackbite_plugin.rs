//! Project-scoped MCP docking for coordinators launched by Stackbite.
//!
//! Stackbite has no native plugin registry. This installs the coordinator CLI's
//! project MCP settings without modifying Stackbite, global settings or prompts.
use crate::{connect, contracts, local_laya::LayaConfig};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

const NEXT_ACTION: &str = "Start a new coordinator session in this workspace. Use switchbite-stackbite's switchbite_start with a goal or plan and factual worker pool, then switchbite_next to continue. The coordinator supplies plans, repairs and pool updates when requested; configured local workers can run through verification and integration in the background. Jev needs TYPESAFE_API_KEY in the coordinator environment; Laya needs its configured local server.";
const PLUGIN_KIND: &str = "switchbite.stackbite-coordinator-mcp";
pub const SERVER_NAME: &str = "switchbite-stackbite";

#[derive(Debug, Clone)]
pub enum JudgeConfig {
    Jev,
    Laya { endpoint: String, model: String },
}

impl JudgeConfig {
    fn launch(&self) -> Result<(Vec<String>, Value), InstallError> {
        match self {
            Self::Jev => Ok((
                ["serve", "--profile", "workflow", "--judge", "jev"]
                    .map(String::from)
                    .to_vec(),
                json!({"kind":"jev", "model":contracts::MODEL, "required_env":["TYPESAFE_API_KEY"], "fallback":"hold"}),
            )),
            Self::Laya { endpoint, model } => {
                LayaConfig::new(endpoint, model).map_err(|_| failure("Invalid local Laya endpoint or model; use an explicit loopback HTTP endpoint and model."))?;
                Ok((
                    [
                        "serve",
                        "--profile",
                        "workflow",
                        "--judge",
                        "laya",
                        "--endpoint",
                        endpoint,
                        "--model",
                        model,
                    ]
                    .map(String::from)
                    .to_vec(),
                    json!({"kind":"laya", "endpoint":endpoint, "model":model, "fallback":"hold"}),
                ))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub workspace: PathBuf,
    pub client: String,
    pub judge: JudgeConfig,
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct InstallReport {
    pub status: &'static str,
    pub workspace: PathBuf,
    pub integration: &'static str,
    pub executable: PathBuf,
    pub server: &'static str,
    pub judge: Value,
    pub clients: Vec<connect::Report>,
    pub descriptor: PathBuf,
    pub descriptor_backup: Option<PathBuf>,
    pub next_action: &'static str,
}

/// A later write can fail after an earlier client was configured. Preserve that
/// fact in both human-readable and JSON errors; never claim a full installation.
#[derive(Debug, Serialize)]
pub struct InstallError {
    pub status: &'static str,
    pub message: String,
    pub configured: Vec<connect::Report>,
}
impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if !self.configured.is_empty() {
            write!(f, " Completed client configurations:")?;
            for client in &self.configured {
                write!(f, " {} ({})", client.client, client.configuration.display())?;
            }
            write!(
                f,
                ". Remaining installation steps failed; rerun after resolving the error."
            )?;
        }
        Ok(())
    }
}
impl std::error::Error for InstallError {}

fn failure(message: impl Into<String>) -> InstallError {
    InstallError {
        status: "failed",
        message: message.into(),
        configured: Vec::new(),
    }
}

fn descriptor_is_ours(text: &str) -> Result<(), connect::ConnectError> {
    if text.is_empty() {
        return Ok(());
    }
    let value: Value =
        serde_json::from_str(text).map_err(|_| connect::ConnectError::InvalidConfig)?;
    if value.get("kind").and_then(Value::as_str) != Some(PLUGIN_KIND)
        || value.get("schema_version").and_then(Value::as_u64) != Some(1)
    {
        return Err(connect::ConnectError::Conflict);
    }
    Ok(())
}

/// Reject project config paths whose existing parent redirects outside the
/// selected workspace (including Windows junctions, resolved by canonicalize).
fn confined(workspace: &Path, path: &Path) -> Result<(), InstallError> {
    let mut ancestor = path;
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let resolved = fs::canonicalize(ancestor)
                    .map_err(|_| failure("Unable to resolve the project configuration path."))?;
                if !resolved.starts_with(workspace) {
                    return Err(failure(
                        "Project configuration redirects outside the selected workspace; no settings changed.",
                    ));
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| failure("Invalid project configuration path."))?;
            }
            Err(_) => return Err(failure("Unable to inspect project configuration path.")),
        }
    }
}

/// Install only project MCP settings and an explanatory descriptor. All selected
/// clients and the descriptor are preflighted before copying or registering an
/// executable. Dry runs do not create directories, locks, backups or files.
pub fn install(
    options: &InstallOptions,
    source_executable: &Path,
    managed_root: &Path,
) -> Result<InstallReport, InstallError> {
    // Validate before touching files, including for direct library callers.
    let (mut launch_args, judge) = options.judge.launch()?;
    let forward_typesafe = matches!(options.judge, JudgeConfig::Jev);
    if !options.workspace.is_absolute()
        || !source_executable.is_absolute()
        || !managed_root.is_absolute()
    {
        return Err(failure(
            "Workspace, executable and managed data paths must be absolute.",
        ));
    }
    let workspace = fs::canonicalize(&options.workspace)
        .map_err(|_| failure("Workspace must be an existing project directory."))?;
    if !workspace.is_dir() {
        return Err(failure("Workspace must be an existing project directory."));
    }
    if workspace.join("ARCHIVED.json").exists() {
        return Err(failure(
            "Workspace is archived. Restore it using ARCHIVED.json before installing.",
        ));
    }
    launch_args.push("--workspace".into());
    launch_args.push(
        options
            .workspace
            .to_str()
            .ok_or_else(|| failure("Workspace path must be valid Unicode."))?
            .to_owned(),
    );
    let clients: &[&str] = match options.client.as_str() {
        "codex" => &["codex"],
        "claude" => &["claude"],
        "both" => &["codex", "claude"],
        _ => {
            return Err(failure(
                "Unsupported coordinator client; use codex, claude or both.",
            ));
        }
    };
    let executable = connect::managed_executable(source_executable, managed_root, true)
        .map_err(|error| failure(error.to_string()))?;
    let args: Vec<&str> = launch_args.iter().map(String::as_str).collect();
    let mut plans = Vec::new();
    for client in clients {
        let path = workspace.join(if *client == "codex" {
            ".codex/config.toml"
        } else {
            ".mcp.json"
        });
        confined(&workspace, &path)?;
        connect::connect_named_launch_to(
            client,
            &path,
            &executable,
            &args,
            SERVER_NAME,
            forward_typesafe,
            true,
        )
        .map_err(|error| failure(format!("{client} preflight: {error}")))?;
        plans.push((*client, path));
    }
    let descriptor = workspace.join(".switchbite/stackbite-plugin.json");
    confined(&workspace, &descriptor)?;
    let metadata = json!({
        "schema_version": 1,
        "kind": PLUGIN_KIND,
        "integration": "project_scoped_coordinator_mcp",
        "clients": clients,
        "server": SERVER_NAME,
        "executable": executable,
        "args": args,
        "judge": judge,
        "coordinator_guidance": crate::workflow_interface::INSTRUCTIONS,
        "next_action": NEXT_ACTION
    });
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&metadata)
            .map_err(|_| failure("Unable to encode plugin descriptor."))?
    );
    connect::save_descriptor(&descriptor, &executable, &text, true, descriptor_is_ours)
        .map_err(|error| failure(format!("Plugin descriptor preflight: {error}")))?;

    if !options.dry_run {
        let actual = connect::managed_executable(source_executable, managed_root, false)
            .map_err(|error| failure(error.to_string()))?;
        if actual != executable {
            return Err(failure(
                "Executable changed during setup; no client settings changed. Retry installation.",
            ));
        }
    }
    let mut configured = Vec::new();
    for (client, path) in plans {
        match connect::connect_named_launch_to(
            client,
            &path,
            &executable,
            &args,
            SERVER_NAME,
            forward_typesafe,
            options.dry_run,
        ) {
            Ok(report) => configured.push(report),
            Err(error) => {
                return Err(InstallError {
                    status: if configured.is_empty() {
                        "failed"
                    } else {
                        "partial"
                    },
                    message: format!("{client} configuration failed: {error}"),
                    configured,
                });
            }
        }
    }
    let saved = connect::save_descriptor(
        &descriptor,
        &executable,
        &text,
        options.dry_run,
        descriptor_is_ours,
    )
    .map_err(|error| InstallError {
        status: if configured.is_empty() {
            "failed"
        } else {
            "partial"
        },
        message: format!("Plugin descriptor write failed: {error}"),
        configured: std::mem::take(&mut configured),
    })?;
    Ok(InstallReport {
        status: if options.dry_run {
            "preview"
        } else {
            "installed"
        },
        workspace,
        integration: "project_scoped_coordinator_mcp",
        executable,
        server: SERVER_NAME,
        judge,
        clients: configured,
        descriptor,
        descriptor_backup: saved.backup,
        next_action: if options.dry_run {
            "Run without --dry-run to install; then start a new trusted coordinator CLI session."
        } else {
            NEXT_ACTION
        },
    })
}
