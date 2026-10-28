use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};
use switchbite::{adapters::LocalHost, workflow::Workflow, workflow_runner};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn coordinator_interrupt_stops_a_claimed_native_launch_before_replanning() {
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
            "--allow-empty",
            "-qm",
            "base",
        ],
    );
    let source = tmp.path().join("waiting.rs");
    fs::write(&source, r#"use std::{fs,io::Write,thread,time::Duration};
fn main() {
 let path=fs::read_dir(".").unwrap().map(|e|e.unwrap().path()).find(|p|p.file_name().unwrap().to_string_lossy().starts_with(".switchbite-feedback-")).unwrap();
 let mut file=fs::OpenOptions::new().append(true).open(path).unwrap();
 writeln!(file,"{{\"id\":\"need-direction\",\"kind\":\"blocked\",\"message\":\"Need revised instructions\"}}").unwrap(); file.flush().unwrap();
 loop {thread::sleep(Duration::from_millis(20));}
}"#).unwrap();
    let exe = tmp.path().join(if cfg!(windows) {
        "waiting.exe"
    } else {
        "waiting"
    });
    assert!(
        Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    let root = tmp.path().join("state");
    let engine = Workflow::new(root.clone());
    let state = engine.start(&json!({"workspace":repo,"goal":"Wait for direction","host":{"timeout_seconds":20},"pool":[{"id":"worker","description":"worker","capabilities":[],"available":true,"provider":"codex","model":"fake","effort":"high","executable":exe}],"plan":{"nodes":[{"id":"work","objective":"Work after receiving direction","write_paths":["result.txt"]}]}})).unwrap();
    let run = state["run_id"].as_str().unwrap().to_owned();
    let route = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "route")
        .unwrap();
    let state=engine.observe(&run,&json!({"action_id":route["id"],"event_id":"route","result":{"status":"ready","candidate_id":"worker"}})).unwrap();
    let prepare = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "prepare")
        .unwrap();
    assert!(engine.claim(&run, prepare["id"].as_str().unwrap()).unwrap());
    let host = LocalHost::new(
        &repo,
        &root.join(&run).join("host"),
        &json!({"timeout_seconds":20}),
    )
    .unwrap();
    let prepared = host.perform(prepare).unwrap();
    let state = engine
        .observe(
            &run,
            &json!({"action_id":prepare["id"],"event_id":"prepare","result":prepared}),
        )
        .unwrap();
    let launch = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "launch")
        .unwrap()
        .clone();
    assert!(engine.claim(&run, launch["id"].as_str().unwrap()).unwrap());
    let job_action = launch.clone();
    let worker = thread::spawn(move || host.perform(&job_action));
    let deadline = Instant::now() + Duration::from_secs(10);
    let replan = loop {
        let state = workflow_runner::advance(&root, &run, None).unwrap();
        if let Some(action) = state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == "replan")
        {
            assert!(!worker.is_finished());
            break action.clone();
        }
        assert!(Instant::now() < deadline, "Missing live report: {state}");
        thread::sleep(Duration::from_millis(20));
    };
    engine.observe(&run,&json!({"action_id":replan["id"],"event_id":"stop-worker","result":{"plan":null,"reply":"Stop this worker and revise after termination.","interrupt_tasks":["work"]}})).unwrap();
    while !worker.is_finished() {
        workflow_runner::advance(&root, &run, None).unwrap();
        assert!(
            Instant::now() < deadline,
            "Claimed native launch was not interrupted"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let receipt = worker.join().unwrap().unwrap();
    assert_eq!(receipt["status"], "failed");
    let state = engine
        .observe(
            &run,
            &json!({"action_id":launch["id"],"event_id":"native-stopped","result":receipt}),
        )
        .unwrap();
    assert_ne!(state["tasks"][0]["status"], "done");
    assert!(
        state["feedback"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["source"] == "system")
    );
    assert!(
        state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "replan")
    );
}

#[test]
fn live_worker_reports_are_durable_and_block_success() {
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
    let source = tmp.path().join("fake.rs");
    fs::write(&source, r#"
use std::{fs::{self, OpenOptions}, io::Write, thread, time::Duration};
fn main() {
    let path = fs::read_dir(".").unwrap().map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with(".switchbite-feedback-")).unwrap();
    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    writeln!(f, "{{\"id\":\"one\",\"kind\":\"blocked\",\"message\":\"Need an input\"}}").unwrap();
    f.flush().unwrap();
    while !std::path::Path::new("release-test").exists() { thread::sleep(Duration::from_millis(10)); }
    fs::remove_file("release-test").unwrap();
    writeln!(f, "{{\"id\":\"two\",\"kind\":\"finding\",\"message\":\"Final note\"}}").unwrap();
    f.flush().unwrap();
    drop(f);
    fs::write("created.txt", "done").unwrap();
    println!("{{\"type\":\"thread.started\",\"thread_id\":\"t\"}}");
    println!("{{\"type\":\"turn.started\"}}");
    println!("{{\"type\":\"turn.completed\"}}");
}
"#).unwrap();
    let exe = tmp
        .path()
        .join(if cfg!(windows) { "fake.exe" } else { "fake" });
    assert!(
        Command::new("rustc")
            .arg(&source)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    // Force the host's short-worktree branch on Windows. The collector must
    // derive the same digest from the normalized canonical host-state path.
    let root = tmp.path().join(format!("state-{}", "x".repeat(130)));
    let engine = Workflow::new(root.clone());
    let state = engine.start(&json!({"workspace":repo,"goal":"Make a file","pool":[{"id":"fake","description":"worker","capabilities":[],"available":true,"provider":"codex","model":"fake","effort":"high","executable":exe}],"plan":{"nodes":[{"id":"work","objective":"Create file","role":"implementation","write_paths":["created.txt"]}]}})).unwrap();
    let run = state["run_id"].as_str().unwrap().to_owned();
    let route = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "route")
        .unwrap();
    let state = engine.observe(&run, &json!({"action_id":route["id"],"event_id":"route-result","result":{"status":"ready","candidate_id":"fake"}})).unwrap();
    assert!(
        state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "prepare")
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let (launch, dir) = loop {
        let state = workflow_runner::advance(&root, &run, None).unwrap();
        if let Some(action) = state["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == "launch")
        {
            let dir = action["payload"]["prepared"]["workspace"]
                .as_str()
                .unwrap()
                .to_owned();
            break (action.clone(), dir);
        }
        assert!(
            Instant::now() < deadline,
            "launch was not scheduled: {state}"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let dir = Path::new(&dir);
    loop {
        let state = workflow_runner::advance(&root, &run, None).unwrap();
        if state["feedback"].as_array().unwrap().len() == 1 {
            assert_eq!(state["feedback"][0]["kind"], "blocked");
            assert!(state["driver"]["active_actions"].as_u64().unwrap() > 0);
            break;
        }
        assert!(Instant::now() < deadline, "live report missing: {state}");
        thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..3 {
        assert_eq!(
            workflow_runner::advance(&root, &run, None).unwrap()["feedback"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    fs::write(dir.join("release-test"), "go").unwrap();
    loop {
        let state = workflow_runner::advance(&root, &run, None).unwrap();
        if state["driver"]["active_actions"] == 0 {
            let reports = state["feedback"].as_array().unwrap();
            let worker: Vec<_> = reports.iter().filter(|f| f["source"] == "worker").collect();
            assert_eq!(worker.len(), 2);
            assert!(worker.iter().any(|f| f["kind"] == "blocked"));
            assert!(worker.iter().any(|f| f["kind"] == "finding"));
            assert!(reports.iter().any(|f| {
                f["source"] == "system"
                    && f["id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with("blocked-outcome:"))
            }));
            break;
        }
        assert!(Instant::now() < deadline, "worker did not finish: {state}");
        thread::sleep(Duration::from_millis(20));
    }
    let id = launch["id"].as_str().unwrap();
    let digest = format!("{:x}", Sha256::digest(id.as_bytes()));
    let key = &digest[..24];
    let host_state = root.join(&run).join("host");
    assert!(
        host_state
            .join("messages")
            .join(format!("{key}.feedback.jsonl"))
            .exists()
    );
    assert!(
        !dir.join(format!(".switchbite-feedback-{key}.jsonl"))
            .exists()
    );
    let receipt: Value = serde_json::from_slice(
        &fs::read(host_state.join("actions").join(format!("{key}.json"))).unwrap(),
    )
    .unwrap();
    // The host receipt persists the failed result even when native execution was verified.
    let result = &receipt["result"];
    assert_eq!(result["status"], "failed");
    assert_eq!(result["reason"], "worker_reported_blocked");
    assert_eq!(result["native_completion_verified"], true);
    let host = LocalHost::new(&repo, &host_state, &json!({})).unwrap();
    let integrated = host.perform(&json!({"id":"test-integration","kind":"integrate","run_id":run,"payload":launch["payload"]})).unwrap();
    assert_eq!(integrated["status"], "integrated");
    let target = Path::new(
        launch["payload"]["prepared"]["integration_workspace"]
            .as_str()
            .unwrap(),
    );
    assert!(target.join("created.txt").exists());
    assert!(
        !target
            .join(format!(".switchbite-feedback-{key}.jsonl"))
            .exists()
    );
}

#[test]
fn superseded_route_error_does_not_block_revised_task() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use switchbite::{
        agent::{AgentJudge, ConfiguredJudge},
        runtime::RuntimeError,
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("state");
    let engine = Workflow::new(root.clone());
    let original = json!({"id":"work","objective":"Original task"});
    let revised = json!({"id":"work","objective":"Revised task"});
    let state = engine.start(&json!({"workspace":tmp.path(),"goal":"test","pool":[{"id":"fake","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[original]}})).unwrap();
    let run = state["run_id"].as_str().unwrap().to_owned();
    let entered = Arc::new(AtomicUsize::new(0));
    let first_release = Arc::new(AtomicBool::new(false));
    let second_release = Arc::new(AtomicBool::new(false));
    let judge: Arc<AgentJudge> = {
        let entered = entered.clone();
        let first_release = first_release.clone();
        let second_release = second_release.clone();
        Arc::new(move |_| {
            let index = entered.fetch_add(1, Ordering::SeqCst);
            let release = if index == 0 {
                &first_release
            } else {
                &second_release
            };
            while !release.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            Err(RuntimeError::from_code("invalid_judgment"))
        })
    };
    let judge = Some(Arc::new(ConfiguredJudge::legacy(judge)));
    workflow_runner::advance(&root, &run, judge.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while entered.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let state = engine.report(&run, &json!({"id":"change","source":"user","task_id":"work","kind":"request_change","message":"Revise"})).unwrap();
    let replan = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "replan")
        .unwrap();
    engine.observe(&run, &json!({"action_id":replan["id"],"event_id":"revise","result":{"plan":{"nodes":[revised]},"reply":"Updated","interrupt_tasks":[]}})).unwrap();
    first_release.store(true, Ordering::SeqCst);
    loop {
        let state = workflow_runner::advance(&root, &run, judge.clone()).unwrap();
        assert!(
            state.get("attention").is_none(),
            "obsolete failure raised attention: {state}"
        );
        if entered.load(Ordering::SeqCst) >= 2 && state["driver"]["active_actions"] == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "revised route did not start: {state}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    second_release.store(true, Ordering::SeqCst);
    loop {
        let state = workflow_runner::advance(&root, &run, judge.clone()).unwrap();
        if state["driver"]["active_actions"] == 0 {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}
