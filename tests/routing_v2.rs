mod common;

use common::{fit_key, judgment, preference_key, score};
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use switchbite::{ContractError, Judge, contracts, routing};

fn request() -> Value {
    serde_json::from_str(include_str!("../examples/route-v2-online.json")).unwrap()
}

fn judge(body: &Value) -> Result<Value, ContractError> {
    Ok(judgment(body))
}

fn checked(request: &Value, judge: Option<&dyn Judge>, now: u64) -> Value {
    let before = request.clone();
    let result = routing::route(request, judge, now).unwrap();
    assert_eq!(before, *request, "routing must not mutate input");
    contracts::validate(&result, &routing::output_schema()).unwrap();
    for key in [
        "version",
        "status",
        "candidate_id",
        "next_action",
        "summary",
        "reason_codes",
        "selected",
        "alternatives",
        "excluded",
        "snapshot",
    ] {
        assert!(result.get(key).is_some(), "missing {key}");
    }
    if result["status"] == "hold" {
        assert!(result["candidate_id"].is_null());
        assert!(result["selected"].is_null());
    }
    result
}

/// Native/evaluation replay builder: agents never compute hashes or typed scores.
/// It records the exact typed response and binds all normalized semantic inputs.
fn bound_replay(request: &Value, now: u64, provider: &dyn Judge) -> Value {
    let captured = Mutex::new(None);
    let recording = |body: &Value| {
        let response = provider.judge(body)?;
        *captured.lock().unwrap() = Some(response.clone());
        Ok(response)
    };
    checked(request, Some(&recording), now);
    let mut replay = routing::snapshot(request, now).unwrap();
    replay["judgment"] = captured
        .into_inner()
        .unwrap()
        .expect("eligible request invokes judge");
    let mut request = request.clone();
    request["replay"] = replay;
    request
}

fn two() -> Value {
    let mut r = request();
    let mut second = r["pool"][0].clone();
    r["pool"][0]["id"] = json!("a");
    second["id"] = json!("b");
    r["pool"].as_array_mut().unwrap().push(second);
    r
}

#[test]
fn minimal_online_and_agent_schema_are_simple() {
    let r = request();
    contracts::validate(&r, &routing::agent_input_schema()).unwrap();
    assert!(
        routing::agent_input_schema()["properties"]
            .get("replay")
            .is_none()
    );
    assert!(
        routing::input_schema()["properties"]
            .get("replay")
            .is_some()
    );
    assert!(
        routing::agent_input_schema()["properties"]["policy"]["properties"]
            .get("judgment_validity_ms")
            .is_none()
    );
    let result = checked(&r, Some(&judge), 1000);
    assert_eq!(result["status"], "ready");
    assert_eq!(result["next_action"], "recheck_and_dispatch");
    assert_eq!(result["selected"]["policy_id"], "balanced");
    assert_eq!(
        result["selected"]["breakdown"]["confidence_kind"],
        "typesafe_score_concentration"
    );
    assert_eq!(result["snapshot"]["expires_at_ms"], 61000);
    assert_eq!(checked(&r, None, 1000)["next_action"], "configure_judge");
}

#[test]
fn normalization_is_stable_and_preserves_unknown_observations() {
    let r = request();
    let normalized = routing::normalize(&r).unwrap();
    assert_eq!(routing::normalize(&normalized).unwrap(), normalized);
    assert_eq!(
        routing::snapshot(&normalized, 10).unwrap(),
        routing::snapshot(&r, 10).unwrap()
    );
    for key in ["estimated_cost", "estimated_latency_ms", "has_context"] {
        assert!(normalized["pool"][0].get(key).is_none());
    }
    let mut explicit = r.clone();
    explicit["version"] = json!(2);
    explicit["task"]["requirements"] = json!([]);
    explicit["task"]["context_refs"] = json!([]);
    explicit["preferences"] = json!("");
    explicit["policy"] = json!({});
    explicit["continuity"] = json!({});
    assert_eq!(
        routing::snapshot(&explicit, 10).unwrap(),
        routing::snapshot(&r, 10).unwrap()
    );
    let mut reordered = two();
    let original = routing::snapshot(&reordered, 10).unwrap();
    reordered["pool"].as_array_mut().unwrap().reverse();
    reordered["pool"][0]["capabilities"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert_eq!(routing::snapshot(&reordered, 10).unwrap(), original);
    for (fit, preference) in [(7.0, 3.0), (0.13, 0.29), (0.001, 99.0), (0.0, 1.0)] {
        let mut weighted = r.clone();
        weighted["policy"] = json!({"fit_weight":fit,"preference_weight":preference});
        let normalized = routing::normalize(&weighted).unwrap();
        assert_eq!(routing::normalize(&normalized).unwrap(), normalized);
    }
}

#[test]
fn strict_bounded_requests_fail_before_judge() {
    let never =
        |_: &Value| -> Result<Value, ContractError> { panic!("judge invoked on invalid request") };
    let mut cases = Vec::new();
    for path in ["unknown", "version", "pool", "policy"] {
        let mut r = request();
        r[path] = match path {
            "version" => json!(1),
            "pool" => json!([]),
            "policy" => json!({"fit_weight":0,"preference_weight":0}),
            _ => json!(true),
        };
        if path != "pool" {
            cases.push(r);
        }
    }
    for field in ["capabilities", "available", "description"] {
        let mut r = request();
        r["pool"][0].as_object_mut().unwrap().remove(field);
        cases.push(r);
    }
    let mut r = request();
    r["task"]["objective"] = json!("x".repeat(24001));
    cases.push(r);
    let mut r = request();
    r["task"]["extra"] = json!(true);
    cases.push(r);
    let mut r = request();
    r["pool"][0]["extra"] = json!(true);
    cases.push(r);
    let mut r = request();
    r["policy"] = json!({"min_fit":1.1});
    cases.push(r);
    let mut r = request();
    r["policy"] = json!({"surprise":true});
    cases.push(r);
    let mut r = request();
    r["continuity"] = json!({"surprise":true});
    cases.push(r);
    let mut r = two();
    r["pool"][1]["id"] = json!("a");
    cases.push(r);
    let mut r = request();
    r["pool"] = json!(
        (0..17)
            .map(|i| {
                let mut c = r["pool"][0].clone();
                c["id"] = json!(format!("c{i}"));
                c
            })
            .collect::<Vec<_>>()
    );
    cases.push(r);
    let mut r = request();
    r["replay"] = json!({"judgment":{}});
    cases.push(r);
    for r in cases {
        assert_eq!(
            routing::route(&r, Some(&never), 0),
            Err(ContractError::InvalidRequest)
        );
    }
    assert_eq!(
        routing::snapshot(&request(), u64::MAX),
        Err(ContractError::InvalidRequest)
    );
    assert_eq!(
        routing::route(&request(), Some(&never), i64::MAX as u64),
        Err(ContractError::InvalidRequest)
    );
}

#[test]
fn replay_is_bound_and_does_not_extend_expiry() {
    let replay = bound_replay(&request(), 1000, &judge);
    let never = |_: &Value| -> Result<Value, ContractError> { panic!("replay invoked judge") };
    let result = checked(&replay, Some(&never), 2000);
    assert_eq!(result["status"], "ready");
    assert_eq!(result["snapshot"]["expires_at_ms"], 61000);
    assert_eq!(
        checked(&replay, None, 61000)["reason_codes"],
        json!(["replay_expired"])
    );
    let mut future = replay.clone();
    future["replay"]["expires_at_ms"] = json!(62001);
    assert_eq!(
        checked(&future, Some(&never), 2000)["reason_codes"],
        json!(["replay_future"])
    );
}

#[test]
fn every_semantic_change_refuses_replay_before_judge() {
    let replay = bound_replay(&request(), 1000, &judge);
    let never =
        |_: &Value| -> Result<Value, ContractError> { panic!("stale replay invoked judge") };
    let mut changes = Vec::new();
    let mut r = replay.clone();
    r["task"]["objective"] = json!("Different objective");
    changes.push(r);
    let mut r = replay.clone();
    r["task"]["id"] = json!("other");
    changes.push(r);
    let mut r = replay.clone();
    r["task"]["requirements"] = json!(["rust"]);
    changes.push(r);
    let mut r = replay.clone();
    r["task"]["context_refs"] = json!(["artifact:2"]);
    changes.push(r);
    let mut r = replay.clone();
    r["pool"][0]["description"] = json!("Different description");
    changes.push(r);
    let mut r = replay.clone();
    r["pool"][0]["capabilities"] = json!(["rust"]);
    changes.push(r);
    let mut r = replay.clone();
    r["pool"][0]["available"] = json!(false);
    changes.push(r);
    let mut r = replay.clone();
    r["pool"][0]["estimated_cost"] = json!(2);
    changes.push(r);
    let mut r = replay.clone();
    r["pool"][0]["estimated_latency_ms"] = json!(2);
    changes.push(r);
    let mut r = replay.clone();
    r["pool"][0]["has_context"] = json!(true);
    changes.push(r);
    let mut r = replay.clone();
    r["preferences"] = json!("Prefer speed");
    changes.push(r);
    let mut r = replay.clone();
    r["policy"] = json!({"min_fit":0.1});
    changes.push(r);
    let mut r = replay.clone();
    r["continuity"] = json!({"in_progress":true});
    changes.push(r);
    let mut r = replay.clone();
    r["replay"]["input_sha256"] = json!("g".repeat(64));
    changes.push(r);
    for r in changes {
        let result = checked(&r, Some(&never), 2000);
        assert_eq!(result["reason_codes"], json!(["replay_mismatch"]));
        assert_eq!(result["next_action"], "refresh_judgment");
    }
}

#[test]
fn gates_do_not_invoke_judge_or_invent_estimates() {
    let calls = AtomicUsize::new(0);
    let counting = |body: &Value| {
        calls.fetch_add(1, Ordering::SeqCst);
        judge(body)
    };
    for (policy, reason) in [
        (json!({"max_cost":1}), "cost_unknown"),
        (json!({"max_latency_ms":1}), "latency_unknown"),
        (
            json!({"excluded_candidate_ids":["rust-worker"]}),
            "explicit_exclusion",
        ),
        (
            json!({"producer_candidate_id":"rust-worker","profile":"review"}),
            "producer_excluded",
        ),
    ] {
        let mut r = request();
        r["policy"] = policy;
        let result = checked(&r, Some(&counting), 0);
        assert_eq!(result["next_action"], "refresh_pool");
        assert!(
            result["excluded"][0]["reasons"]
                .as_array()
                .unwrap()
                .contains(&json!(reason))
        );
    }
    let mut r = request();
    r["pool"][0]["available"] = json!(false);
    assert_eq!(
        checked(&r, Some(&counting), 0)["excluded"][0]["reasons"],
        json!(["unavailable"])
    );
    let mut r = request();
    r["task"]["requirements"] = json!(["missing"]);
    assert_eq!(
        checked(&r, Some(&counting), 0)["excluded"][0]["reasons"],
        json!(["required_capability_absent"])
    );
    let mut r = request();
    r["pool"] = json!([]);
    assert_eq!(
        checked(&r, Some(&counting), 0)["next_action"],
        "refresh_pool"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut r = request();
    r["policy"] = json!({"max_cost":1,"max_latency_ms":10});
    r["pool"][0]["estimated_cost"] = json!(1);
    r["pool"][0]["estimated_latency_ms"] = json!(10);
    assert_eq!(checked(&r, Some(&counting), 0)["status"], "ready");
    r["pool"][0]["estimated_cost"] = json!(2);
    r["pool"][0]["estimated_latency_ms"] = json!(11);
    assert_eq!(
        checked(&r, Some(&counting), 0)["excluded"][0]["reasons"],
        json!(["cost_limit", "latency_limit"])
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn profiles_and_configurable_thresholds_override_legacy_holds() {
    for profile in ["balanced", "quick", "implementation", "review", "mockup"] {
        let mut r = request();
        r["policy"] = json!({"profile":profile});
        assert_eq!(
            checked(&r, Some(&judge), 0)["selected"]["policy_id"],
            profile
        );
    }
    assert!(routing::default_policy("invented").is_err());
    let weak = |body: &Value| {
        let mut response = judgment(body);
        response["answers"]["context_sufficient"]["noul"] = json!(0.2);
        response["answers"][fit_key("rust-worker")] = score(&[0.0, 1.0, 0.0, 0.0]);
        Ok(response)
    };
    let mut r = request();
    r["policy"] = json!({"min_fit":0.3,"min_context":0.1});
    assert_eq!(checked(&r, Some(&weak), 0)["status"], "ready");
    r["policy"]["min_context"] = json!(0.3);
    assert_eq!(checked(&r, Some(&weak), 0)["next_action"], "clarify_task");
    r["policy"]["min_context"] = json!(0.1);
    r["policy"]["min_fit"] = json!(0.4);
    assert_eq!(
        checked(&r, Some(&weak), 0)["reason_codes"],
        json!(["no_fit"])
    );
    let diffuse = |body: &Value| {
        let mut response = judgment(body);
        response["answers"][fit_key("rust-worker")] = score(&[0.25, 0.25, 0.25, 0.25]);
        Ok(response)
    };
    r["policy"] = json!({"min_confidence":0.0});
    assert_eq!(checked(&r, Some(&diffuse), 0)["status"], "ready");
    r["policy"]["min_confidence"] = json!(0.1);
    assert_eq!(
        checked(&r, Some(&diffuse), 0)["next_action"],
        "request_review"
    );
}

#[test]
fn configurable_weights_change_the_choice() {
    let different = |body: &Value| {
        let mut response = judgment(body);
        response["answers"][preference_key("a")] = score(&[1.0, 0.0, 0.0]);
        response["answers"][fit_key("b")] = score(&[0.0, 0.0, 1.0, 0.0]);
        Ok(response)
    };
    let mut r = two();
    r["policy"] = json!({"fit_weight":10,"preference_weight":0});
    assert_eq!(checked(&r, Some(&different), 0)["candidate_id"], "a");
    r["policy"] = json!({"fit_weight":1,"preference_weight":9});
    assert_eq!(checked(&r, Some(&different), 0)["candidate_id"], "b");
}

#[test]
fn ties_hold_by_default_and_explicit_policies_are_deterministic() {
    let mut r = two();
    assert_eq!(checked(&r, Some(&judge), 0)["reason_codes"], json!(["tie"]));
    r["policy"] = json!({"tie_behavior":"stable_id"});
    assert_eq!(checked(&r, Some(&judge), 0)["candidate_id"], "a");
    r["pool"][0]["estimated_cost"] = json!(3);
    r["pool"][1]["estimated_cost"] = json!(1);
    r["policy"]["tie_behavior"] = json!("lowest_cost");
    assert_eq!(checked(&r, Some(&judge), 0)["candidate_id"], "b");
    r["pool"][0]["estimated_latency_ms"] = json!(1);
    r["pool"][1]["estimated_latency_ms"] = json!(3);
    r["policy"]["tie_behavior"] = json!("lowest_latency");
    assert_eq!(checked(&r, Some(&judge), 0)["candidate_id"], "a");
    r["pool"][1]
        .as_object_mut()
        .unwrap()
        .remove("estimated_latency_ms");
    assert_eq!(
        checked(&r, Some(&judge), 0)["reason_codes"],
        json!(["tie_estimate_unknown"])
    );
    r["policy"]["tie_behavior"] = json!("stable_id");
    let original = checked(&r, Some(&judge), 0);
    r["pool"].as_array_mut().unwrap().reverse();
    assert_eq!(checked(&r, Some(&judge), 0), original);
}

#[test]
fn review_excludes_the_producer_before_judgment() {
    let mut r = two();
    r["policy"] = json!({"profile":"review","producer_candidate_id":"a"});
    let inspect = |body: &Value| {
        assert!(
            body["state"]["candidates"]
                .get(switchbite::candidate_key("a"))
                .is_none()
        );
        judge(body)
    };
    let result = checked(&r, Some(&inspect), 0);
    assert_eq!(result["candidate_id"], "b");
    assert_eq!(
        result["excluded"],
        json!([{"candidate_id":"a","reasons":["producer_excluded"]}])
    );
}

#[test]
fn tie_margin_and_continuity_cannot_override_fit_gates() {
    let different = |body: &Value| {
        let mut response = judgment(body);
        response["answers"][preference_key("b")] = score(&[0.0, 1.0, 0.0]);
        Ok(response)
    };
    let mut r = two();
    r["policy"] = json!({"tie_margin":0.2});
    assert_eq!(
        checked(&r, Some(&different), 0)["reason_codes"],
        json!(["tie"])
    );
    r["policy"]["tie_margin"] = json!(0.1);
    assert_eq!(checked(&r, Some(&different), 0)["candidate_id"], "a");
    r["continuity"] = json!({"current_candidate_id":"b"});
    r["policy"] = json!({"switch_margin":1.0,"switch_penalty":1.0,"min_fit":0.9});
    let no_fit_current = |body: &Value| {
        let mut response = judgment(body);
        response["answers"][fit_key("b")] = score(&[0.0, 1.0, 0.0, 0.0]);
        Ok(response)
    };
    assert_eq!(checked(&r, Some(&no_fit_current), 0)["candidate_id"], "a");
    r["pool"][1]["available"] = json!(false);
    assert_eq!(
        checked(&r, Some(&judge), 0)["selected"]["breakdown"]["switch_penalty"],
        0.0
    );
}

#[test]
fn continuity_retains_context_but_never_moves_running_work() {
    let different = |body: &Value| {
        let mut response = judgment(body);
        response["answers"][preference_key("b")] = score(&[0.0, 1.0, 0.0]);
        Ok(response)
    };
    let mut r = two();
    r["continuity"] = json!({"current_candidate_id":"b"});
    r["pool"][1]["has_context"] = json!(true);
    let result = checked(&r, Some(&different), 0);
    assert_eq!(result["candidate_id"], "b");
    assert_eq!(result["reason_codes"], json!(["continuity_retained"]));
    r["pool"][1]["has_context"] = json!(false);
    let no_context = checked(&r, Some(&different), 0);
    assert_eq!(no_context["candidate_id"], "a");
    assert_eq!(no_context["selected"]["breakdown"]["switch_penalty"], 0.0);
    r["pool"][1]["has_context"] = json!(true);
    assert_eq!(
        result["alternatives"][0]["breakdown"]["switch_penalty"],
        0.05
    );
    r["policy"] = json!({"switch_margin":0.01,"switch_penalty":0.0});
    assert_eq!(checked(&r, Some(&different), 0)["candidate_id"], "a");
    r["continuity"]["in_progress"] = json!(true);
    let never = |_: &Value| -> Result<Value, ContractError> { panic!("running work was rerouted") };
    assert_eq!(
        checked(&r, Some(&never), 0)["next_action"],
        "wait_for_completion"
    );
    r["pool"] = json!([]);
    assert_eq!(
        checked(&r, Some(&never), 0)["next_action"],
        "wait_for_completion"
    );
}

#[test]
fn alternatives_are_top_three_and_results_do_not_echo_backend_state() {
    let mut r = request();
    r["pool"] = json!(
        (0..16)
            .map(|i| {
                let mut c = r["pool"][0].clone();
                c["id"] = json!(format!("c{i:02}"));
                c
            })
            .collect::<Vec<_>>()
    );
    r["policy"] = json!({"id":"explicit-policy","tie_behavior":"stable_id"});
    let result = checked(&r, Some(&judge), 0);
    assert_eq!(result["candidate_id"], "c00");
    assert_eq!(result["alternatives"].as_array().unwrap().len(), 3);
    assert_eq!(result["alternatives"][2]["candidate_id"], "c03");
    assert_eq!(result["selected"]["policy_id"], "explicit-policy");
    assert!(result.get("judgment").is_none());
    assert!(result.get("proposals").is_none());
}

#[test]
fn malformed_typed_judgments_and_provider_errors_are_static() {
    let failed = |_: &Value| Err(ContractError::InvalidJudgment);
    assert_eq!(
        routing::route(&request(), Some(&failed), 0),
        Err(ContractError::JudgeFailed)
    );
    let bad = |body: &Value| {
        let mut response = judgment(body);
        response["answers"][fit_key("rust-worker")]["confidence"] = json!(0.0);
        Ok(response)
    };
    assert_eq!(
        routing::route(&request(), Some(&bad), 0),
        Err(ContractError::InvalidJudgment)
    );
}
