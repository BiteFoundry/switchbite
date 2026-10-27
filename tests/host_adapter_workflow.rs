use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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
    fs::write(&worker, r#"fn main() { std::fs::write("created.txt", "done").unwrap(); println!("{{\"type\":\"thread.started\",\"thread_id\":\"t\"}}"); println!("{{\"type\":\"turn.started\"}}"); println!("{{\"type\":\"turn.completed\"}}"); }"#).unwrap();
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
    let gathered = host
        .perform(&action(
            "context",
            "gather_context",
            json!({"task":{"context_refs":["artifact:design/mockup"]}}),
        ))
        .unwrap();
    assert_eq!(gathered["context_refs"][0], "artifact:design/mockup");
    let task = json!({"id":"task-1","objective":"Make the change","outputs":["created.txt"]});
    let candidate = json!({"provider":"codex","executable":exe,"model":"example","effort":"high"});
    let prepare = action("prepare-1", "prepare", json!({"task":task}));
    let prepared = host.perform(&prepare).unwrap();
    assert_eq!(prepared["status"], "prepared");
    assert_eq!(prepared, host.perform(&prepare).unwrap());
    let prepare_key = format!("{:x}", Sha256::digest(b"prepare-1"));
    fs::remove_file(
        state
            .join("actions")
            .join(format!("{}.json", &prepare_key[..24])),
    )
    .unwrap();
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
    let integration_action = action("integrate-1", "integrate", payload);
    let integrated = host.perform(&integration_action).unwrap();
    assert_eq!(integrated["status"], "integrated");
    assert!(
        Path::new(prepared["integration_workspace"].as_str().unwrap())
            .join("created.txt")
            .exists()
    );
    assert!(!repo.join("created.txt").exists());
    let integration_key = format!("{:x}", Sha256::digest(b"integrate-1"));
    fs::remove_file(
        state
            .join("actions")
            .join(format!("{}.json", &integration_key[..24])),
    )
    .unwrap();
    let base = Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let base = String::from_utf8(base.stdout).unwrap();
    let target = Path::new(prepared["integration_workspace"].as_str().unwrap());
    fs::write(target.parent().unwrap().join("integration-transaction.json"), serde_json::to_vec(&json!({"action_id":"integrate-1","base_commit":base.trim(),"task_commit":integrated["commit"]})).unwrap()).unwrap();
    assert_eq!(
        host.perform(&integration_action).unwrap()["status"],
        "integrated"
    );
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
#[test]
fn cancel_from_another_host_signals_only_owned_child() {
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
    let source = tmp.path().join("slow.rs");
    fs::write(
        &source,
        r#"fn main() {
            if std::env::args().any(|a| a == "descendant") {
                std::thread::sleep(std::time::Duration::from_secs(2));
                std::fs::write("descendant-wrote", "bad").unwrap();
            } else {
                std::process::Command::new(std::env::current_exe().unwrap()).arg("descendant").spawn().unwrap();
                std::fs::write("child-started", "yes").unwrap();
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
        }"#,
    )
    .unwrap();
    let exe = tmp
        .path()
        .join(if cfg!(windows) { "slow.exe" } else { "slow" });
    assert!(
        Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    let state = tmp.path().join("state");
    let host = LocalHost::new(&repo, &state, &json!({"timeout_seconds":20})).unwrap();
    let task = json!({"id":"t", "objective":"work"});
    let prepared = host
        .perform(&action("p", "prepare", json!({"task":task})))
        .unwrap();
    let worker_dir = std::path::PathBuf::from(prepared["workspace"].as_str().unwrap());
    let launch = action(
        "slow-launch",
        "launch",
        json!({"task":task, "prepared":prepared, "candidate":{"provider":"codex", "executable":exe, "model":"m", "effort":"high"}}),
    );
    let owner = std::thread::spawn(move || host.perform(&launch).unwrap());
    let execution_dir = state.join("executions");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !execution_dir.exists()
        || !fs::read_dir(&execution_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.path().extension().is_some_and(|x| x == "json"))
        || !worker_dir.join("child-started").exists()
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let observer = LocalHost::new(&repo, &state, &json!({})).unwrap();
    let cancel = observer
        .perform(&action(
            "cancel",
            "inspect",
            json!({"execution_id":"slow-launch", "operation":"cancel"}),
        ))
        .unwrap();
    assert_eq!(cancel["status"], "unknown");
    let result = owner.join().unwrap();
    assert_eq!(result["status"], "failed");
    assert_eq!(result["cancelled"], true);
    let recovered = observer
        .perform(&action(
            "inspect",
            "inspect",
            json!({"execution_id":"slow-launch"}),
        ))
        .unwrap();
    assert_eq!(recovered, result);
    std::thread::sleep(std::time::Duration::from_millis(2300));
    assert!(!worker_dir.join("descendant-wrote").exists());
}

#[test]
fn integration_enforces_outputs_and_checks_the_merged_tree() {
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
    let host = LocalHost::new(&repo, &tmp.path().join("state"), &json!({})).unwrap();
    let task = json!({"id":"t","objective":"work","outputs":["allowed.txt", "artifact:design"]});
    let prepared = host
        .perform(&action("prep-check", "prepare", json!({"task":task})))
        .unwrap();
    let dir = Path::new(prepared["workspace"].as_str().unwrap());
    fs::write(dir.join("outside.txt"), "outside").unwrap();
    let payload = json!({"task":task,"prepared":prepared,"checks":[]});
    let rejected = host
        .perform(&action("reject", "integrate", payload.clone()))
        .unwrap();
    assert_eq!(rejected["status"], "failed");
    assert_eq!(rejected["evidence"][0], "write outside declared outputs");
    fs::remove_file(dir.join("outside.txt")).unwrap();
    fs::write(dir.join("allowed.txt"), "ok").unwrap();
    let mut failing = payload.clone();
    failing["checks"] = json!([{"program":"git","args":["not-a-real-subcommand"]}]);
    let result = host
        .perform(&action("failed-check", "integrate", failing))
        .unwrap();
    assert_eq!(result["reason"], "integration_checks_failed");
    assert!(
        !Path::new(prepared["integration_workspace"].as_str().unwrap())
            .join("allowed.txt")
            .exists()
    );
    assert!(dir.join("allowed.txt").exists());
}
