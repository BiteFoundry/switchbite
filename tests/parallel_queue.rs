use serde_json::{Value, json};
use switchbite::workflow::Workflow;

fn request(nodes: Value, max_parallel: u64) -> Value {
    json!({"workspace":"workspace", "max_parallel":max_parallel,
        "pool":[
            {"id":"shared","description":"worker","capabilities":[],"available":true,
             "resource_key":"account-a","resource_capacity":1},
            {"id":"other","description":"worker","capabilities":[],"available":true,
             "resource_key":"account-b","resource_capacity":1}],
        "plan":{"nodes":nodes}})
}

fn action(state: &Value, kind: &str, task: &str) -> Value {
    state["actions"].as_array().unwrap().iter()
        .find(|a| a["kind"] == kind && a["task_id"] == task)
        .unwrap_or_else(|| panic!("missing {kind} for {task}: {state}"))
        .clone()
}

fn has_action(state: &Value, kind: &str, task: &str) -> bool {
    state["actions"].as_array().unwrap().iter()
        .any(|a| a["kind"] == kind && a["task_id"] == task)
}

fn task<'a>(state: &'a Value, id: &str) -> &'a Value {
    state["tasks"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()
}

fn send(w: &Workflow, run: &str, a: &Value, result: Value) -> Value {
    let id = a["id"].as_str().unwrap();
    w.observe(run, &json!({"event_id":format!("event:{id}"),"action_id":id,"result":result})).unwrap()
}

fn route(w: &Workflow, run: &str, state: &Value, task: &str, candidate: &str) -> Value {
    send(w, run, &action(state, "route", task), json!({"status":"ready","candidate_id":candidate}))
}

fn prepared() -> Value {
    json!({"status":"prepared","workspace":"w","branch":"b","integration_workspace":"i","integration_branch":"ib"})
}

#[test]
fn quota_quarantine_stops_queued_shared_resource_and_allows_other_resource() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut state = w.start(&request(json!([
        {"id":"first","objective":"first","outputs":["first.txt"]},
        {"id":"queued","objective":"queued","outputs":["queued.txt"]},
        {"id":"independent","objective":"independent","outputs":["independent.txt"]}
    ]), 1)).unwrap();
    let run = state["run_id"].as_str().unwrap().to_owned();
    for (task, candidate) in [("first","shared"), ("queued","shared"), ("independent","other")] {
        state = route(&w, &run, &state, task, candidate);
    }
    assert!(!has_action(&state, "prepare", "queued"));
    assert!(!has_action(&state, "prepare", "independent"));
    state = send(&w, &run, &action(&state, "prepare", "first"), prepared());
    let launch = action(&state, "launch", "first");
    assert!(w.claim(&run, launch["id"].as_str().unwrap()).unwrap());
    state = send(&w, &run, &launch, json!({"status":"failed","execution_id":launch["id"],
        "completion_verified":false,"error":{"code":"rate_limited"}}));
    assert!(!has_action(&state, "prepare", "queued"), "{state}");
    assert!(!has_action(&state, "launch", "queued"), "{state}");
    assert!(has_action(&state, "prepare", "independent"), "{state}");
}

#[test]
fn completed_worker_releases_slot_but_retains_output_lock_through_integration() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut state = w.start(&request(json!([
        {"id":"first","objective":"first","outputs":["src/shared"]},
        {"id":"collision","objective":"collision","outputs":["src/shared/child"]},
        {"id":"independent","objective":"independent","outputs":["src/other"]}
    ]), 2)).unwrap();
    let run = state["run_id"].as_str().unwrap().to_owned();
    for task in ["first", "collision", "independent"] {
        state = route(&w, &run, &state, task, "shared");
    }
    state = send(&w, &run, &action(&state, "prepare", "first"), prepared());
    let launch = action(&state, "launch", "first");
    state = send(&w, &run, &launch, json!({"status":"completed","completion_verified":true,"execution_id":launch["id"]}));
    assert!(has_action(&state, "verify", "first"), "{state}");
    assert!(has_action(&state, "prepare", "independent"), "{state}");
    assert!(!has_action(&state, "prepare", "collision"), "{state}");
    assert_eq!(task(&state, "collision")["wait_reason"]["code"], "write_conflict", "{state}");
    state = send(&w, &run, &action(&state, "verify", "first"), json!({"status":"passed","evidence":["check"]}));
    assert!(has_action(&state, "integrate", "first"), "{state}");
    assert!(!has_action(&state, "prepare", "collision"), "{state}");
    assert_eq!(task(&state, "collision")["wait_reason"]["code"], "write_conflict", "{state}");
}
