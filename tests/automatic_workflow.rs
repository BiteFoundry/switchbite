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
    assert_eq!(
        switchbite::route_context::explicit_route(&a).unwrap()["candidate_id"],
        "codex:2"
    );
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
fn automatic_choices_still_require_judgment() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Workflow::new(dir.path().to_path_buf());
    let state = engine.start(&json!({"workspace":"synthetic","pool":[candidate(1,"model-a")],"plan":{"nodes":[{"id":"work","objective":"Choose a target"}]}})).unwrap();
    assert!(switchbite::route_context::explicit_route(&action(&state, "route", "work")).is_none());
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

#[test]
fn pool_refresh_preserves_a_claimed_route_and_its_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Workflow::new(dir.path().to_path_buf());
    let pool = json!([candidate(1, "model-a"), candidate(2, "model-b")]);
    let state = engine.start(&json!({"workspace":"synthetic","pool":pool,"max_parallel":2,"plan":{"nodes":[{"id":"a","objective":"A"},{"id":"b","objective":"B"}]}})).unwrap();
    let run = state["run_id"].as_str().unwrap();
    let leased = action(&state, "route", "b");
    assert!(engine.claim(run, leased["id"].as_str().unwrap()).unwrap());
    let state = observe(
        &engine,
        run,
        &action(&state, "route", "a"),
        json!({"status":"hold"}),
    );
    let state = observe(
        &engine,
        run,
        &action(&state, "refresh_pool", "a"),
        json!({"pool":pool}),
    );
    assert_eq!(action(&state, "route", "b"), leased);
    let state = observe(
        &engine,
        run,
        &leased,
        json!({"status":"ready","candidate_id":"codex:2"}),
    );
    assert_eq!(state["tasks"][1]["status"], "preparing");
    assert_eq!(state["tasks"][1]["candidate"]["model"], "model-b");
}

#[test]
fn a_claimed_route_removed_from_the_current_pool_reroutes_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Workflow::new(dir.path().to_path_buf());
    let state = engine.start(&json!({"workspace":"synthetic","pool":[candidate(1,"model-a"),candidate(2,"model-b")],"max_parallel":2,"plan":{"nodes":[{"id":"a","objective":"A"},{"id":"b","objective":"B"}]}})).unwrap();
    let run = state["run_id"].as_str().unwrap();
    let leased = action(&state, "route", "b");
    engine.claim(run, leased["id"].as_str().unwrap()).unwrap();
    let state = observe(
        &engine,
        run,
        &action(&state, "route", "a"),
        json!({"status":"hold"}),
    );
    let state = observe(
        &engine,
        run,
        &action(&state, "refresh_pool", "a"),
        json!({"pool":[candidate(1,"model-a")]}),
    );
    assert_eq!(action(&state, "route", "b"), leased);
    let state = observe(
        &engine,
        run,
        &leased,
        json!({"status":"ready","candidate_id":"codex:2"}),
    );
    assert!(state["tasks"][1]["candidate"].is_null());
    let next = action(&state, "route", "b");
    assert_ne!(next["id"], leased["id"]);
    assert_eq!(
        next["payload"]["candidate_snapshot"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn review_with_seventeen_producers_keeps_an_eligible_shortlist() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Workflow::new(dir.path().join("state"));
    let mut nodes: Vec<Value> = (0..17).map(|i| json!({"id":format!("t{i}"),"objective":"Implement a bounded file","role":"implementation","routing":{"candidate_id":format!("codex:{i}")}})).collect();
    nodes.push(json!({"id":"review","objective":"Review integrated outputs","role":"review","depends_on":(0..17).map(|i|format!("t{i}")).collect::<Vec<_>>()}));
    let state = engine.start(&json!({"workspace":dir.path(),"catalog":{"providers":[]},"plan":{"nodes":nodes},"max_parallel":16})).unwrap();
    let run = state["run_id"].as_str().unwrap();
    let state = observe(
        &engine,
        run,
        &action(&state, "route", "t0"),
        json!({"status":"hold"}),
    );
    let mut state = observe(
        &engine,
        run,
        &action(&state, "refresh_pool", "t0"),
        json!({"catalog":catalog((0..18).map(|i|candidate(i,"m")).collect(),"all")}),
    );
    for _ in 0..200 {
        let a = state["actions"]
            .as_array()
            .unwrap()
            .first()
            .expect("work remains")
            .clone();
        if a["task_id"] == "review" && a["kind"] == "route" {
            assert_eq!(a["payload"]["route_input"]["pool"][0]["id"], "codex:17");
            assert!(
                a["payload"]["route_input"]["policy"]["excluded_candidate_ids"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            return;
        }
        let result = match a["kind"].as_str().unwrap() {
            "route" => {
                json!({"status":"ready","candidate_id":a["payload"]["candidate_snapshot"][0]["id"]})
            }
            "prepare" => {
                json!({"status":"prepared","workspace":"w","branch":"b","integration_workspace":"i","integration_branch":"ib"})
            }
            "launch" => {
                json!({"status":"completed","completion_verified":true,"execution_id":a["id"]})
            }
            "verify" => json!({"status":"passed","evidence":[]}),
            "integrate" => json!({"status":"integrated"}),
            kind => panic!("unexpected {kind}"),
        };
        state = observe(&engine, run, &a, result);
    }
    panic!("review did not become ready");
}

#[test]
fn quota_failure_reassigns_automatic_work_and_holds_an_explicit_target() {
    for pinned in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let engine = Workflow::new(dir.path().to_path_buf());
        let mut node = json!({"id":"work","objective":"Write a config"});
        if pinned {
            node["routing"] = json!({"candidate_id":"codex:1"});
        }
        let state=engine.start(&json!({"workspace":"synthetic","pool":[candidate(1,"a"),candidate(2,"b")],"plan":{"nodes":[node]}})).unwrap();
        let run = state["run_id"].as_str().unwrap();
        let state = observe(
            &engine,
            run,
            &action(&state, "route", "work"),
            json!({"status":"ready","candidate_id":"codex:1"}),
        );
        let state = observe(
            &engine,
            run,
            &action(&state, "prepare", "work"),
            json!({"status":"prepared","workspace":"w","branch":"b","integration_workspace":"i","integration_branch":"ib"}),
        );
        let a = action(&state, "launch", "work");
        let state = observe(
            &engine,
            run,
            &a,
            json!({"status":"failed","execution_id":a["id"],"completion_verified":false,"error":{"code":"rate_limited"}}),
        );
        assert!(
            state["actions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["kind"] != "repair")
        );
        if pinned {
            assert_eq!(
                action(&state, "refresh_pool", "work")["kind"],
                "refresh_pool"
            );
        } else {
            let route = action(&state, "route", "work");
            assert_eq!(
                route["payload"]["candidate_snapshot"][0]["available"],
                false
            );
            let state = observe(
                &engine,
                run,
                &route,
                json!({"status":"ready","candidate_id":"codex:2"}),
            );
            assert_eq!(state["tasks"][0]["candidate"]["id"], "codex:2");
        }
    }
}
