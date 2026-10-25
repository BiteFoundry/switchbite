use std::{fs, path::PathBuf};
use switchbite::{
    connect,
    stackbite_plugin::{self, InstallOptions},
};
use tempfile::tempdir;

fn options(workspace: PathBuf, dry_run: bool) -> InstallOptions {
    InstallOptions {
        workspace,
        client: "both".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model: "laya:multilingual".into(),
        dry_run,
    }
}

#[test]
fn dry_run_does_not_create_client_directories_managed_binary_or_descriptor() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("프로젝트");
    fs::create_dir(&workspace).unwrap();
    let source = dir.path().join("source.exe");
    fs::write(&source, b"synthetic binary").unwrap();
    let data = dir.path().join("data");
    let report =
        stackbite_plugin::install(&options(workspace.clone(), true), &source, &data).unwrap();
    assert_eq!(report.status, "preview");
    assert_eq!(report.clients.len(), 2);
    assert_eq!(fs::read_dir(&workspace).unwrap().count(), 0);
    assert!(!data.exists());
}

#[test]
fn foreign_second_client_preflight_preserves_first_and_creates_no_copy() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("project");
    fs::create_dir_all(workspace.join(".codex")).unwrap();
    let codex = workspace.join(".codex/config.toml");
    fs::write(&codex, "# keep\nmodel='existing'\n").unwrap();
    let foreign = r#"{"mcpServers":{"switchbite":{"command":"foreign-server"}}}"#;
    fs::write(workspace.join(".mcp.json"), foreign).unwrap();
    let source = dir.path().join("source.exe");
    fs::write(&source, b"synthetic binary").unwrap();
    let data = dir.path().join("data");
    let error =
        stackbite_plugin::install(&options(workspace.clone(), false), &source, &data).unwrap_err();
    assert_eq!(error.status, "failed");
    assert!(error.configured.is_empty());
    assert_eq!(
        fs::read_to_string(codex).unwrap(),
        "# keep\nmodel='existing'\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.join(".mcp.json")).unwrap(),
        foreign
    );
    assert!(!data.exists());
    assert!(!workspace.join(".switchbite").exists());
}

#[test]
fn installation_registers_stable_local_launch_and_preserves_unrelated_settings() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("project");
    fs::create_dir_all(workspace.join(".codex")).unwrap();
    let original = "# keep this comment\nmodel='existing'\n";
    fs::write(workspace.join(".codex/config.toml"), original).unwrap();
    fs::write(
        workspace.join(".mcp.json"),
        "{\n// keep JSON comment\n\"mcpServers\":{\"other\":{\"command\":\"other\"}}\n}\n",
    )
    .unwrap();
    let source = dir.path().join("source.exe");
    fs::write(&source, b"synthetic binary").unwrap();
    let data = dir.path().join("data");
    let settings = options(workspace.clone(), false);
    let first = stackbite_plugin::install(&settings, &source, &data).unwrap();
    assert_eq!(first.integration, "project_scoped_coordinator_mcp");
    assert!(first.executable.exists());
    let backup = first.clients[0].backup.as_ref().unwrap();
    assert_eq!(fs::read_to_string(backup).unwrap(), original);
    for path in [
        workspace.join(".codex/config.toml"),
        workspace.join(".mcp.json"),
    ] {
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("keep"));
        assert!(text.contains("--judge"));
        assert!(text.contains("laya:multilingual"));
        assert!(!text.contains("TYPESAFE_API_KEY"));
    }
    let descriptor: serde_json::Value =
        serde_json::from_slice(&fs::read(first.descriptor).unwrap()).unwrap();
    assert_eq!(descriptor["judge"]["fallback"], "hold");
    assert!(!workspace.join("AGENTS.md").exists());
    assert!(!workspace.join("CLAUDE.md").exists());
    let second = stackbite_plugin::install(&settings, &source, &data).unwrap();
    assert!(
        second
            .clients
            .iter()
            .all(|client| client.status == "already_configured")
    );
    assert!(second.clients.iter().all(|client| client.backup.is_none()));
    assert!(second.descriptor_backup.is_none());
    fs::remove_file(source).unwrap();
    assert_eq!(fs::read(second.executable).unwrap(), b"synthetic binary");
}

#[test]
fn switching_managed_server_to_laya_removes_only_cloud_judge_grants() {
    let dir = tempdir().unwrap();
    let exe = dir.path().join("switchbite.exe");
    let args = ["serve", "--profile", "agent", "--judge", "laya"];
    let toml = "[mcp_servers.switchbite]\ncommand='switchbite'\nenv_vars=['TYPESAFE_API_KEY', 'KEEP']\n[mcp_servers.switchbite.env]\nTYPESAFE_API_KEY='do-not-forward'\nKEEP='retained'\n";
    let result = connect::merge_launch("codex", toml, &exe, &args).unwrap();
    assert!(!result.contains("TYPESAFE_API_KEY"));
    assert!(!result.contains("do-not-forward"));
    assert!(result.contains("KEEP"));
    assert!(result.contains("retained"));
    let lower = "[mcp_servers.switchbite]\ncommand='switchbite'\nenv_vars=['typesafe_api_key', {name='TypeSafe_API_Key'}]\n[mcp_servers.switchbite.env]\nTypeSafe_API_Key='do-not-forward'\n";
    let result = connect::merge_launch("codex", lower, &exe, &args).unwrap();
    assert!(!result.to_ascii_lowercase().contains("typesafe_api_key"));
    assert!(!result.contains("do-not-forward"));
    let json = r#"{"mcpServers":{"switchbite":{"command":"switchbite","env":{"TYPESAFE_API_KEY":"do-not-forward","KEEP":"retained"}}}}"#;
    let result = connect::merge_launch("claude", json, &exe, &args).unwrap();
    assert!(!result.contains("TYPESAFE_API_KEY"));
    assert!(!result.contains("do-not-forward"));
    assert!(result.contains("retained"));
}

#[test]
fn invalid_local_endpoint_or_foreign_descriptor_stops_before_registration() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("project");
    fs::create_dir(&workspace).unwrap();
    let source = dir.path().join("source.exe");
    fs::write(&source, b"synthetic binary").unwrap();
    let data = dir.path().join("data");
    let mut settings = options(workspace.clone(), false);
    settings.endpoint = "https://remote.example.test".into();
    assert!(stackbite_plugin::install(&settings, &source, &data).is_err());
    assert_eq!(fs::read_dir(&workspace).unwrap().count(), 0);
    settings.endpoint = "http://127.0.0.1:11435".into();
    fs::create_dir(workspace.join(".switchbite")).unwrap();
    fs::write(
        workspace.join(".switchbite/stackbite-plugin.json"),
        "{\"kind\":\"foreign\"}",
    )
    .unwrap();
    assert!(stackbite_plugin::install(&settings, &source, &data).is_err());
    assert!(!workspace.join(".codex").exists());
    assert!(!workspace.join(".mcp.json").exists());
    assert!(!data.exists());
}

#[test]
fn later_write_error_reports_completed_client_without_claiming_full_installation() {
    let dir = tempdir().unwrap();
    let workspace = dir.path().join("project");
    fs::create_dir(&workspace).unwrap();
    // Existing lock is invisible to read-only preflight; the second mutation
    // fails after the first completes, exercising the accurate partial report.
    fs::write(workspace.join(".mcp.json.switchbite-lock"), "other process").unwrap();
    let source = dir.path().join("source.exe");
    fs::write(&source, b"synthetic binary").unwrap();
    let error = stackbite_plugin::install(
        &options(workspace.clone(), false),
        &source,
        &dir.path().join("data"),
    )
    .unwrap_err();
    assert_eq!(error.status, "partial");
    assert_eq!(error.configured.len(), 1);
    assert_eq!(error.configured[0].client, "codex");
    assert!(workspace.join(".codex/config.toml").exists());
    assert!(!workspace.join(".mcp.json").exists());
    assert!(!workspace.join(".switchbite/stackbite-plugin.json").exists());
    assert!(
        error
            .to_string()
            .contains("Completed client configurations")
    );
}
