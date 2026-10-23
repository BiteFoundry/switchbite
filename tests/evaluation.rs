use serde_json::{Value, json};
use switchbite::{
    ContractError, contracts,
    evaluation::{evaluate, input_schema, output_schema},
};

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!("../examples/evaluation-synthetic.json")).unwrap()
}

fn minimal() -> Value {
    json!({"kind":"observed", "cases":[{"id":"one", "acceptable_candidate_ids":["a"],
        "expected_hold":false, "baseline_candidates":{"manual":"b","strongest":"c"}}],
        "decisions":[], "observations":[]})
}

fn reject(value: &Value) {
    assert_eq!(
        evaluate(value),
        Err(ContractError::InvalidRequest),
        "{value}"
    );
}

#[test]
fn fixture_is_explicitly_synthetic_pure_and_schema_valid() {
    let input = fixture();
    let original = input.clone();
    contracts::validate(&input, &input_schema()).unwrap();
    let result = evaluate(&input).unwrap();
    contracts::validate(&result, &output_schema()).unwrap();
    assert_eq!(input, original);
    assert_eq!(result["kind"], "synthetic");
    assert_eq!(result["evidence"], "synthetic_fixture");
    assert_eq!(result["proposal_only"], true);
    assert_eq!(result["routing"]["agreement"]["matched_count"], 5);
    assert_eq!(result["routing"]["agreement"]["value"], 1.0);
    assert_eq!(result["routing"]["hold_coverage"]["value"], 1.0);
    // Correct route selection does not imply a successful execution.
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["success"],
        json!({"sample_count":3,"missing_count":2,"value":2.0/3.0})
    );
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["duration_ms"],
        json!({"sample_count":4,"missing_count":1,"value":1025.0})
    );
    let mut reordered = input;
    for key in ["cases", "decisions", "observations"] {
        reordered[key].as_array_mut().unwrap().reverse();
    }
    assert_eq!(evaluate(&reordered).unwrap(), result);
}

#[test]
fn paired_metrics_use_only_the_same_tasks_and_per_metric_coverage() {
    let mut input = minimal();
    input["cases"].as_array_mut().unwrap().push(json!({"id":"two",
        "acceptable_candidate_ids":["a"],"expected_hold":false,"baseline_candidates":{"manual":"b"}}));
    input["decisions"] = json!([
        {"case_id":"one","status":"ready","candidate_id":"a","duration_ms":10,"quota_units":0},
        {"case_id":"two","status":"ready","candidate_id":"a","duration_ms":1000}
    ]);
    input["observations"] = json!([
        {"case_id":"one","candidate_id":"a","outcome":"passed"},
        {"case_id":"one","candidate_id":"b","outcome":"failed","duration_ms":20,"quota_units":4,"rework_count":1},
        {"case_id":"two","candidate_id":"a","outcome":"failed"}
    ]);
    let result = evaluate(&input).unwrap();
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["duration_ms"]["value"],
        505.0
    );
    assert_eq!(
        result["paired_comparisons"]["manual"]["duration_ms"],
        json!({
        "sample_count":1,"missing_count":1,"switchbite_mean":10.0,"baseline_mean":20.0,
        "delta":-10.0,"case_ids":["one"]})
    );
    assert_eq!(
        result["paired_comparisons"]["manual"]["success"]["delta"],
        1.0
    );
    assert_eq!(
        result["paired_comparisons"]["manual"]["quota_units"]["delta"],
        -4.0
    );
    assert_eq!(
        result["paired_comparisons"]["manual"]["rework_count"]["sample_count"],
        0
    );
    assert!(result["paired_comparisons"]["manual"]["rework_count"]["delta"].is_null());
}

#[test]
fn absent_outcomes_never_become_failures_or_zero_costs() {
    let mut input = minimal();
    input["decisions"] =
        json!([{"case_id":"one","status":"ready","candidate_id":"a","duration_ms":0}]);
    let result = evaluate(&input).unwrap();
    assert_eq!(result["kind"], "observed");
    assert_eq!(result["evidence"], "imported_unverified");
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["duration_ms"]["value"],
        0.0
    );
    for strategy in ["switchbite", "manual", "strongest"] {
        assert_eq!(
            result["strategies"][strategy]["metrics"]["success"],
            json!({"sample_count":0,"missing_count":1,"value":null})
        );
    }
    assert_eq!(result["routing"]["hold_coverage"]["value"], Value::Null);
}

#[test]
fn all_held_and_partial_decisions_have_explicit_coverage() {
    let mut input = minimal();
    input["cases"][0]["expected_hold"] = json!(true);
    input["cases"][0]["acceptable_candidate_ids"] = json!([]);
    input["cases"][0]["baseline_candidates"] = json!({});
    input["cases"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"two", "expected_hold":true,
        "acceptable_candidate_ids":[],"baseline_candidates":{}}));
    input["decisions"] = json!([{"case_id":"one","status":"hold"}]);
    let result = evaluate(&input).unwrap();
    assert_eq!(
        result["routing"]["agreement"],
        json!({"sample_count":1,"missing_count":1,"matched_count":1,"value":1.0})
    );
    assert_eq!(
        result["routing"]["hold_coverage"],
        json!({"expected_count":2,
        "observed_count":1,"correctly_held_count":1,"missing_count":1,"value":0.5})
    );
    assert_eq!(result["strategies"]["switchbite"]["selected_count"], 0);
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["success"]["missing_count"],
        2
    );
    assert!(result["paired_comparisons"]["manual"]["duration_ms"]["delta"].is_null());
    input["decisions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"case_id":"two","status":"hold"}));
    let all_held = evaluate(&input).unwrap();
    assert_eq!(all_held["routing"]["hold_coverage"]["value"], 1.0);
    assert_eq!(
        all_held["strategies"]["switchbite"]["metrics"]["duration_ms"],
        json!({"sample_count":0,"missing_count":2,"value":null})
    );
}

#[test]
fn baseline_only_observations_never_create_a_switchbite_outcome() {
    let mut input = minimal();
    input["observations"] = json!([
        {"case_id":"one","candidate_id":"a","outcome":"passed","duration_ms":9},
        {"case_id":"one","candidate_id":"b","outcome":"failed","duration_ms":20}
    ]);
    let result = evaluate(&input).unwrap();
    assert_eq!(
        result["strategies"]["manual"]["metrics"]["success"]["value"],
        0.0
    );
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["success"]["value"],
        Value::Null
    );
    assert_eq!(
        result["paired_comparisons"]["manual"]["duration_ms"]["sample_count"],
        0
    );
}

#[test]
fn empty_import_template_and_absent_decisions_remain_unknown() {
    let input: Value = serde_json::from_slice(include_bytes!(
        "../examples/evaluation-import-template.json"
    ))
    .unwrap();
    let result = evaluate(&input).unwrap();
    assert_eq!(result["routing"]["agreement"]["value"], Value::Null);
    assert_eq!(result["routing"]["agreement"]["missing_count"], 1);
    assert_eq!(result["cases"][0]["route_agrees"], Value::Null);
}

#[test]
fn incorrect_route_can_have_passed_execution_and_unexpected_hold_is_counted() {
    let mut input = minimal();
    input["decisions"] = json!([{"case_id":"one","status":"ready","candidate_id":"wrong"}]);
    input["observations"] = json!([{"case_id":"one","candidate_id":"wrong","outcome":"passed"}]);
    let result = evaluate(&input).unwrap();
    assert_eq!(result["routing"]["agreement"]["value"], 0.0);
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["success"]["value"],
        1.0
    );
    input["decisions"] = json!([{"case_id":"one","status":"hold"}]);
    input["observations"] = json!([]);
    assert_eq!(
        evaluate(&input).unwrap()["routing"]["unexpected_hold_count"],
        1
    );
}

#[test]
fn same_candidate_baseline_reuses_evidence_and_partial_metrics_merge() {
    let mut input = minimal();
    input["cases"][0]["baseline_candidates"]["manual"] = json!("a");
    input["decisions"] =
        json!([{"case_id":"one","status":"ready","candidate_id":"a","duration_ms":10}]);
    input["observations"] = json!([{"case_id":"one","candidate_id":"a","outcome":"passed","duration_ms":10.0,"quota_units":2}]);
    let result = evaluate(&input).unwrap();
    assert_eq!(
        result["paired_comparisons"]["manual"]["duration_ms"]["delta"],
        0.0
    );
    assert_eq!(
        result["strategies"]["switchbite"]["metrics"]["quota_units"]["value"],
        2.0
    );
    input["observations"][0]["duration_ms"] = json!(11);
    reject(&input);
}

#[test]
fn duplicate_case_decision_and_observation_entries_are_rejected() {
    for key in ["cases", "decisions", "observations"] {
        let mut input = fixture();
        let duplicate = input[key][0].clone();
        input[key].as_array_mut().unwrap().push(duplicate);
        reject(&input);
    }
    let mut input = minimal();
    input["cases"][0]["acceptable_candidate_ids"] = json!(["a", "a"]);
    reject(&input);
}

#[test]
fn unknown_case_candidate_and_cross_task_observation_joins_are_rejected() {
    let mut input = fixture();
    input["decisions"][0]["case_id"] = json!("unknown");
    reject(&input);
    for (field, value) in [
        ("case_id", "unknown"),
        ("candidate_id", "unknown"),
        ("candidate_id", "designer"),
    ] {
        let mut input = fixture();
        input["observations"][0][field] = json!(value);
        reject(&input);
    }
}

#[test]
fn unknown_fields_and_malformed_types_are_rejected_at_every_level() {
    for pointer in [
        "",
        "/cases/0",
        "/cases/0/baseline_candidates",
        "/decisions/0",
        "/observations/0",
    ] {
        let mut input = fixture();
        input.pointer_mut(pointer).unwrap()["extra"] = json!(true);
        reject(&input);
    }
    for (pointer, value) in [
        ("/kind", json!("live")),
        ("/cases/0/id", json!(" ")),
        ("/cases/0/task_type", json!("unknown")),
        ("/cases/0/sample", json!("production")),
        ("/cases/0/expected_hold", json!("false")),
        ("/decisions/0/status", json!("running")),
        ("/observations/0/outcome", json!("unknown")),
        ("/observations/0/quota_units", Value::Null),
    ] {
        let mut input = fixture();
        *input.pointer_mut(pointer).unwrap() = value;
        reject(&input);
    }
}

#[test]
fn contradictory_holds_and_missing_ready_candidate_are_rejected() {
    let mut input = minimal();
    input["cases"][0]["acceptable_candidate_ids"] = json!([]);
    reject(&input);
    let mut input = minimal();
    input["cases"][0]["expected_hold"] = json!(true);
    reject(&input);
    for decision in [
        json!({"case_id":"one","status":"ready"}),
        json!({"case_id":"one","status":"hold","candidate_id":"a"}),
        json!({"case_id":"one","status":"hold","duration_ms":0}),
    ] {
        let mut input = minimal();
        input["decisions"] = json!([decision]);
        reject(&input);
    }
}

#[test]
fn numeric_bounds_and_integral_rework_are_enforced() {
    for key in ["decisions", "observations"] {
        for (metric, value) in [
            ("duration_ms", json!(-1)),
            ("quota_units", json!(1e30)),
            ("rework_count", json!(0.5)),
            ("rework_count", json!(1_000_001)),
            ("duration_ms", json!("10")),
            ("quota_units", json!(true)),
        ] {
            let mut input = fixture();
            input[key][0][metric] = value;
            reject(&input);
        }
    }
}

#[test]
fn bounded_batches_and_large_valid_measurements_are_supported() {
    let mut input = minimal();
    input["cases"] = json!((0..32).map(|i| json!({"id":format!("case-{i}"),
        "expected_hold":false,"acceptable_candidate_ids":["a"],"baseline_candidates":{"manual":"b"}})).collect::<Vec<_>>());
    input["decisions"] = json!(
        (0..32)
            .map(|i| json!({"case_id":format!("case-{i}"),
        "status":"ready","candidate_id":"a","duration_ms":1e12}))
            .collect::<Vec<_>>()
    );
    input["observations"] = json!(
        (0..32)
            .map(|i| json!({"case_id":format!("case-{i}"),
        "candidate_id":"b","outcome":"passed","duration_ms":0}))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        evaluate(&input).unwrap()["paired_comparisons"]["manual"]["duration_ms"]["delta"],
        1e12
    );
    let extra = input["cases"][0].clone();
    input["cases"].as_array_mut().unwrap().push(extra);
    reject(&input);
    let mut empty = minimal();
    empty["cases"] = json!([]);
    reject(&empty);
}

#[test]
fn byte_parser_rejects_duplicate_json_members_before_value_ingestion() {
    assert!(
        contracts::parse_json(
            br#"{"kind":"observed","kind":"synthetic"}"#,
            contracts::MAX_JSON_BYTES
        )
        .is_err()
    );
}
