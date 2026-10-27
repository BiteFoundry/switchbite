use serde_json::json;
use switchbite::{web_workbench::Workbench, workflow::Workflow};

#[test]
fn reopened_app_preserves_run_scope_and_saved_coordinator() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow = Workflow::new(tmp.path().join(".switchbite/workflow"));
    let state=workflow.start(&json!({"workspace":tmp.path(),"goal":"Keep my task","catalog":{"providers":["codex"]}})).unwrap();
    let run = state["run_id"].as_str().unwrap();
    workflow.control(run, "pause").unwrap();
    let candidates = vec![
        json!({"id":"codex-one","provider":"codex","model":"test","effort":"high","available":true}),
        json!({"id":"claude-one","provider":"claude","model":"test","effort":"high","available":true}),
    ];
    let app = Workbench::with_candidates(tmp.path(), candidates.clone(), None).unwrap();
    assert_eq!(
        app.resume_existing(run, Some("claude-one")).unwrap_err()["code"],
        "coordinator_unavailable"
    );
    let restored = app.resume_existing(run, Some("codex-one")).unwrap();
    assert_eq!(restored["run_id"], run);
    assert_eq!(restored["status"], "paused");
    assert_eq!(restored["tools"], json!(["codex"]));
    drop(app);
    let app = Workbench::with_candidates(tmp.path(), candidates, None).unwrap();
    let restored = app.resume_existing(run, None).unwrap();
    assert_eq!(restored["coordinator"]["model"], "test");
    assert_eq!(restored["goal"], "Keep my task");
}
