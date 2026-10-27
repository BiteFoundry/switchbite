use serde_json::json;
use switchbite::workflow_interface::{Interface, tools};

#[test]
fn workflow_tools_are_three_and_named() {
    let names: Vec<String> = tools().iter().map(|t| t.name.to_string()).collect();
    assert_eq!(
        names,
        ["switchbite_start", "switchbite_next", "switchbite_observe"]
    );
}

#[test]
fn workflow_plan_action_survives_reopening_and_pause_resume() {
    let dir = tempfile::tempdir().unwrap();
    let request =
        json!({"workspace":dir.path().to_str().unwrap(), "goal":"Create a report", "pool":[]});
    let first = Interface::new(dir.path().to_path_buf(), None);
    let started = first.start(&request).unwrap();
    let run = started["run_id"].as_str().unwrap();
    let action = started["actions"][0].clone();
    assert_eq!(action["kind"], "plan");
    let reopened = Interface::new(dir.path().to_path_buf(), None);
    assert_eq!(reopened.next(run).unwrap()["actions"][0], action);
    assert_eq!(reopened.control(run, "pause").unwrap()["status"], "paused");
    assert_eq!(
        reopened.control(run, "resume").unwrap()["status"],
        "running"
    );
    assert_eq!(reopened.status(run).unwrap()["actions"][0], action);
}
