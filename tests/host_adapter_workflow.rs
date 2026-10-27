use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
use switchbite::adapters::LocalHost;

fn git(dir: &Path, args: &[&str]) {
    let result = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn action(id: &str, kind: &str, payload: Value) -> Value {
    json!({"id":id,"kind":kind,"run_id":"run-1","payload":payload})
}
#[test]
fn owned_workflow_and_idempotence() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@localhost",
            "commit",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    let worker = tmp.path().join("worker.rs");
    fs::write(&worker, r#"fn main() { std::fs::write("created.txt", "done").unwrap(); println!("{{\"type\":\"turn.completed\"}}"); }"#).unwrap();
    let exe = tmp.path().join(if cfg!(windows) {
        "worker.exe"
    } else {
        "worker"
    });
    assert!(
        Command::new("rustc")
            .arg(&worker)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    let state = tmp.path().join("state");
    let host = LocalHost::new(&repo, &state, &json!({"timeout_seconds":10})).unwrap();
    let task = json!({"id":"task-1","objective":"Make the change"});
    let candidate = json!({"provider":"codex","executable":exe,"model":"example","effort":"high"});
    let prepare = action("prepare-1", "prepare", json!({"task":task}));
    let prepared = host.perform(&prepare).unwrap();
    assert_eq!(prepared["status"], "prepared");
    assert_eq!(prepared, host.perform(&prepare).unwrap());
    let payload = json!({"task":task,"candidate":candidate,"prepared":prepared,"checks":[]});
    let launch = action("launch-1", "launch", payload.clone());
    let result = host.perform(&launch).unwrap();
    assert_eq!(result["status"], "completed");
    assert_eq!(result["completion_verified"], true);
    assert_eq!(result, host.perform(&launch).unwrap());
    let inspect = host
        .perform(&action(
            "inspect-1",
            "inspect",
            json!({"execution_id":"launch-1"}),
        ))
        .unwrap();
    assert_eq!(inspect, result);
    let verify = host
        .perform(&action("verify-1", "verify", payload.clone()))
        .unwrap();
    assert_eq!(verify["status"], "passed");
    let integrated = host
        .perform(&action("integrate-1", "integrate", payload))
        .unwrap();
    assert_eq!(integrated["status"], "integrated");
    assert!(
        Path::new(prepared["integration_workspace"].as_str().unwrap())
            .join("created.txt")
            .exists()
    );
    assert!(!repo.join("created.txt").exists());
    let conflicting = action("launch-1", "launch", json!({}));
    assert_eq!(
        host.perform(&conflicting).unwrap_err().to_value()["code"],
        "action_conflict"
    );
    let reopened = LocalHost::new(&repo, &state, &json!({})).unwrap();
    assert_eq!(reopened.perform(&launch).unwrap(), result);
}
#[test]
fn incomplete_receipt_and_failed_check_do_not_advance() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@localhost",
            "commit",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    let worker = tmp.path().join("worker.rs");
    fs::write(&worker, "fn main() { println!(\"not a receipt\"); }").unwrap();
    let exe = tmp.path().join(if cfg!(windows) {
        "worker.exe"
    } else {
        "worker"
    });
    assert!(
        Command::new("rustc")
            .arg(&worker)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    let host = LocalHost::new(&repo, &tmp.path().join("state"), &json!({})).unwrap();
    let task = json!({"id":"t","objective":"work"});
    let prepared = host
        .perform(&action("p", "prepare", json!({"task":task})))
        .unwrap();
    let payload = json!({"task":task,"prepared":prepared,"candidate":{"provider":"codex","executable":exe,"model":"m","effort":"high"},"checks":[{"program":"git","args":["not-a-real-subcommand"]}]});
    assert_eq!(
        host.perform(&action("l", "launch", payload.clone()))
            .unwrap()["status"],
        "failed"
    );
    assert_eq!(
        host.perform(&action("v", "verify", payload)).unwrap()["status"],
        "failed"
    );
}
