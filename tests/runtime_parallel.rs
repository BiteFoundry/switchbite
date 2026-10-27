mod common;
use serde_json::json;
use std::{
    fs,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use switchbite::{
    agent::{AgentJudge, ConfiguredJudge},
    workflow::Workflow,
    workflow_runner,
};

#[test]
fn routes_overlap_and_advance_remains_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    assert!(
        Command::new("git")
            .current_dir(&repo)
            .args(["init"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&repo)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@localhost",
                "commit",
                "--allow-empty",
                "-m",
                "base"
            ])
            .status()
            .unwrap()
            .success()
    );
    let root = tmp.path().join("state");
    let request = json!({"workspace":repo,"pool":[
        {"id":"one","description":"worker","capabilities":["one"],"available":true,"provider":"codex","model":"fake-model","effort":"high","executable":"fake"},
        {"id":"two","description":"worker","capabilities":["two"],"available":true,"provider":"codex","model":"fake-model","effort":"high","executable":"fake"}
    ],"plan":{"nodes":[
        {"id":"one","objective":"one","role":"implementation","depends_on":[],"requirements":["one"],"acceptance":["done"],"write_paths":["one.txt"]},
        {"id":"two","objective":"two","role":"implementation","depends_on":[],"requirements":["two"],"acceptance":["done"],"write_paths":["two.txt"]}
    ]},"max_parallel":2});
    let state = Workflow::new(root.clone()).start(&request).unwrap();
    let run = state["run_id"].as_str().unwrap();
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(AtomicBool::new(false));
    let callback: Arc<AgentJudge> = {
        let entered = entered.clone();
        let release = release.clone();
        Arc::new(move |body| {
            entered.fetch_add(1, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !release.load(Ordering::SeqCst) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(common::judgment(body))
        })
    };
    let judge = Some(Arc::new(ConfiguredJudge::legacy(callback)));
    let started = Instant::now();
    workflow_runner::advance(&root, run, judge.clone()).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "advance waited for routing"
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while entered.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        workflow_runner::advance(&root, run, judge.clone()).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    let count = entered.load(Ordering::SeqCst);
    release.store(true, Ordering::SeqCst);
    assert_eq!(count, 2, "independent routes did not overlap");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let state = workflow_runner::advance(&root, run, judge.clone()).unwrap();
        if state["driver"]["active_actions"] == 0 || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}
