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
fn send(w: &Workflow, run: &str, a: &Value, mut result: Value) -> Value {
    if result["status"] == "completed" && result.get("execution_id").is_none() {
        result["execution_id"] = if a["kind"] == "launch" {
            a["id"].clone()
        } else {
            a["payload"]["execution_id"].clone()
        };
    }
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
    assert!(w.claim(run, launch_id.as_str().unwrap()).unwrap());
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
        json!({"status":"gathered","context_refs":["docs/context.md"],"summary":"important context"}),
    );
    assert!(
        action(&s, "route", "one")["payload"]["route_input"]["task"]["objective"]
            .as_str()
            .unwrap()
            .contains("important context")
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
    assert_eq!(action(&s, "route", "one")["payload"]["attempt"], 1);
    assert_eq!(
        action(&s, "route", "one")["payload"]["task"]["objective"],
        "retry"
    );
}

#[test]
fn repair_can_split_work_and_restate_completed_nodes_without_changing_them() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let request = json!({"workspace":"workspace","pool":[{"id":"a","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"done","objective":"Done","outputs":["done.md"]},{"id":"research","objective":"Research","depends_on":["done"],"outputs":["research.md"]}]}});
    let s = w.start(&request).unwrap();
    let run = s["run_id"].as_str().unwrap().to_owned();
    let s = send(
        &w,
        &run,
        action(&s, "route", "done"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(&w, &run, action(&s, "prepare", "done"), prepared());
    let s = send(
        &w,
        &run,
        action(&s, "launch", "done"),
        json!({"status":"completed","completion_verified":true}),
    );
    let s = send(
        &w,
        &run,
        action(&s, "verify", "done"),
        json!({"status":"passed","evidence":["check"]}),
    );
    let s = send(
        &w,
        &run,
        action(&s, "integrate", "done"),
        json!({"status":"integrated","commit":"abc"}),
    );
    let s = send(
        &w,
        &run,
        action(&s, "route", "research"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(&w, &run, action(&s, "prepare", "research"), prepared());
    let s = send(
        &w,
        &run,
        action(&s, "launch", "research"),
        json!({"status":"failed","reason":"timeout"}),
    );
    let repair = action(&s, "repair", "research");
    let mut nodes = request["plan"]["nodes"].as_array().unwrap().clone();
    nodes[1]["objective"] = json!("A smaller research task");
    nodes.push(json!({"id":"research-two","objective":"Second category","depends_on":["done"],"outputs":["second.md"]}));
    let mut invalid = nodes.clone();
    invalid[0]["objective"] = json!("Rewrite completed work");
    assert_eq!(w.observe(&run,&json!({"event_id":"invalid-repair","action_id":repair["id"],"result":{"plan":{"nodes":invalid}}})).unwrap_err().code,"invalid_receipt");
    let result = send(&w, &run, repair, json!({"plan":{"nodes":nodes}}));
    assert_eq!(result["progress"]["done"], 1);
    assert_eq!(result["progress"]["total"], 3);
    assert_eq!(result["tasks"][0]["status"], "done");
    assert_eq!(action(&result, "route", "research-two")["kind"], "route");
    assert_eq!(action(&result, "route", "research")["kind"], "route");
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
    assert!(
        w.claim(run, action(&a, "launch", "one")["id"].as_str().unwrap())
            .unwrap()
    );
    let a = w.control(run, "cancel").unwrap();
    let inspection = action(&a, "inspect", "one").clone();
    let a = send(&w, run, &inspection, json!({"status":"unknown"}));
    assert!(
        !a["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["task_id"] == "one")
    );
    let revision = a["revision"].clone();
    assert_eq!(w.next(run).unwrap()["revision"], revision);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let a = w.next(run).unwrap();
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

#[test]
fn host_errors_trigger_repairs_and_sanitized_evidence() {
    for stage in ["prepare", "gather_context", "verify", "integrate"] {
        let dir = tempfile::tempdir().unwrap();
        let w = Workflow::new(dir.path().to_path_buf());
        let mut req = request();
        req["repair_rounds"] = json!(1);
        let s = w.start(&req).unwrap();
        let run = s["run_id"].as_str().unwrap();
        let s = if stage == "gather_context" {
            send(
                &w,
                run,
                action(&s, "route", "one"),
                json!({"status":"hold","reason_codes":["context_insufficient"]}),
            )
        } else {
            let s = send(
                &w,
                run,
                action(&s, "route", "one"),
                json!({"status":"ready","candidate_id":"a"}),
            );
            if stage == "prepare" {
                s
            } else {
                let s = send(&w, run, action(&s, "prepare", "one"), prepared());
                let s = send(
                    &w,
                    run,
                    action(&s, "launch", "one"),
                    json!({"status":"completed","completion_verified":true}),
                );
                if stage == "verify" {
                    s
                } else {
                    send(
                        &w,
                        run,
                        action(&s, "verify", "one"),
                        json!({"status":"passed","evidence":[]}),
                    )
                }
            }
        };
        let s = send(
            &w,
            run,
            action(&s, stage, "one"),
            json!({"status":"failed","error":{"code":"host_failure","message":"secret detail"}}),
        );
        let repair = action(&s, "repair", "one");
        let evidence = &repair["payload"]["evidence"];
        assert!(
            evidence
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["error_code"] == "host_failure")
        );
        assert!(!evidence.to_string().contains("secret detail"));
    }
}
#[test]
fn rejects_ambiguous_output_paths() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    for path in ["../elsewhere", "src/../other", "C:/external", "/absolute"] {
        let mut req = request();
        req["plan"]["nodes"][0]["outputs"] = json!([path]);
        assert_eq!(w.start(&req).unwrap_err().code, "invalid_request");
    }
}
#[test]
fn write_paths_override_artifacts_and_reserve_glob_or_project() {
    for paths in [json!(["."]), json!(["src/**/*.rs"])] {
        let dir = tempfile::tempdir().unwrap();
        let w = Workflow::new(dir.path().to_path_buf());
        let mut req = request();
        req["plan"]["nodes"][0]["outputs"] = json!(["artifact:result"]);
        req["plan"]["nodes"][0]["write_paths"] = paths;
        req["plan"]["nodes"][1]["write_paths"] = json!(["src/file.rs"]);
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
        assert_eq!(action(&s, "prepare", "one")["kind"], "prepare");
        assert!(
            !s["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["kind"] == "prepare" && a["task_id"] == "two")
        );
    }
}
#[test]
fn claim_observes_pause_resume_and_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let s = w.start(&request()).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let id = s["actions"][0]["id"].as_str().unwrap();
    assert!(w.claim(run, id).unwrap());
    w.control(run, "pause").unwrap();
    assert!(!w.claim(run, id).unwrap());
    w.control(run, "resume").unwrap();
    assert!(w.claim(run, id).unwrap());
    w.control(run, "cancel").unwrap();
    assert!(!w.claim(run, id).unwrap());
}
#[test]
fn compact_events_preserve_idempotency_and_bound_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let nodes=(0..10).map(|i|json!({"id":format!("t{i}"),"objective":"do work","outputs":[format!("src/file{i}")]})).collect::<Vec<_>>();
    let pool=(0..10).map(|i|json!({"id":format!("c{i}"),"description":"worker","capabilities":[],"available":true})).collect::<Vec<_>>();
    let mut s = w
        .start(
            &json!({"workspace":"workspace","max_parallel":16,"plan":{"nodes":nodes},"pool":pool}),
        )
        .unwrap();
    let run = s["run_id"].as_str().unwrap().to_owned();
    let mut first = None;
    while s["status"] != "completed" {
        let a = s["actions"].as_array().unwrap()[0].clone();
        let task = a["task_id"].as_str().unwrap();
        let suffix = &task[1..];
        let result = match a["kind"].as_str().unwrap() {
            "route" => json!({"status":"ready","candidate_id":format!("c{suffix}")}),
            "prepare" => prepared(),
            "launch" => {
                json!({"status":"completed","completion_verified":true,"execution_id":a["id"]})
            }
            "verify" => {
                json!({"status":"passed","evidence":[{"status":"passed","private":"never expose"}]})
            }
            "integrate" => json!({"status":"integrated"}),
            _ => panic!("unexpected action"),
        };
        let event = json!({"event_id":format!("event:{}",a["id"].as_str().unwrap()),"action_id":a["id"],"result":result});
        if first.is_none() {
            first = Some(event.clone());
        }
        s = w.observe(&run, &event).unwrap();
    }
    assert_eq!(s["event_count"], 50);
    assert_eq!(s["events"].as_array().unwrap().len(), 32);
    assert_eq!(s["event_cursor"], json!({"first":19,"last":50}));
    assert!(!s["events"].to_string().contains("never expose"));
    let event = first.unwrap();
    assert_eq!(w.observe(&run, &event).unwrap(), s);
    let mut changed = event.clone();
    changed["result"]["status"] = json!("hold");
    assert_eq!(
        w.observe(&run, &changed).unwrap_err().code,
        "event_conflict"
    );
    let count = std::fs::read_dir(dir.path().join(&run))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .count();
    assert!(count <= 2, "snapshot files: {count}");
    let restarted = Workflow::new(dir.path().to_path_buf());
    assert_eq!(restarted.observe(&run, &event).unwrap(), s);
}

#[test]
fn role_policies_exclude_dependency_producers_from_review() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let req = json!({"workspace":"workspace","pool":[{"id":"a","description":"worker","capabilities":[],"available":true},{"id":"b","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"build","objective":"build","role":"implementation"},{"id":"review","objective":"review","role":"review","depends_on":["build"]}]}});
    let s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap();
    assert_eq!(
        action(&s, "route", "build")["payload"]["route_input"]["policy"]["profile"],
        "implementation"
    );
    let s = send(
        &w,
        run,
        action(&s, "route", "build"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(&w, run, action(&s, "prepare", "build"), prepared());
    let s = send(
        &w,
        run,
        action(&s, "launch", "build"),
        json!({"status":"completed","completion_verified":true}),
    );
    let s = send(
        &w,
        run,
        action(&s, "verify", "build"),
        json!({"status":"passed","evidence":[]}),
    );
    let s = send(
        &w,
        run,
        action(&s, "integrate", "build"),
        json!({"status":"integrated"}),
    );
    let a = action(&s, "route", "review");
    let input = &a["payload"]["route_input"];
    assert_eq!(input["policy"]["profile"], "review");
    assert_eq!(input["policy"]["excluded_candidate_ids"], json!(["a"]));
    switchbite::routing::normalize(input).unwrap();
    let event = json!({"action_id":a["id"],"event_id":"bad-reviewer","result":{"status":"ready","candidate_id":"a"}});
    assert_eq!(w.observe(run, &event).unwrap_err().code, "invalid_receipt");
    let s = send(&w, run, a, json!({"status":"ready","candidate_id":"b"}));
    assert_eq!(action(&s, "prepare", "review")["kind"], "prepare");
}
#[test]
fn refresh_pool_resumes_holds_and_protects_active_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let req = request();
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
        json!({"status":"hold","reason_codes":["no_eligible_candidates"],"next_action":"refresh_pool"}),
    );
    let refresh = action(&s, "refresh_pool", "two").clone();
    let mut pool = req["pool"].clone();
    pool[0]["description"] = json!("changed active profile");
    let bad = json!({"action_id":refresh["id"],"event_id":"bad-pool","result":{"pool":pool}});
    assert_eq!(w.observe(run, &bad).unwrap_err().code, "invalid_receipt");
    let mut pool = req["pool"].clone();
    pool[1]["description"] = json!("updated idle profile");
    let s = send(&w, run, &refresh, json!({"pool":pool}));
    let route = action(&s, "route", "two");
    assert_eq!(
        route["payload"]["route_input"]["pool"][1]["description"],
        "updated idle profile"
    );
    assert_eq!(
        route["payload"]["route_input"]["policy"]["tie_behavior"],
        "stable_id"
    );
    assert_eq!(action(&s, "prepare", "one")["kind"], "prepare");
}
#[test]
fn rejects_wrong_and_missing_execution_ids() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let s = w.start(&request()).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let s = send(
        &w,
        run,
        action(&s, "route", "one"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(&w, run, action(&s, "prepare", "one"), prepared());
    let launch = action(&s, "launch", "one");
    for result in [
        json!({"status":"completed","completion_verified":true}),
        json!({"status":"completed","completion_verified":true,"execution_id":"another"}),
        json!({"status":"running","execution_id":"another"}),
    ] {
        let event = json!({"action_id":launch["id"],"event_id":"bad-identity","result":result});
        assert_eq!(w.observe(run, &event).unwrap_err().code, "invalid_receipt");
    }
    let s = send(
        &w,
        run,
        launch,
        json!({"status":"completed","completion_verified":true}),
    );
    assert_eq!(action(&s, "verify", "one")["kind"], "verify");
}

#[test]
fn cancellation_retains_claimed_integration_and_records_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let s = w.start(&request()).unwrap();
    let run = s["run_id"].as_str().unwrap();
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
        json!({"status":"completed","completion_verified":true}),
    );
    let s = send(
        &w,
        run,
        action(&s, "verify", "one"),
        json!({"status":"passed","evidence":[]}),
    );
    let integration = action(&s, "integrate", "one").clone();
    let id = integration["id"].as_str().unwrap();
    assert!(w.claim(run, id).unwrap());
    let restarted = Workflow::new(dir.path().to_path_buf());
    let s = restarted.control(run, "cancel").unwrap();
    assert_eq!(action(&s, "integrate", "one")["id"], id);
    assert!(restarted.claim(run, id).unwrap());
    let s = send(
        &restarted,
        run,
        &integration,
        json!({"status":"integrated","commit":"0123456789012345678901234567890123456789"}),
    );
    assert_eq!(s["status"], "cancelled");
    assert_eq!(s["tasks"][0]["status"], "done");
    assert!(
        s["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "integrate" && e["status"] == "integrated")
    );
    assert!(
        !s["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["task_id"] == "three")
    );
}
#[test]
fn cancelled_unclaimed_launch_does_not_create_phantom_execution() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let s = w.start(&request()).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let s = send(
        &w,
        run,
        action(&s, "route", "one"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let s = send(&w, run, action(&s, "prepare", "one"), prepared());
    let launch = action(&s, "launch", "one")["id"].as_str().unwrap();
    let cancelled = w.control(run, "cancel").unwrap();
    assert!(!w.claim(run, launch).unwrap());
    assert_eq!(cancelled["tasks"][0]["status"], "cancelled");
    assert!(
        !cancelled["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["task_id"] == "one")
    );
}
#[test]
fn cancelled_claimed_prepare_receipt_leaves_task_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let s = w.start(&request()).unwrap();
    let run = s["run_id"].as_str().unwrap();
    let s = send(
        &w,
        run,
        action(&s, "route", "one"),
        json!({"status":"ready","candidate_id":"a"}),
    );
    let prepare = action(&s, "prepare", "one").clone();
    assert!(w.claim(run, prepare["id"].as_str().unwrap()).unwrap());
    w.control(run, "cancel").unwrap();
    let s = send(&w, run, &prepare, prepared());
    assert_eq!(s["status"], "cancelled");
    assert_eq!(s["tasks"][0]["status"], "cancelled");
    assert!(
        !s["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["task_id"] == "one")
    );
}

fn parallel_request(capacity: Option<u64>) -> Value {
    let mut req = json!({"workspace":"workspace","max_parallel":3,"pool":[
        {"id":"a","description":"worker","capabilities":[],"available":true,"resource_key":"shared"},
        {"id":"b","description":"worker","capabilities":[],"available":true,"resource_key":"other"}],
        "plan":{"nodes":[{"id":"one","objective":"one","outputs":["src/one"]},
            {"id":"two","objective":"two","outputs":["src/two"]},
            {"id":"three","objective":"three","outputs":["src/three"]}]}});
    if let Some(capacity) = capacity {
        req["pool"][0]["resource_capacity"] = json!(capacity);
    }
    req
}
fn task<'a>(s: &'a Value, id: &str) -> &'a Value {
    s["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}
#[test]
fn capacity_three_reserves_independent_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut s = w.start(&parallel_request(Some(3))).unwrap();
    let run = s["run_id"].as_str().unwrap().to_owned();
    for id in ["one", "two", "three"] {
        s = send(
            &w,
            &run,
            action(&s, "route", id),
            json!({"status":"ready","candidate_id":"a"}),
        );
    }
    for id in ["one", "two", "three"] {
        assert_eq!(action(&s, "prepare", id)["kind"], "prepare");
    }
    for id in ["one", "two", "three"] {
        s = send(&w, &run, action(&s, "prepare", id), prepared());
    }
    for id in ["one", "two", "three"] {
        assert_eq!(action(&s, "launch", id)["kind"], "launch");
    }
}
#[test]
fn capacity_defaults_to_one_and_waiting_does_not_starve_other_resources() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut req = parallel_request(None);
    req["max_parallel"] = json!(2);
    let mut s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap().to_owned();
    for (id, candidate) in [("one", "a"), ("two", "a"), ("three", "b")] {
        s = send(
            &w,
            &run,
            action(&s, "route", id),
            json!({"status":"ready","candidate_id":candidate}),
        );
    }
    assert_eq!(
        task(&s, "two")["wait_reason"],
        json!({"code":"resource_capacity","resource_key":"shared","limit":1})
    );
    assert_eq!(action(&s, "prepare", "one")["kind"], "prepare");
    assert_eq!(action(&s, "prepare", "three")["kind"], "prepare");
}
#[test]
fn capacity_is_conservative_and_write_conflicts_still_wait() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut req = parallel_request(Some(3));
    req["pool"][1]["resource_key"] = json!("shared");
    req["pool"][1]["resource_capacity"] = json!(1);
    req["plan"]["nodes"][2]["outputs"] = json!(["src/one/child"]);
    let mut s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap().to_owned();
    for id in ["one", "two", "three"] {
        s = send(
            &w,
            &run,
            action(&s, "route", id),
            json!({"status":"ready","candidate_id":"a"}),
        );
    }
    assert_eq!(task(&s, "two")["wait_reason"]["code"], "resource_capacity");
    assert_eq!(
        task(&s, "three")["wait_reason"],
        json!({"code":"write_conflict","task_id":"one"})
    );
}
#[test]
fn rejects_invalid_resource_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    for capacity in [json!(0), json!(17), json!(-1), json!("3"), Value::Null] {
        let mut req = parallel_request(None);
        req["pool"][0]["resource_capacity"] = capacity;
        assert_eq!(w.start(&req).unwrap_err().code, "invalid_request");
    }
}
#[test]
fn parallel_limit_is_reported_for_routed_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut req = parallel_request(Some(3));
    req["max_parallel"] = json!(1);
    let mut s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap().to_owned();
    for (id, candidate) in [("one", "a"), ("two", "b")] {
        s = send(
            &w,
            &run,
            action(&s, "route", id),
            json!({"status":"ready","candidate_id":candidate}),
        );
    }
    assert_eq!(s["max_parallel"], 1);
    assert_eq!(
        task(&s, "two")["wait_reason"],
        json!({"code":"parallel_limit","limit":1})
    );
}
#[test]
fn parallelism_changes_preserve_claims_and_resume_queued_work() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workflow::new(dir.path().to_path_buf());
    let mut req = parallel_request(Some(3));
    req["max_parallel"] = json!(1);
    let mut s = w.start(&req).unwrap();
    let run = s["run_id"].as_str().unwrap().to_owned();
    for id in ["one", "two"] {
        s = send(
            &w,
            &run,
            action(&s, "route", id),
            json!({"status":"ready","candidate_id":"a"}),
        );
    }
    let first = action(&s, "prepare", "one")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(w.claim(&run, &first).unwrap());
    w.control(&run, "pause").unwrap();
    assert_eq!(
        w.set_parallelism(&run, 0).unwrap_err().code,
        "invalid_request"
    );
    let s = w.set_parallelism(&run, 3).unwrap();
    assert_eq!(s["max_parallel"], 3);
    assert_eq!(s["status"], "paused");
    let s = w.control(&run, "resume").unwrap();
    assert_eq!(action(&s, "prepare", "one")["id"], first);
    assert_eq!(action(&s, "prepare", "two")["kind"], "prepare");
}
