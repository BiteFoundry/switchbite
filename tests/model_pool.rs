use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use switchbite::{ContractError, Plugin, candidate_key};

fn pool() -> Value {
    serde_json::from_slice(include_bytes!("../examples/model-pool-replay.json")).unwrap()
}

#[test]
fn model_pool_preserves_distinct_harness_profiles_and_excludes_ineligible_targets() {
    let request = pool();
    let original = request.clone();
    let result = Plugin::default().invoke("decide", &request).unwrap();
    assert_eq!(result["ready"], true);
    assert_eq!(result["candidate_id"], "pool:claude:implementation");
    assert_eq!(
        result["proposals"],
        json!([{
            "type":"assign", "task_id":"demo:rust-feature", "candidate_id":"pool:claude:implementation"
        }])
    );
    for id in ["pool:codex:review", "pool:codex:general"] {
        assert!(result["scores"]["candidates"][id].is_object());
    }
    assert_eq!(
        result["excluded"],
        json!([
            {"candidate_id":"pool:antigravity:writing", "reasons":["required_capability_absent"]},
            {"candidate_id":"pool:claude:busy", "reasons":["unavailable"]}
        ])
    );
    assert_eq!(request, original);
    let mut reordered = request.clone();
    reordered["candidates"].as_array_mut().unwrap().reverse();
    // Host-only mapping and cost metadata are deliberately not optimization inputs.
    reordered["candidates"][0]["extensions"] =
        json!({"cost":9999,"target":"different-host-handle"});
    assert_eq!(
        result,
        Plugin::default().invoke("decide", &reordered).unwrap()
    );
}

#[test]
fn model_pool_uses_only_eligible_targets_for_an_injected_judge() {
    let mut request = pool();
    let replay = request.as_object_mut().unwrap().remove("judgment").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let plugin = Plugin::with_judge(move |body: &Value| {
        observed.fetch_add(1, Ordering::SeqCst);
        let targets = body["state"]["candidates"].as_object().unwrap();
        assert_eq!(targets.len(), 3);
        for id in [
            "pool:codex:review",
            "pool:codex:general",
            "pool:claude:implementation",
        ] {
            let target = &targets[&candidate_key(id)];
            assert_eq!(target["id"], id);
            assert!(target.get("extensions").is_none());
            assert!(target.get("available").is_none());
        }
        for id in ["pool:claude:busy", "pool:antigravity:writing"] {
            assert!(!targets.contains_key(&candidate_key(id)));
        }
        Ok(replay.clone())
    });
    let result = plugin.invoke("decide", &request).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(result["candidate_id"], "pool:claude:implementation");
    assert_eq!(
        Plugin::default().invoke("decide", &request).unwrap()["hold_reasons"],
        json!(["judge_unavailable"])
    );
}

#[test]
fn changed_model_pool_refuses_a_replay_with_a_different_eligible_set() {
    let mut request = pool();
    request["candidates"][3]["available"] = json!(true);
    assert_eq!(
        Plugin::default().invoke("decide", &request),
        Err(ContractError::InvalidJudgment)
    );
}
