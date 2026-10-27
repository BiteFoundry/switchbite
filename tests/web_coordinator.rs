use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
use switchbite::{web_coordinator, workflow::Workflow};

fn git(dir: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}
fn setup() -> (tempfile::TempDir, String, Value) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@localhost",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    let state = Workflow::new(tmp.path().join("state")).start(&json!({"workspace":repo,"goal":"Keep the complete user goal", "pool":[{"id":"worker","description":"worker","capabilities":[],"available":true}]})).unwrap();
    let action = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "plan")
        .unwrap()
        .clone();
    (tmp, state["run_id"].as_str().unwrap().to_owned(), action)
}
fn fake_worker(tmp: &Path) -> Value {
    let source = tmp.join("worker.rs");
    fs::write(&source, r##"fn main() {
        let args: Vec<String> = std::env::args().collect();
        let prompt = args.join(" ");
        assert!(prompt.contains("Keep the complete user goal"));
        assert!(prompt.contains("independent review node"));
        std::fs::write("switchbite-observation.json", r#"{"plan":{"nodes":[{"id":"build","objective":"Build"}]}}"#).unwrap();
        println!("{{\"type\":\"thread.started\",\"thread_id\":\"t\"}}");
        println!("{{\"type\":\"turn.started\"}}");
        println!("{{\"type\":\"turn.completed\"}}");
    }"##).unwrap();
    let exe = tmp.join(if cfg!(windows) {
        "worker.exe"
    } else {
        "worker"
    });
    assert!(
        Command::new("rustc")
            .arg(source)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    json!({"provider":"codex","executable":exe,"model":"test","effort":"high"})
}
#[test]
fn verified_worker_produces_plan_with_complete_context() {
    let (tmp, run, action) = setup();
    let candidate = fake_worker(tmp.path());
    let result =
        web_coordinator::fulfill(&tmp.path().join("state"), &run, &action, &candidate).unwrap();
    assert_eq!(result["plan"]["nodes"][0]["id"], "build");
    assert!(
        !tmp.path()
            .join("repo")
            .join("switchbite-observation.json")
            .exists()
    );
    // Idempotent replay returns the same receipt and artifact.
    assert_eq!(
        result,
        web_coordinator::fulfill(&tmp.path().join("state"), &run, &action, &candidate).unwrap()
    );
}
#[test]
fn cancellation_before_launch_is_bound_to_attempt() {
    let (tmp, run, action) = setup();
    let candidate = fake_worker(tmp.path());
    let root = tmp.path().join("state");
    web_coordinator::cancel(&root, &run, &action).unwrap();
    assert_eq!(
        web_coordinator::fulfill(&root, &run, &action, &candidate).unwrap_err()["code"],
        "coordinator_execution_failed"
    );
    let mut retry = action.clone();
    retry["coordinator_attempt"] = json!(1);
    assert!(web_coordinator::fulfill(&root, &run, &retry, &candidate).is_ok());
}
