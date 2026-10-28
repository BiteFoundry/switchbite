use serde_json::{Value, json};
use switchbite::workflow::Workflow;

fn request(review: bool) -> Value {
    json!({"workspace":"workspace","goal":"test","review_plan":review,"pool":[{"id":"worker","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"a","objective":"A"},{"id":"b","objective":"B"},{"id":"c","objective":"C","depends_on":["b"]}]}})
}
fn action(state: &Value, kind: &str, task: &str) -> Value {
    state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == kind && (task.is_empty() || a["task_id"] == task))
        .unwrap()
        .clone()
}

#[test]
fn optional_plan_review_holds_dispatch_and_manual_edges_control_ready_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().into());
    let s = w.start(&request(true)).unwrap();
    let run = s["run_id"].as_str().unwrap();
    assert_eq!(s["status"], "paused");
    assert_eq!(s["actions"], json!([]));
    let s = w.set_dependencies(run, "b", Some(&json!(["a"]))).unwrap();
    assert_eq!(s["dependency_overrides"]["b"], json!(["a"]));
    assert_eq!(s["tasks"][1]["task"]["depends_on"], json!(["a"]));
    let s = w.control(run, "resume").unwrap();
    assert_eq!(s["plan_review_pending"], false);
    assert!(
        s["actions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["task_id"] == "a")
    );
}

#[test]
fn automatic_start_is_unchanged_and_invalid_edges_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().into());
    let s = w.start(&request(false)).unwrap();
    let run = s["run_id"].as_str().unwrap();
    assert_eq!(s["status"], "running");
    assert!(!s["actions"].as_array().unwrap().is_empty());
    for deps in [
        json!(["c"]),
        json!(["b"]),
        json!(["missing"]),
        json!(["a", "a"]),
    ] {
        assert!(w.set_dependencies(run, "b", Some(&deps)).is_err());
        assert_eq!(w.status(run).unwrap()["dependency_overrides"], json!({}));
        assert_eq!(w.status(run).unwrap()["plan_revision"], 0);
    }
}

#[test]
fn stale_route_receipt_cannot_reassign_a_manually_changed_task() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().into());
    let s = w.start(&request(false)).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let route = action(&s, "route", "b");
    let s = w.set_dependencies(run, "b", Some(&json!(["a"]))).unwrap();
    assert!(
        s["actions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != route["id"])
    );
    let s=w.observe(run,&json!({"action_id":route["id"],"event_id":"old-route","result":{"status":"ready","candidate_id":"worker"}})).unwrap();
    assert_eq!(s["tasks"][1]["status"], "pending");
    assert_eq!(s["tasks"][1]["candidate"], Value::Null);
}

#[test]
fn manual_edges_survive_restart_and_coordinator_replan_and_can_be_released() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().into());
    let s = w.start(&request(false)).unwrap();
    let run = s["run_id"].as_str().unwrap();
    w.set_dependencies(run, "b", Some(&json!(["a"]))).unwrap();
    let w = Workflow::new(dir.path().into());
    let s = w
        .report(
            run,
            &json!({"id":"direction","source":"user","kind":"finding","message":"Review plan"}),
        )
        .unwrap();
    let replan = action(&s, "replan", "");
    let s=w.observe(run,&json!({"action_id":replan["id"],"event_id":"replan","result":{"plan":request(false)["plan"],"reply":"Updated","interrupt_tasks":[]}})).unwrap();
    assert_eq!(s["tasks"][1]["task"]["depends_on"], json!(["a"]));
    let s = w.set_dependencies(run, "b", None).unwrap();
    assert!(s["dependency_overrides"].get("b").is_none());
    assert!(
        s["feedback"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["task_id"] == "b" && f["status"] == "pending")
    );
    let count = s["feedback"].as_array().unwrap().len();
    assert_eq!(
        w.set_dependencies(run, "b", None).unwrap()["feedback"]
            .as_array()
            .unwrap()
            .len(),
        count
    );
}

#[test]
fn claimed_preparation_cannot_be_rewired() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().into());
    let s = w.start(&request(false)).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let route = action(&s, "route", "b");
    let s=w.observe(run,&json!({"action_id":route["id"],"event_id":"route","result":{"status":"ready","candidate_id":"worker"}})).unwrap();
    let prep = action(&s, "prepare", "b");
    assert!(w.claim(run, prep["id"].as_str().unwrap()).unwrap());
    assert_eq!(
        w.set_dependencies(run, "b", Some(&json!(["a"])))
            .unwrap_err()
            .code,
        "task_already_started"
    );
    assert!(
        !w.status(run).unwrap()["editable_task_ids"]
            .as_array()
            .unwrap()
            .contains(&json!("b"))
    );
}

#[test]
fn generated_plan_review_pauses_before_any_worker_route() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().into());
    let mut req = request(true);
    req.as_object_mut().unwrap().remove("plan");
    let s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let plan = action(&s, "plan", "");
    let s=w.observe(run,&json!({"action_id":plan["id"],"event_id":"plan","result":{"plan":request(false)["plan"]}})).unwrap();
    assert_eq!(s["plan_review_pending"], true);
    assert_eq!(s["actions"], json!([]));
}
