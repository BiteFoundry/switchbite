mod common;
use common::*;
use serde_json::{Value, json};
use switchbite::{
    ContractError, Plugin, candidate_key,
    contracts::{self, MAX_JSON_BYTES, MAX_JUDGE_BYTES},
    judgment_body,
};
fn judged() -> Plugin {
    Plugin::with_judge(|body: &Value| Ok(judgment(body)))
}
fn assert_error(op: &str, value: &Value) {
    let e = judged().invoke(op, value).unwrap_err();
    assert!(e.to_string().len() < 100);
    assert!(!e.to_string().contains("SECRET"));
}
#[test]
fn generic_non_coding_assignment_and_exact_question_keys() {
    assert_eq!(
        candidate_key("abc"),
        "cba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let r = request();
    let original = r.clone();
    let b = judgment_body(&r).unwrap();
    assert_eq!(b["model"], "jev-1.14.0");
    assert!(b["state"]["task"].get("extensions").is_none());
    assert!(
        b["state"]["candidates"]
            .as_object()
            .unwrap()
            .values()
            .all(|c| c.get("extensions").is_none())
    );
    let result = judged().invoke("decide", &r).unwrap();
    assert_eq!(result["ready"], true);
    assert_eq!(result["candidate_id"], "translator:ko/en");
    assert_eq!(
        result["proposals"],
        json!([{"type":"assign","task_id":"ticket:42","candidate_id":"translator:ko/en"}])
    );
    assert_eq!(result["calibrated_correctness"], Value::Null);
    assert_eq!(
        result["scores"]["request_sha256"].as_str().unwrap().len(),
        64
    );
    assert_eq!(r, original);
}
#[test]
fn actual_ranking_preference_weight_fit_gate_and_ties() {
    let mut r = request();
    r["candidates"] = json!([candidate("a"), candidate("b"), candidate("c")]);
    let mut j = judgment(&judgment_body(&r).unwrap());
    j["answers"][fit_key("a")] = score(&[1.0, 0.0, 0.0, 0.0]); // Preference cannot rescue zero fit.
    j["answers"][preference_key("b")] = score(&[1.0, 0.0, 0.0]); // fit 1, pref 0 -> .7
    j["answers"][fit_key("c")] = score(&[0.0, 0.0, 1.0, 0.0]); // fit 2/3, pref 1 -> .766666667
    r["judgment"] = j;
    let out = Plugin::default().invoke("decide", &r).unwrap();
    assert_eq!(out["candidate_id"], "c");
    assert_eq!(out["scores"]["order"], json!(["c", "b", "a"]));
    assert_eq!(out["scores"]["eligible"], json!(["c", "b"]));
    assert_eq!(
        out["scores"]["candidates"]["c"]["composite"],
        json!(0.766666667)
    );
    r.as_object_mut().unwrap().remove("judgment");
    r["candidates"] = json!([candidate("z:opaque"), candidate("a/fit_c0")]);
    let one = judged().invoke("decide", &r).unwrap();
    r["candidates"].as_array_mut().unwrap().reverse();
    assert_eq!(one, judged().invoke("decide", &r).unwrap());
    assert_eq!(one["candidate_id"], "a/fit_c0");
    assert_eq!(one["scores"]["tied"], json!(["a/fit_c0", "z:opaque"]));
}
#[test]
fn gates_precede_judge_and_filter_question_set() {
    let forbidden = Plugin::with_judge(|_: &Value| -> Result<Value, ContractError> {
        panic!("judge called past hard gate")
    });
    for (change, reason) in [
        (json!([]), "no_candidates"),
        (json!([{ "available":false}]), "no_available_candidates"),
        (json!([{"capabilities":[]}]), "required_capability_absent"),
    ] {
        let mut r = request();
        if change.as_array().unwrap().is_empty() {
            r["candidates"] = change;
        } else {
            for (k, v) in change[0].as_object().unwrap() {
                r["candidates"][0][k] = v.clone();
            }
        }
        let result = forbidden.invoke("decide", &r).unwrap();
        assert_eq!(result["hold_reasons"], json!([reason]));
        assert_eq!(result["proposals"], json!([]));
    }
    let mut r = request();
    let mut missing = candidate("missing");
    missing["capabilities"] = json!([]);
    let mut unavailable = candidate("unavailable");
    unavailable["available"] = json!(false);
    r["candidates"] = json!([missing, unavailable, candidate("included")]);
    let b = judgment_body(&r).unwrap();
    assert_eq!(b["questions"].as_object().unwrap().len(), 3);
    assert_eq!(
        b["state"]["candidates"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()["id"],
        "included"
    );
    assert_eq!(
        judged().invoke("decide", &r).unwrap()["excluded"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn offline_and_replay_are_network_independent() {
    let r = request();
    assert_eq!(
        Plugin::default().invoke("decide", &r).unwrap()["hold_reasons"],
        json!(["judge_unavailable"])
    );
    let r = replay();
    let forbidden = Plugin::with_judge(|_: &Value| -> Result<Value, ContractError> {
        panic!("replay called provider")
    });
    assert_eq!(
        forbidden.invoke("decide", &r).unwrap(),
        Plugin::default().invoke("decide", &r).unwrap()
    );
}
#[test]
fn typed_judgment_invariants_and_stale_candidate_sets() {
    let r = replay();
    let fit = fit_key("translator:ko/en");
    let mutations = vec![
        (vec!["model".to_owned()], json!("SECRET")),
        (vec!["unknown".into()], json!("SECRET")),
        (vec!["answers".into(), "unknown".into()], json!({})),
        (
            vec!["answers".into(), fit.clone(), "score".into()],
            json!(true),
        ),
        (
            vec!["answers".into(), fit.clone(), "score".into()],
            json!(0.0),
        ),
        (
            vec!["answers".into(), fit.clone(), "confidence".into()],
            json!(0.0),
        ),
        (
            vec!["answers".into(), fit.clone(), "probabilities".into()],
            json!({"0":0.1}),
        ),
        (
            vec!["answers".into(), fit.clone(), "probabilities".into()],
            json!({"0":0.0,"1":0.0,"2":0.0,"3":0.5}),
        ),
        (
            vec!["answers".into(), fit.clone(), "legend".into()],
            json!({"0":"SECRET","1":"SECRET","2":"SECRET","3":"SECRET"}),
        ),
        (
            vec!["answers".into(), fit.clone(), "unknown".into()],
            json!("SECRET"),
        ),
        (
            vec!["answers".into(), "context_sufficient".into(), "type".into()],
            json!("score"),
        ),
        (
            vec!["answers".into(), "context_sufficient".into(), "noul".into()],
            json!(1.1),
        ),
        (vec!["usage".into()], json!({"input_tokens":-1})),
        (vec!["usage".into()], json!({"credential":"SECRET"})),
    ];
    for (path, value) in mutations {
        let mut bad = r.clone();
        let mut target = &mut bad["judgment"];
        for part in &path[..path.len() - 1] {
            target = &mut target[part];
        }
        target[path.last().unwrap()] = value;
        assert_error("decide", &bad);
    }
    for candidates in [
        json!([candidate("different-id")]),
        json!([candidate("a"), candidate("b")]),
    ] {
        let mut bad = r.clone();
        bad["candidates"] = candidates;
        assert_eq!(
            Plugin::default().invoke("decide", &bad).unwrap_err(),
            ContractError::InvalidJudgment
        );
    }
    let mut bad = r.clone();
    bad["candidates"] = json!([]);
    bad["judgment"]["unknown"] = json!(true);
    assert_error("decide", &bad);
    let broken = Plugin::with_judge(|_: &Value| Err(ContractError::InvalidJudgment));
    assert_eq!(
        broken.invoke("decide", &request()).unwrap_err(),
        ContractError::JudgeFailed
    );
}
#[test]
fn context_fit_confidence_and_boundary_thresholds() {
    for (context, fit, hold) in [
        (0.49, vec![0., 0., 0., 1.], "context_insufficient"),
        (1., vec![1., 0., 0., 0.], "no_fit"),
        (1., vec![0.25; 4], "low_confidence"),
    ] {
        let mut r = replay();
        r["judgment"]["answers"]["context_sufficient"]["noul"] = json!(context);
        r["judgment"]["answers"][fit_key("translator:ko/en")] = score(&fit);
        let out = Plugin::default().invoke("decide", &r).unwrap();
        assert_eq!(out["hold_reasons"], json!([hold]));
        assert_eq!(out["ready"], false);
    }
    let mut r = replay();
    r["judgment"]["answers"]["context_sufficient"]["noul"] = json!(0.5);
    r["judgment"]["answers"][fit_key("translator:ko/en")] = score(&[0., 0.5, 0.5, 0.]);
    assert_eq!(
        Plugin::default().invoke("decide", &r).unwrap()["ready"],
        true
    );
}
#[test]
fn malformed_oversized_unknown_and_exact_json_types() {
    for raw in [
        br#"{"version":1,"version":1}"#.as_slice(),
        br#"{"x":NaN}"#,
        br#"{"x":1e400}"#,
        br#"{"x":9223372036854775808}"#,
        br#"{"x":-9223372036854775809}"#,
        br#"{"x":184467440737095516160000000000000}"#,
        br#"{"x":"\ud800"}"#,
        b"[] trailing",
        b"",
    ] {
        assert!(contracts::parse_request(raw).is_err());
    }
    assert!(contracts::parse_request(&vec![b' '; MAX_JSON_BYTES + 1]).is_err());
    let exact = contracts::parse_request(br#"{"min":-9223372036854775808,"max":9223372036854775807,"text":"184467440737095516160 \"quote\"","float":1.5e100}"#).unwrap();
    assert_eq!(exact["min"].as_i64(), Some(i64::MIN));
    assert_eq!(exact["max"].as_i64(), Some(i64::MAX));
    for v in [
        json!(0),
        json!(2),
        json!(true),
        json!(1.0),
        json!("1"),
        Value::Null,
    ] {
        let mut r = request();
        r["version"] = v;
        assert_error("decide", &r);
    }
    assert_eq!(
        Plugin::default().invoke("SECRET", &request()).unwrap_err(),
        ContractError::UnknownOperation
    );
    let mut r = request();
    r["task"]["SECRET"] = json!("SECRET");
    assert_error("decide", &r);
    let mut r = request();
    r["candidates"][0]["command"] = json!("SECRET");
    assert_error("decide", &r);
    let mut r = request();
    r["candidates"] = json!([candidate("same"), candidate("same")]);
    assert_error("decide", &r);
    let mut r = request();
    r["preferences"] = json!("x".repeat(8001));
    assert_error("decide", &r);
    let mut r = request();
    r["candidates"] = json!(
        (0..17)
            .map(|i| candidate(&i.to_string()))
            .collect::<Vec<_>>()
    );
    assert_error("decide", &r);
    let mut deep = json!({});
    for _ in 0..14 {
        deep = json!({"nested":deep});
    }
    for ext in [
        json!({"x":u64::MAX}),
        json!({"x":"x".repeat(24001)}),
        json!({"x":vec![0;129]}),
        deep,
        json!(
            (0..70)
                .map(|i| (i.to_string(), json!("x".repeat(1000))))
                .collect::<serde_json::Map<_, _>>()
        ),
        json!({"":"empty key"}),
        json!(
            (0..129)
                .map(|i| (i.to_string(), json!(0)))
                .collect::<serde_json::Map<_, _>>()
        ),
        json!(
            (0..64)
                .map(|i| (i.to_string(), json!(vec![false; 128])))
                .collect::<serde_json::Map<_, _>>()
        ),
    ] {
        let mut r = request();
        r["extensions"] = ext;
        assert_error("decide", &r);
    }
    let mut r = request();
    r["task"]["objective"] = json!("   ");
    assert_error("decide", &r);
}
#[test]
fn valid_large_request_allows_generated_question_overhead() {
    let mut r = request();
    r["task"]["objective"] = json!("x".repeat(24000));
    r["task"]["requirements"] = json!([]);
    r["preferences"] = json!("p".repeat(6000));
    r["candidates"]=json!((0..16).map(|i|json!({"id":format!("candidate-{i}"),"description":"d".repeat(2000),"capabilities":[],"available":true,"extensions":{}})).collect::<Vec<_>>());
    contracts::bounded(&r, MAX_JSON_BYTES).unwrap();
    let b = judgment_body(&r).unwrap();
    assert!(serde_json::to_vec(&b).unwrap().len() > MAX_JSON_BYTES);
    contracts::bounded(&b, MAX_JUDGE_BYTES).unwrap();
    assert_eq!(judged().invoke("decide", &r).unwrap()["ready"], true);
}
#[test]
fn dag_order_and_all_structural_failures() {
    let p = json!({"version":1,"nodes":[node("send",&["translate"]),node("translate",&[]),node("review",&["translate"])]});
    let result = Plugin::default().invoke("validate_plan", &p).unwrap();
    assert_eq!(
        result["topological_order"],
        json!(["translate", "review", "send"])
    );
    assert_eq!(result["valid"], true);
    let mut reversed = p.clone();
    reversed["nodes"].as_array_mut().unwrap().reverse();
    assert_eq!(
        result,
        Plugin::default()
            .invoke("validate_plan", &reversed)
            .unwrap()
    );
    for (nodes, issue) in [
        (
            vec![node("a", &["b"]), node("b", &["a"])],
            "dependency_cycle",
        ),
        (vec![node("a", &["missing"])], "unknown_dependency"),
        (vec![node("a", &["a"])], "self_dependency"),
        (vec![node("a", &[]), node("a", &[])], "duplicate_node_id"),
    ] {
        let out = Plugin::default()
            .invoke("validate_plan", &json!({"version":1,"nodes":nodes}))
            .unwrap();
        assert_eq!(out["valid"], false);
        assert!(out["issues"].as_array().unwrap().contains(&json!(issue)));
        assert_eq!(out["proposals"], json!([]));
        assert_eq!(out["topological_order"], json!([]));
    }
    assert_error(
        "validate_plan",
        &json!({"version":1,"nodes":[node("a",&["b","b"]),node("b",&[])]}),
    );
    assert_error("validate_plan", &json!({"version":1,"nodes":[]}));
}
#[test]
fn feedback_normalizes_evidence_and_bounds_the_response() {
    let f = feedback();
    let mut out = Plugin::default().invoke("feedback", &f).unwrap();
    assert_eq!(out["event"]["extensions"], json!({}));
    assert_eq!(out["event"]["type"], "feedback");
    assert_eq!(out["proposals"], json!([]));
    out["event"]["evidence"]
        .as_array_mut()
        .unwrap()
        .push(json!("receipt:later"));
    assert_eq!(f["evidence"], json!(["receipt:host/42"]));
    for change in [
        json!({"evidence":[]}),
        json!({"evidence":[""]}),
        json!({"evidence":["same","same"]}),
        json!({"outcome":"SECRET"}),
        json!({"write_to":"SECRET"}),
    ] {
        let mut bad = f.clone();
        for (k, v) in change.as_object().unwrap() {
            bad[k] = v.clone();
        }
        assert_error("feedback", &bad);
    }
    let mut large = feedback();
    large["extensions"] =
        json!({"a":"x".repeat(22000),"b":"x".repeat(22000),"c":"x".repeat(20000)});
    let remaining = MAX_JSON_BYTES - serde_json::to_vec(&large).unwrap().len() - 20;
    large["extensions"]["c"] = json!("x".repeat(20000 + remaining));
    contracts::bounded(&large, MAX_JSON_BYTES).unwrap();
    assert_error("feedback", &large);
}
#[test]
fn describe_and_embedded_schema_are_consistent() {
    let d = contracts::describe();
    assert_eq!(d["plugin_id"], "switchbite");
    assert_eq!(d["operations"].as_object().unwrap().len(), 3);
    for (op, data) in d["operations"].as_object().unwrap() {
        assert_eq!(data["required_capabilities"], json!([]));
        for direction in ["input", "output"] {
            assert_eq!(
                &data[format!("{direction}_schema")],
                contracts::operation_schema(op, direction).unwrap()
            );
        }
    }
}
