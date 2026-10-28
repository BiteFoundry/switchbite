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
        let mut input = String::new();
        let prompt = if args.last().map(String::as_str) == Some("-") {
            use std::io::Read;
            std::io::stdin().read_to_string(&mut input).unwrap();
            input
        } else {
            args.join(" ")
        };
        if prompt.contains("LONG_CONTEXT_BEGIN") {
            assert_eq!(args.last().map(String::as_str), Some("-"));
            assert!(prompt.len() > 32767);
            assert!(prompt.contains(&format!("LONG_CONTEXT_BEGIN{}LONG_CONTEXT_END", "x".repeat(40000))));
        }
        assert!(prompt.contains("Keep the complete user goal"));
        if prompt.contains("REPLAN_FIXTURE") {
            assert!(prompt.contains("actual user feedback"));
            assert!(prompt.contains("Please add a review"));
            std::fs::write("switchbite-observation.json", r#"{"plan":{"nodes":[{"id":"build","objective":"Build"},{"id":"review","objective":"Review the build","depends_on":["build"]}]},"reply":"Added a review after the build.","interrupt_tasks":[]}"#).unwrap();
        } else if prompt.contains("Review the proposed worker assignment") {
            assert!(prompt.contains("candidate_snapshot"));
            std::fs::write("switchbite-observation.json", r#"{"status":"ready","candidate_id":"worker","reason_codes":["coordinator_review"],"summary":"Eligible worker"}"#).unwrap();
        } else {
            assert!(prompt.contains("independent review node"));
            assert!(prompt.contains("result_schema"));
            assert!(prompt.contains("Exact capability IDs"));
            if prompt.contains("RETRY_FIXTURE") && !prompt.contains("coordinator_plan_invalid") {
                std::fs::write("switchbite-observation.json", r#"{"plan":{"nodes":[{"id":"build","objective":"Build","depends_on":["missing"]}]}}"#).unwrap();
            } else {
                if prompt.contains("RETRY_FIXTURE") {
                    assert!(prompt.contains("prior_result"));
                    assert!(prompt.contains("depends_on"));
                    assert!(prompt.contains("missing"));
                    assert!(prompt.contains("coordinator_plan_invalid"));
                }
                std::fs::write("switchbite-observation.json", r#"{"plan":{"nodes":[{"id":"build","objective":"Build","acceptance":"Build works"}]}}"#).unwrap();
            }
        }
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
    assert_eq!(
        result["plan"]["nodes"][0]["acceptance"],
        json!(["Build works"])
    );
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
    let registered = Workflow::new(tmp.path().join("state"))
        .observe(
            &run,
            &json!({"action_id":action["id"],"event_id":"plan-result","result":result}),
        )
        .unwrap();
    assert_eq!(registered["tasks"][0]["id"], "build");
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

#[test]
fn coordinator_reviews_low_confidence_route_and_core_accepts_the_choice() {
    let (tmp, run, plan) = setup();
    let root = tmp.path().join("state");
    let workflow = Workflow::new(root.clone());
    let state = workflow.observe(&run, &json!({"action_id":plan["id"],"event_id":"plan","result":{"plan":{"nodes":[{"id":"build","objective":"Build","acceptance":["Build works"]}]}}})).unwrap();
    let route = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "route")
        .unwrap();
    let state = workflow.observe(&run, &json!({"action_id":route["id"],"event_id":"held","result":{"status":"hold","reason_codes":["low_confidence"],"next_action":"request_review"}})).unwrap();
    let mut review = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "review_route")
        .unwrap()
        .clone();
    review["payload"]["long_context"] = json!(format!(
        "LONG_CONTEXT_BEGIN{}LONG_CONTEXT_END",
        "x".repeat(40000)
    ));
    let result = web_coordinator::fulfill(&root, &run, &review, &fake_worker(tmp.path())).unwrap();
    assert_eq!(result["candidate_id"], "worker");
    let state = workflow
        .observe(
            &run,
            &json!({"action_id":review["id"],"event_id":"reviewed","result":result}),
        )
        .unwrap();
    assert_eq!(state["tasks"][0]["candidate"]["id"], "worker");
}

#[test]
fn coordinator_retry_receives_previous_plan_and_diagnostic() {
    let (tmp, run, mut action) = setup();
    let candidate = fake_worker(tmp.path());
    let root = tmp.path().join("state");
    action["payload"]["fixture"] = json!("RETRY_FIXTURE");
    let feedback = web_coordinator::fulfill(&root, &run, &action, &candidate).unwrap_err();
    assert_eq!(feedback["code"], "coordinator_plan_invalid");
    assert_eq!(feedback["node_id"], "build");
    assert_eq!(feedback["field"], "depends_on");
    assert_eq!(
        feedback["prior_result"]["plan"]["nodes"][0]["depends_on"],
        json!(["missing"])
    );
    action["coordinator_attempt"] = json!(1);
    action["payload"]["coordinator_feedback"] = feedback;
    let result = web_coordinator::fulfill(&root, &run, &action, &candidate).unwrap();
    let registered = Workflow::new(root)
        .observe(
            &run,
            &json!({"action_id":action["id"],"event_id":"corrected-plan","result":result}),
        )
        .unwrap();
    assert_eq!(registered["tasks"][0]["id"], "build");
}

#[test]
fn replan_receives_report_and_returns_updated_dependency() {
    let (tmp, run, mut action) = setup();
    action["kind"] = json!("replan");
    action["payload"] = json!({"fixture":"REPLAN_FIXTURE", "feedback":[{"message":"Please add a review"}], "tasks":[]});
    let result = web_coordinator::fulfill(
        &tmp.path().join("state"),
        &run,
        &action,
        &fake_worker(tmp.path()),
    )
    .unwrap();
    assert_eq!(result["reply"], "Added a review after the build.");
    assert_eq!(result["plan"]["nodes"][1]["depends_on"], json!(["build"]));
}

#[test]
fn replan_acknowledgement_and_active_stop_are_validated() {
    let action = json!({"payload":{"tasks":[{"task":{"id":"build"},"status":"running"},{"task":{"id":"done"},"status":"completed"}]}});
    let ack = json!({"plan":null,"reply":"I will keep the current plan.","interrupt_tasks":[]});
    assert_eq!(
        web_coordinator::normalize_observation("replan", ack.clone(), &action).unwrap(),
        ack
    );
    let stop = json!({"plan":null,"reply":"Stopping build before changing it.","interrupt_tasks":["build"]});
    assert_eq!(
        web_coordinator::normalize_observation("replan", stop.clone(), &action).unwrap(),
        stop
    );
    let combined = json!({"plan":{"nodes":[{"id":"build","objective":"Build"},{"id":"followup","objective":"Review","depends_on":["build"]}]},"reply":"Adding follow-up and requesting stop.","interrupt_tasks":["build"]});
    assert_eq!(
        web_coordinator::normalize_observation("replan", combined.clone(), &action).unwrap(),
        combined
    );
    let settled = json!({"plan":null,"reply":"Checking stop.","interrupt_tasks":["done"]});
    assert_eq!(
        web_coordinator::normalize_observation("replan", settled.clone(), &action).unwrap(),
        settled
    );
    let invalid = json!({"plan":null,"reply":"Stopping.","interrupt_tasks":["missing"]});
    let error = web_coordinator::normalize_observation("replan", invalid, &action).unwrap_err();
    assert_eq!(error["field"], "interrupt_tasks");
    assert_eq!(error["code"], "coordinator_replan_invalid");
}
