use std::{fs, path::PathBuf, process::Command};
use switchbite::connect::{self, ConnectError};
use tempfile::tempdir;

fn executable() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\스위치\switchbite.exe")
    } else {
        PathBuf::from("/opt/스위치/switchbite")
    }
}

#[test]
fn every_dialect_preserves_existing_configuration_and_is_idempotent() {
    for client in connect::CLIENTS {
        let original = if matches!(*client, "codex" | "grok") {
            "# keep this comment\nmodel = 'keep-model'\n[mcp_servers.other]\ncommand = 'other' # keep inline\n"
        } else if *client == "opencode" {
            "{\n// keep this comment\n\"model\": \"keep-model\",\n\"mcp\": {\"servers\": {\"other\": {\"command\": [\"other\"]}}},\n}\n"
        } else {
            "{\n// keep this comment\n\"model\": \"keep-model\",\n\"mcpServers\": {\"other\": {\"command\": \"other\"}},\n}\n"
        };
        let updated = connect::merge(client, original, &executable(), false).unwrap();
        assert!(updated.contains("keep this comment"), "{client}");
        assert!(updated.contains("keep-model"));
        assert!(updated.contains("other"));
        assert_eq!(
            connect::merge(client, &updated, &executable(), false).unwrap(),
            updated,
            "{client}"
        );
    }
}

#[test]
fn preserves_server_customizations_and_credentials_when_upgrading() {
    let toml = "# judge selection stays separate\njudge = 'laya'\n[mcp_servers.switchbite]\ncommand = 'C:\\old\\switchbite.exe' # keep command comment\nargs = ['serve']\nenv_vars = ['ANOTHER_KEY']\nenabled = false\n[mcp_servers.switchbite.env]\nCUSTOM = 'existing-value'\n";
    let updated = connect::merge("codex", toml, &executable(), false).unwrap();
    let doc = updated.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(doc["judge"].as_str(), Some("laya"));
    assert_eq!(
        doc["mcp_servers"]["switchbite"]["env"]["CUSTOM"].as_str(),
        Some("existing-value")
    );
    assert_eq!(
        doc["mcp_servers"]["switchbite"]["enabled"].as_bool(),
        Some(true)
    );
    assert!(updated.contains("keep command comment"));
    let grants = doc["mcp_servers"]["switchbite"]["env_vars"]
        .as_array()
        .unwrap();
    assert_eq!(grants.len(), 2);
    let json = r#"{"mcpServers":{"switchbite":{"command":"switchbite","args":[],"disabled":true,"env":{"TYPESAFE_API_KEY":"existing-secret","CUSTOM":"existing-value"},"timeout":123}}}"#;
    let result = connect::merge("claude", json, &executable(), false).unwrap();
    let root = jsonc_parser::cst::CstRootNode::parse(&result, &Default::default()).unwrap();
    let server = root.to_serde_value().unwrap()["mcpServers"]["switchbite"].clone();
    assert_eq!(server["env"]["TYPESAFE_API_KEY"], "existing-secret");
    assert_eq!(server["timeout"], 123);
    assert_eq!(server["disabled"], false);
}

#[test]
fn refuses_ambiguous_or_foreign_configs_without_overwriting_them() {
    for (client, original) in [
        ("codex", "[mcp_servers.switchbite]\ncommand='unrelated'\n"),
        (
            "claude",
            r#"{"mcpServers":{"switchbite":{"url":"https://example.test"}}}"#,
        ),
        ("cursor", r#"{"mcpServers":4}"#),
        ("cursor", r#"{"mcpServers":{},"mcpServers":{}}"#),
        (
            "cursor",
            r#"{"mcpServers":{"switchbite":{"command":"switchbite","command":"other"}}}"#,
        ),
        ("opencode", r#"{"mcp":{"servers":false}}"#),
        (
            "opencode",
            r#"{"mcp":{"legacy":{"type":"local","command":["other"]}}}"#,
        ),
        ("grok", "[mcp_servers\n"),
        ("claude", "[1,2,3]"),
    ] {
        let dir = tempdir().unwrap();
        let file = dir.path().join("config");
        fs::write(&file, original).unwrap();
        assert!(
            connect::connect_to(client, &file, &executable(), false, false).is_err(),
            "{client}"
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn backup_is_exact_and_repeated_connect_creates_no_extra_backup() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("config.toml");
    let original = b"# untouched CRLF\r\nmodel='my-model'\r\n";
    fs::write(&file, original).unwrap();
    let first = connect::connect_to("codex", &file, &executable(), false, false).unwrap();
    assert_eq!(first.status, "configured");
    assert_eq!(fs::read(first.backup.unwrap()).unwrap(), original);
    let before = fs::read(&file).unwrap();
    let second = connect::connect_to("codex", &file, &executable(), false, false).unwrap();
    assert_eq!(second.status, "already_configured");
    assert!(second.backup.is_none());
    assert_eq!(fs::read(&file).unwrap(), before);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn previews_have_no_filesystem_side_effects_and_busy_files_are_preserved() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("not-created/config.json");
    let report = connect::connect_to("cursor", &file, &executable(), false, true).unwrap();
    assert_eq!(report.status, "preview");
    assert!(!file.parent().unwrap().exists());
    let file = dir.path().join("config");
    fs::write(&file, "{}").unwrap();
    let lock = dir.path().join("config.switchbite-lock");
    fs::write(&lock, "another writer").unwrap();
    assert_eq!(
        connect::connect_to("cursor", &file, &executable(), false, false).unwrap_err(),
        ConnectError::Busy
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "{}");
    assert_eq!(fs::read_to_string(lock).unwrap(), "another writer");
}

#[test]
fn immutable_managed_copy_survives_source_removal_and_verifies_existing_bytes() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("downloaded.exe");
    fs::write(&source, b"synthetic binary").unwrap();
    let data = dir.path().join("data");
    let preview = connect::managed_executable(&source, &data, true).unwrap();
    assert!(!data.exists());
    let installed = connect::managed_executable(&source, &data, false).unwrap();
    assert_eq!(installed, preview);
    fs::write(&source, b"updated binary").unwrap();
    let replacement = connect::managed_executable(&source, &data, false).unwrap();
    assert_ne!(replacement, preview);
    assert_eq!(fs::read(&replacement).unwrap(), b"updated binary");
    assert_eq!(fs::read(&installed).unwrap(), b"synthetic binary");
    fs::write(&source, b"synthetic binary").unwrap();
    assert_eq!(
        connect::managed_executable(&source, &data, false).unwrap(),
        installed
    );
    fs::write(&installed, b"unexpected replacement").unwrap();
    assert_eq!(
        connect::managed_executable(&source, &data, false).unwrap_err(),
        ConnectError::Conflict
    );
    fs::write(&installed, b"synthetic binary").unwrap();
    fs::remove_file(source).unwrap();
    assert_eq!(fs::read(installed).unwrap(), b"synthetic binary");
}

#[test]
#[cfg(windows)]
fn replacement_failure_keeps_original_and_cleans_stage_and_lock() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("config.json");
    let original = b"{\"other\":true}";
    fs::write(&config, original).unwrap();
    let original_permissions = fs::metadata(&config).unwrap().permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&config, readonly).unwrap();
    let result = connect::connect_to("cursor", &config, &executable(), false, false);
    // Restore the attribute so tempfile can remove its own fixture.
    fs::set_permissions(&config, original_permissions).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(config).unwrap(), original);
    for entry in fs::read_dir(dir.path()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!name.ends_with(".tmp"));
        assert!(!name.ends_with(".switchbite-lock"));
    }
}

#[test]
fn inline_override_stops_setup_before_any_writes_or_secret_disclosure() {
    let dir = tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_switchbite"))
        .args(["opencode", "--json"])
        .env("USERPROFILE", dir.path())
        .env("HOME", dir.path())
        .env("LOCALAPPDATA", dir.path().join("data"))
        .env("OPENCODE_CLI_CONFIG_CONTENT", "inline-secret-sentinel")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("inline-secret-sentinel"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("override"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn every_shorthand_sets_up_only_its_selected_client() {
    let dir = tempdir().unwrap();
    for client in connect::CLIENTS {
        let home = dir.path().join(client);
        let output = Command::new(env!("CARGO_BIN_EXE_switchbite"))
            .args([client, "--json", "--offline"])
            .env("USERPROFILE", &home)
            .env("HOME", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", &home)
            .env("LOCALAPPDATA", dir.path().join("data"))
            .env("XDG_DATA_HOME", dir.path().join("data"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env_remove("OPENCODE_CLI_CONFIG_CONTENT")
            .env_remove("OPENCODE_CONFIG_CONTENT")
            .env_remove("OPENCODE_CONFIG")
            .env_remove("OPENCODE_CONFIG_DIR")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{client}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["client"], *client);
        let config = PathBuf::from(report["configuration"].as_str().unwrap());
        assert!(config.starts_with(&home));
        assert!(fs::read_to_string(config).unwrap().contains("--offline"));
    }
}

#[test]
fn one_command_uses_effective_codex_home_and_never_persists_environment_secrets() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("codex home");
    let data = dir.path().join("application data");
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_switchbite"))
            .args(["codex", "--json"])
            .args(extra)
            .env("CODEX_HOME", &config)
            .env("LOCALAPPDATA", &data)
            .env("XDG_DATA_HOME", &data)
            .env("TYPESAFE_API_KEY", "sentinel-do-not-persist")
            .output()
            .unwrap()
    };
    let preview = run(&["--dry-run"]);
    assert!(preview.status.success());
    assert!(!config.exists());
    assert!(!data.exists());
    let first = run(&[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["status"], "configured");
    assert_eq!(report["judge_configuration"], "unchanged");
    assert!(PathBuf::from(report["executable"].as_str().unwrap()).exists());
    let settings = fs::read_to_string(config.join("config.toml")).unwrap();
    assert!(settings.contains("TYPESAFE_API_KEY"));
    assert!(!settings.contains("sentinel-do-not-persist"));
    assert!(!String::from_utf8_lossy(&first.stdout).contains("sentinel-do-not-persist"));
    let second = run(&[]);
    assert!(second.status.success());
    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["status"], "already_configured");
    let status = Command::new(report["executable"].as_str().unwrap())
        .arg("--version")
        .output()
        .unwrap();
    assert!(status.status.success());
}

#[test]
fn migration_preserves_explicit_judge_and_rejects_custom_launch() {
    let input = "[mcp_servers.switchbite]\ncommand='switchbite'\nargs=['serve','--profile','agent','--judge','laya','--model','custom']\n";
    let output = connect::merge("codex", input, &executable(), false).unwrap();
    let doc = output.parse::<toml_edit::DocumentMut>().unwrap();
    let args = doc["mcp_servers"]["switchbite"]["args"].as_array().unwrap();
    let args: Vec<_> = args.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(
        args,
        [
            "serve",
            "--profile",
            "workflow",
            "--judge",
            "laya",
            "--model",
            "custom"
        ]
    );
    let custom = "[mcp_servers.switchbite]\ncommand='switchbite'\nargs=['custom-command']\n";
    assert!(connect::merge("codex", custom, &executable(), false).is_err());
}
