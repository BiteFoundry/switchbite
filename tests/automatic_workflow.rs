use serde_json::{Value, json};
use switchbite::workflow::Workflow;

fn action(state: &Value, kind: &str, task: &str) -> Value {
    state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == kind && a["task_id"] == task)
        .unwrap()
        .clone()
}
fn observe(engine: &Workflow, run: &str, a: &Value, result: Value) -> Value {
    engine.observe(run, &json!({"event_id":format!("test:{}",a["id"].as_str().unwrap()),"action_id":a["id"],"result":result})).unwrap()
}
fn candidate(i: usize, model: &str) -> Value {
    json!({"id":format!("codex:{i}"),"description":"Worker","provider":"codex","model":model,"effort":"high","executable":"test-executable","resource_key":format!("account:{i}"),"capabilities":["code.change"],"available":true})
}
fn catalog(candidates: Vec<Value>, revision: &str) -> Value {
    json!({"schema_version":1,"revision":revision,"observed_at_ms":1,"candidates":candidates,"diagnostics":[]})
}

#[test]
fn manual_pool_wins_and_exact_pin_reaches_receipt_check() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Workflow::new(dir.path().join("state"));
    let state = engine.start(&json!({"workspace":"synthetic","catalog":"ignored for explicit pool","pool":[candidate(1,"model-a"),candidate(2,"model-b")],"plan":{"nodes":[{"id":"work","objective":"Write JSON","acceptance":["JSON parses"],"outputs":["out.json"],"routing":{"candidate_id":"codex:2"}}]}})).unwrap();
    assert!(state.get("catalog").is_none());
    let run = state["run_id"].as_str().unwrap();
    let a = action(&state, "route", "work");
    let input = &a["payload"]["route_input"];
    assert_eq!(input["pool"].as_array().unwrap().len(), 1);
    assert!(
        input["task"]["objective"]
            .as_str()
            .unwrap()
            .contains("JSON parses")
    );
    assert!(
        input["pool"][0]["description"]
            .as_str()
            .unwrap()
            .contains("model-b")
    );
    assert_eq!(engine.observe(run, &json!({"event_id":"wrong","action_id":a["id"],"result":{"status":"ready","candidate_id":"codex:1"}})).unwrap_err().code, "invalid_receipt");
    let state = observe(
        &engine,
        run,
        &a,
        json!({"status":"ready","candidate_id":"codex:2"}),
    );
    assert_eq!(state["tasks"][0]["candidate"]["model"], "model-b");
}

#[test]
fn automatic_catalog_shortlists_more_than_sixteen_and_keeps_leased_facts() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Workflow::new(dir.path().join("state"));
    // Empty provider selection makes this deterministic and avoids local probes.
    let state = engine.start(&json!({"workspace":dir.path(),"catalog":{"providers":[]},"plan":{"nodes":[{"id":"work","objective":"Write a precise config","requirements":["code.change"],"routing":{"model":"chosen"}}]}})).unwrap();
    let run = state["run_id"].as_str().unwrap();
    assert_eq!(state["catalog"]["candidate_count"], 0);
    let a = action(&state, "route", "work");
    let state = observe(&engine, run, &a, json!({"status":"hold"}));
    let refresh = action(&state, "refresh_pool", "work");
    let mut candidates: Vec<_> = (0..40).map(|i| candidate(i, "other")).collect();
    candidates.push(candidate(40, "chosen"));
    let state = observe(
        &engine,
        run,
        &refresh,
        json!({"catalog":catalog(candidates,"new-catalog")}),
    );
    let a = action(&state, "route", "work");
    assert_eq!(
        a["payload"]["candidate_snapshot"].as_array().unwrap().len(),
        1
    );
    assert_eq!(a["payload"]["catalog_revision"], "new-catalog");
    let state = observe(
        &engine,
        run,
        &a,
        json!({"status":"ready","candidate_id":"codex:40"}),
    );
    assert_eq!(state["tasks"][0]["candidate"]["model"], "chosen");
    let restored = Workflow::new(dir.path().join("state")).status(run).unwrap();
    assert_eq!(restored, state);
    assert!(switchbite::workflow_runner::local_execution(
        &engine.request(run).unwrap()
    ));
}

#[test]
fn old_snapshots_load_without_new_catalog_or_schema_fields() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Workflow::new(dir.path().to_path_buf());
    let state = engine
        .start(&json!({"workspace":"synthetic","goal":"Plan","pool":[]}))
        .unwrap();
    let run = state["run_id"].as_str().unwrap();
    let path = dir
        .path()
        .join(run)
        .join(format!("{}.json", state["revision"]));
    let mut persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    persisted.as_object_mut().unwrap().remove("schema_version");
    persisted.as_object_mut().unwrap().remove("catalog");
    std::fs::write(path, serde_json::to_vec(&persisted).unwrap()).unwrap();
    let restored = engine.status(run).unwrap();
    assert_eq!(restored["status"], "running");
    assert_eq!(restored["schema_version"], 0);
    assert_eq!(restored["actions"], state["actions"]);
}
