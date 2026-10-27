use serde_json::{Value, json};
use switchbite::workflow::Workflow;

fn request() -> Value {
    json!({"workspace":"workspace","pool":[{"id":"a","description":"worker","capabilities":[],"available":true},{"id":"b","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"one","objective":"one","outputs":["src/one"]},{"id":"two","objective":"two","outputs":["src/two"]},{"id":"three","objective":"three","depends_on":["one"]}]}})
}
fn action<'a>(v: &'a Value, kind: &str, task: &str) -> &'a Value {
    v["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == kind && a["task_id"] == task)
        .unwrap()
}
fn send(w: &Workflow, run: &str, a: &Value, result: Value) -> Value {
    let aid = a["id"].as_str().unwrap();
    w.observe(
        run,
        &json!({"event_id":format!("event:{aid}"),"action_id":aid,"result":result}),
    )
    .unwrap()
}
fn prepared() -> Value {
    json!({"status":"prepared","workspace":"w","branch":"b","integration_workspace":"i","integration_branch":"ib"})
}
#[test]
fn independent_tasks_release_only_after_verified_integration_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let s = w.start(&request()).unwrap();
    let run = s["run_id"].as_str().unwrap();
    assert_eq!(s["actions"].as_array().unwrap().len(), 2);
    let s = send(
        &w,
        run,
        action(&s, "route", "one"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(
        &w,
        run,
        action(&s, "route", "two"),
        json!({"status":"ready","candidate_id":"b"}),
    );
    let s = send(&w, run, action(&s, "prepare", "one"), prepared());
    let s = send(&w, run, action(&s, "prepare", "two"), prepared());
    let launch = action(&s, "launch", "one").clone();
    let restarted = Workflow::new(dir.path().to_path_buf());
    let recovered = restarted.next(run).unwrap();
    assert_eq!(action(&recovered, "launch", "one")["id"], launch["id"]);
    let bad = json!({"event_id":"bad","action_id":launch["id"],"result":{"status":"completed"}});
    assert_eq!(w.observe(run, &bad).unwrap_err().code, "invalid_receipt");
    let s = send(
        &w,
        run,
        &launch,
        json!({"status":"completed","completion_verified":true}),
    );
    assert!(
        !s["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["task_id"] == "three")
    );
    let s = send(
        &w,
        run,
        action(&s, "verify", "one"),
        json!({"status":"passed","evidence":["check"]}),
    );
    assert!(
        !s["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["task_id"] == "three")
    );
    let s = send(
        &w,
        run,
        action(&s, "integrate", "one"),
        json!({"status":"integrated","commit":"abc"}),
    );
    assert_eq!(action(&s, "route", "three")["kind"], "route");
}
#[test]
fn event_idempotency_and_dag_validation() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut invalid = request();
    invalid["plan"]["nodes"][0]["depends_on"] = json!(["three"]);
    assert_eq!(w.start(&invalid).unwrap_err().code, "invalid_request");
    let s = w.start(&request()).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let a = action(&s, "route", "one");
    let e = json!({"event_id":"same","action_id":a["id"],"result":{"status":"ready","candidate_id":"a"}});
    let first = w.observe(run, &e).unwrap();
    assert_eq!(w.observe(run, &e).unwrap(), first);
    let mut changed = e.clone();
    changed["result"]["candidate_id"] = json!("b");
    assert_eq!(w.observe(run, &changed).unwrap_err().code, "event_conflict");
    let late = json!({"event_id":"new","action_id":a["id"],"result":e["result"]});
    assert_eq!(w.observe(run, &late).unwrap_err().code, "action_not_found");
}
#[test]
fn same_resource_waits_and_cancellation_reconciles_unobserved_launch() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut req = request();
    req["pool"][1]["resource_key"] = json!("shared");
    req["pool"][0]["resource_key"] = json!("shared");
    let s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let s = send(
        &w,
        run,
        action(&s, "route", "one"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(
        &w,
        run,
        action(&s, "route", "two"),
        json!({"status":"ready","candidate_id":"b"}),
    );
    assert!(
        !s["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "prepare" && a["task_id"] == "two")
    );
    let s = send(&w, run, action(&s, "prepare", "one"), prepared());
    let launch_id = action(&s, "launch", "one")["id"].clone();
    let s = w.control(run, "cancel").unwrap();
    assert_eq!(
        action(&s, "inspect", "one")["payload"]["execution_id"],
        launch_id
    );
    assert_eq!(
        action(&s, "inspect", "one")["payload"]["operation"],
        "cancel"
    );
}
#[test]
fn insufficient_context_and_bounded_repair() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut req = request();
    req["repair_rounds"] = json!(1);
    let s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let s = send(
        &w,
        run,
        action(&s, "route", "one"),
        json!({"status":"hold","reason_codes":["context_insufficient"]}),
    );
    let s = send(
        &w,
        run,
        action(&s, "gather_context", "one"),
        json!({"status":"gathered","context_refs":["docs/context.md"]}),
    );
    let s = send(
        &w,
        run,
        action(&s, "route", "one"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(&w, run, action(&s, "prepare", "one"), prepared());
    let s = send(
        &w,
        run,
        action(&s, "launch", "one"),
        json!({"status":"failed"}),
    );
    let s = send(
        &w,
        run,
        action(&s, "repair", "one"),
        json!({"task":{"id":"one","objective":"retry","outputs":["src/one"]}}),
    );
    assert_eq!(
        action(&s, "route", "one")["payload"]["task"]["objective"],
        "retry"
    );
}
#[test]
fn runs_have_unique_action_ids_and_cancel_unknown_is_reconciled() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let a = w.start(&request()).unwrap();
    let b = w.start(&request()).unwrap();
    assert_ne!(a["actions"][0]["id"], b["actions"][0]["id"]);
    let run = a["run_id"].as_str().unwrap();
    let a = send(
        &w,
        run,
        action(&a, "route", "one"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let a = send(&w, run, action(&a, "prepare", "one"), prepared());
    assert_eq!(action(&a, "launch", "one")["kind"], "launch");
    let a = w.control(run, "cancel").unwrap();
    let inspection = action(&a, "inspect", "one").clone();
    let a = send(&w, run, &inspection, json!({"status":"unknown"}));
    let next = action(&a, "inspect", "one").clone();
    assert_ne!(next["id"], inspection["id"]);
    let a = send(&w, run, &next, json!({"status":"failed","cancelled":true}));
    assert!(
        !a["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["task_id"] == "one")
    );
    assert_eq!(a["tasks"][0]["status"], "cancelled");
}
