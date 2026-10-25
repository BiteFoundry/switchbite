use serde_json::Value;
use std::process::Command;
const BIN: &str = env!("CARGO_BIN_EXE_switchbite");
#[test]
fn local_selection_is_explicit_and_status_reports_real_model_without_a_cloud_key() {
    let out = Command::new(BIN)
        .env_clear()
        .args(["doctor", "--judge", "laya"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["model"], "laya:multilingual");
    assert_eq!(status["judge_configured"], true);
    let out = Command::new(BIN)
        .env_clear()
        .args([
            "doctor",
            "--judge",
            "laya",
            "--endpoint",
            "https://example.org",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["code"], "local_judge_config");
}
#[test]
fn local_judge_cannot_silently_render_or_install_a_cloud_connection() {
    for args in [
        vec!["config", "--client", "codex", "--judge", "laya"],
        vec!["codex", "--dry-run", "--judge", "laya"],
        vec![
            "snapshot",
            "--request-file",
            "unused.json",
            "--judge",
            "laya",
        ],
    ] {
        let out = Command::new(BIN).env_clear().args(args).output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("switchbite stackbite"));
    }
}
