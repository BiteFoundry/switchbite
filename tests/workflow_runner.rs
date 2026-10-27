mod common;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};
use switchbite::{
    adapters::LocalHost,
    agent::{AgentJudge, ConfiguredJudge},
    workflow::Workflow,
    workflow_runner,
};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
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
    let markers = tmp.path().join("markers");
    fs::create_dir(&markers).unwrap();
    let src = tmp.path().join("fake.rs");
    let exe = tmp
        .path()
        .join(if cfg!(windows) { "fake.exe" } else { "fake" });
    let code = format!(
        r#"use std::{{fs,fs::OpenOptions,thread,time::{{Duration,Instant}}}};
fn main() {{
 let args:Vec<String>=std::env::args().collect(); let prompt=args.last().unwrap();
 let id=["one","two","review","slow","solo"].iter().find(|id|prompt.contains(&format!("\"id\":\"{{}}\"",id))).expect("task ID");
 let dir=std::path::Path::new({markers:?}); OpenOptions::new().write(true).create_new(true).open(dir.join(id)).expect("duplicate launch");
 if *id=="one"||*id=="two" {{ let other=if *id=="one"{{"two"}}else{{"one"}}; let deadline=Instant::now()+Duration::from_secs(10); while !dir.join(other).exists() {{ assert!(Instant::now()<deadline,"parallel worker did not start");thread::sleep(Duration::from_millis(20)); }} }}
 if *id=="slow" {{ thread::sleep(Duration::from_millis(1500)); }}
 if *id=="review" {{ assert!(std::path::Path::new("one.txt").exists()&&std::path::Path::new("two.txt").exists(),"review ran before integration"); }}
 fs::write(format!("{{}}.txt",id),"done").unwrap();
 if args.iter().any(|a|a=="--session-id") {{ let session=&args[args.iter().position(|a|a=="--session-id").unwrap()+1]; println!("{{{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{{}}\",\"model\":\"fake-model\"}}}}",session);println!("{{{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"permission_denials\":[],\"session_id\":\"{{}}\"}}}}",session); }} else {{ println!("{{{{\"type\":\"thread.started\",\"thread_id\":\"test\"}}}}");println!("{{{{\"type\":\"turn.started\"}}}}");println!("{{{{\"type\":\"turn.completed\"}}}}"); }}
}}"#,
        markers = markers.to_str().unwrap()
    );
    fs::write(&src, code).unwrap();
    assert!(
        Command::new("rustc")
            .arg(&src)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    let state = tmp.path().join("workflow");
    (tmp, repo, exe, state)
}
fn judge() -> Option<Arc<ConfiguredJudge>> {
    let cb: Arc<AgentJudge> = Arc::new(|body| Ok(common::judgment(body)));
    Some(Arc::new(ConfiguredJudge::legacy(cb)))
}
fn candidate(id: &str, provider: &str, exe: &Path) -> Value {
    json!({"id":id,"description":"Test worker","capabilities":[id],"available":true,"provider":provider,"model":"fake-model","effort":"high","executable":exe.to_str().unwrap()})
}
fn node(id: &str, deps: &[&str]) -> Value {
    json!({"id":id,"objective":format!("Implement {id}"),"role":if id=="review"{"review"}else{"implementation"},"depends_on":deps,"requirements":[id],"acceptance":["file created"],"write_paths":[format!("{id}.txt")]})
}
fn request(repo: &Path, exe: &Path, nodes: Value) -> Value {
    json!({"workspace":repo.to_str().unwrap(),"pool":[candidate("one","codex",exe),candidate("two","claude",exe),candidate("review","codex",exe),candidate("slow","codex",exe),candidate("solo","codex",exe)],"plan":{"nodes":nodes},"max_parallel":2,"host":{"timeout_seconds":20}})
}
fn timed<F: FnOnce() -> Value + Send + 'static>(f: F) -> Value {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(30))
        .expect("runner did not settle within 30 seconds")
}
#[test]
fn parallel_workers_integrate_before_dependent_review_without_changing_source_repo() {
    let (_tmp, repo, exe, root) = fixture();
    let before = git(&repo, &["rev-parse", "HEAD"]);
    let req = request(
        &repo,
        &exe,
        json!([
            node("one", &[]),
            node("two", &[]),
            node("review", &["one", "two"])
        ]),
    );
    let run_root = root.clone();
    let result =
        timed(move || workflow_runner::run_to_completion(&req, &run_root, judge()).unwrap());
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["progress"]["done"], 3);
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), before);
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert!(!repo.join("one.txt").exists());
    let integration = result["tasks"][2]["prepared"]["integration_workspace"]
        .as_str()
        .unwrap();
    for name in ["one.txt", "two.txt", "review.txt"] {
        assert!(Path::new(integration).join(name).exists());
    }
}
#[test]
fn pause_keeps_active_launch_tracked_and_resume_finishes() {
    let (tmp, repo, exe, root) = fixture();
    let req = request(&repo, &exe, json!([node("slow", &[])]));
    let engine = Workflow::new(root.clone());
    let state = engine.start(&req).unwrap();
    let run = state["run_id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let _ = workflow_runner::advance(&root, run, judge()).unwrap();
        if tmp.path().join("markers/slow").exists() {
            break;
        }
        assert!(Instant::now() < deadline, "worker did not start");
        thread::sleep(Duration::from_millis(20));
    }
    engine.control(run, "pause").unwrap();
    let paused = workflow_runner::advance(&root, run, judge()).unwrap();
    assert_eq!(paused["status"], "paused");
    assert!(
        paused["driver"]["active_actions"].as_u64().unwrap() > 0,
        "{paused}"
    );
    engine.control(run, "resume").unwrap();
    let run = run.to_owned();
    let finish =
        timed(move || workflow_runner::resume_to_completion(&run, &root, judge()).unwrap());
    assert_eq!(finish["status"], "completed", "{finish}");
}
#[test]
fn held_frontier_with_pending_descendant_returns_attention() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("state");
    let engine = Workflow::new(root.clone());
    let req = json!({"workspace":"unused","pool":[],"plan":{"nodes":[node("one",&[]),node("two",&["one"])]}});
    let start = engine.start(&req).unwrap();
    let run = start["run_id"].as_str().unwrap().to_owned();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(workflow_runner::resume_to_completion(&run, &root, None));
    });
    let result = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("held prerequisite and pending descendant must return promptly")
        .unwrap();
    assert_eq!(result["next_action"], "continue_with_coordinator");
}
#[test]
fn recovered_host_receipt_does_not_launch_worker_twice() {
    let (tmp, repo, exe, root) = fixture();
    let engine = Workflow::new(root.clone());
    let req = request(&repo, &exe, json!([node("solo", &[])]));
    let start = engine.start(&req).unwrap();
    let run = start["run_id"].as_str().unwrap();
    let route = start["actions"][0].clone();
    let state=engine.observe(run,&json!({"action_id":route["id"],"event_id":"manual-route","result":{"status":"ready","candidate_id":"solo"}})).unwrap();
    let prep = state["actions"][0].clone();
    let host = LocalHost::new(&repo, &root.join(run).join("host"), &req["host"]).unwrap();
    let prepared = host.perform(&prep).unwrap();
    let state = engine
        .observe(
            run,
            &json!({"action_id":prep["id"],"event_id":"manual-prepare","result":prepared}),
        )
        .unwrap();
    let launch = state["actions"][0].clone();
    assert_eq!(launch["kind"], "launch");
    assert_eq!(host.perform(&launch).unwrap()["status"], "completed");
    assert!(tmp.path().join("markers/solo").exists());
    let run = run.to_owned();
    let finish =
        timed(move || workflow_runner::resume_to_completion(&run, &root, judge()).unwrap());
    assert_eq!(finish["status"], "completed", "{finish}");
    assert_eq!(fs::read_dir(tmp.path().join("markers")).unwrap().count(), 1);
}
