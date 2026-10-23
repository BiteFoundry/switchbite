//! Offline evaluation of imported routing decisions and execution observations.
//! No provider calls, worker execution, filesystem access, or policy mutation.
use crate::contracts::{self, ContractError, MAX_JSON_BYTES};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_CASES: usize = 32;
const METRICS: [&str; 4] = ["success", "duration_ms", "quota_units", "rework_count"];
const STRATEGIES: [&str; 3] = ["switchbite", "manual", "strongest"];

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object", "additionalProperties":false,
        "properties":properties, "required":required})
}

fn id_schema() -> Value {
    json!({"type":"string", "minLength":1, "maxLength":128})
}

fn number_schema() -> Value {
    json!({"type":"number", "minimum":0, "maximum":1_000_000_000_000_u64})
}

/// Strict structural schema; evaluate also checks uniqueness and relational joins.
pub fn input_schema() -> Value {
    let metrics = json!({
        "duration_ms":number_schema(), "quota_units":number_schema(),
        "rework_count":{"type":"integer", "minimum":0, "maximum":1_000_000}
    });
    let mut decision = metrics.clone();
    decision["case_id"] = id_schema();
    decision["status"] = json!({"type":"string", "enum":["ready","hold"]});
    decision["candidate_id"] = id_schema();
    decision["policy_id"] = id_schema();
    decision["reason"] = json!({"type":"string", "minLength":1, "maxLength":512});
    let mut observation = metrics;
    observation["case_id"] = id_schema();
    observation["candidate_id"] = id_schema();
    observation["outcome"] = json!({"type":"string", "enum":["passed","failed"]});
    let case = object(
        json!({
            "id":id_schema(),
            "acceptable_candidate_ids":{"type":"array", "maxItems":16,
                "uniqueItems":true, "items":id_schema()},
            "expected_hold":{"type":"boolean"},
            "baseline_candidates":object(json!({"manual":id_schema(), "strongest":id_schema()}), &[]),
            "task_type":{"type":"string", "enum":["implementation","review","mockup","quick"]},
            "sample":{"type":"string", "enum":["bootstrap","local"]}
        }),
        &[
            "id",
            "acceptable_candidate_ids",
            "expected_hold",
            "baseline_candidates",
        ],
    );
    object(
        json!({
            "kind":{"type":"string", "enum":["synthetic","observed"]},
            "cases":{"type":"array", "minItems":1, "maxItems":MAX_CASES, "items":case},
            "decisions":{"type":"array", "maxItems":MAX_CASES,
                "items":object(decision, &["case_id","status"])},
            "observations":{"type":"array", "maxItems":128,
                "items":object(observation, &["case_id","candidate_id","outcome"])}
        }),
        &["kind", "cases", "decisions", "observations"],
    )
}

fn nullable(schema: Value) -> Value {
    json!({"anyOf":[schema, {"type":"null"}]})
}

/// Output schema for hosts exposing evaluation alongside their own commands/tools.
pub fn output_schema() -> Value {
    let count = json!({"type":"integer", "minimum":0, "maximum":MAX_CASES});
    let signed = json!({"type":"number", "minimum":-1_000_000_000_000_i64,
        "maximum":1_000_000_000_000_u64});
    let rate = nullable(json!({"type":"number", "minimum":0, "maximum":1}));
    let metric = object(
        json!({"sample_count":count, "missing_count":count,
        "value":nullable(number_schema())}),
        &["sample_count", "missing_count", "value"],
    );
    let pair = object(
        json!({"sample_count":count, "missing_count":count,
        "switchbite_mean":nullable(number_schema()), "baseline_mean":nullable(number_schema()),
        "delta":nullable(signed), "case_ids":{"type":"array", "maxItems":MAX_CASES,
            "uniqueItems":true, "items":id_schema()}}),
        &[
            "sample_count",
            "missing_count",
            "switchbite_mean",
            "baseline_mean",
            "delta",
            "case_ids",
        ],
    );
    let mut metrics = json!({});
    let mut pairs = json!({});
    let mut values = json!({});
    for key in METRICS {
        metrics[key] = metric.clone();
        pairs[key] = pair.clone();
        values[key] = nullable(number_schema());
    }
    let measurement = object(
        json!({"candidate_id":nullable(id_schema()),
        "outcome":nullable(json!({"type":"string", "enum":["passed","failed"]})),
        "metrics":object(values, &METRICS)}),
        &["candidate_id", "outcome", "metrics"],
    );
    let strategy = object(
        json!({"selected_count":count, "missing_selection_count":count,
        "metrics":object(metrics, &METRICS)}),
        &["selected_count", "missing_selection_count", "metrics"],
    );
    let strategies = object(
        json!({"switchbite":strategy, "manual":strategy, "strongest":strategy}),
        &STRATEGIES,
    );
    let measurements = object(
        json!({"switchbite":measurement, "manual":measurement,
        "strongest":measurement}),
        &STRATEGIES,
    );
    let agreement = object(
        json!({"sample_count":count, "missing_count":count,
        "matched_count":count, "value":rate}),
        &["sample_count", "missing_count", "matched_count", "value"],
    );
    let hold = object(
        json!({"expected_count":count, "observed_count":count,
        "correctly_held_count":count, "missing_count":count, "value":rate}),
        &[
            "expected_count",
            "observed_count",
            "correctly_held_count",
            "missing_count",
            "value",
        ],
    );
    let row = object(
        json!({"case":input_schema()["properties"]["cases"]["items"],
        "decision":nullable(input_schema()["properties"]["decisions"]["items"].clone()),
        "route_agrees":nullable(json!({"type":"boolean"})), "measurements":measurements}),
        &["case", "decision", "route_agrees", "measurements"],
    );
    object(
        json!({
            "schema_version":{"const":1}, "kind":{"enum":["synthetic","observed"]},
            "proposal_only":{"const":true},
            "evidence":{"enum":["synthetic_fixture","imported_unverified"]},
            "case_count":count,
            "routing":object(json!({"agreement":agreement, "hold_coverage":hold,
                "unexpected_hold_count":count}), &["agreement","hold_coverage","unexpected_hold_count"]),
            "strategies":strategies,
            "paired_comparisons":object(json!({"manual":object(pairs.clone(), &METRICS),
                "strongest":object(pairs, &METRICS)}), &["manual","strongest"]),
            "cases":{"type":"array", "minItems":1, "maxItems":MAX_CASES, "items":row}
        }),
        &[
            "schema_version",
            "kind",
            "proposal_only",
            "evidence",
            "case_count",
            "routing",
            "strategies",
            "paired_comparisons",
            "cases",
        ],
    )
}

fn id(value: &Value) -> &str {
    value.as_str().expect("schema-validated identifier")
}

fn ratio(numerator: usize, denominator: usize) -> Option<f64> {
    (denominator != 0).then(|| numerator as f64 / denominator as f64)
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn measurement(
    candidate: Option<&str>,
    observation: Option<&&Value>,
    decision: Option<&Value>,
) -> Value {
    let mut metrics = json!({});
    metrics["success"] = match observation.map(|o| id(&o["outcome"])) {
        Some("passed") => json!(1.0),
        Some("failed") => json!(0.0),
        _ => Value::Null,
    };
    for key in &METRICS[1..] {
        metrics[*key] = decision
            .and_then(|d| d.get(*key))
            .or_else(|| observation.and_then(|o| o.get(*key)))
            .cloned()
            .unwrap_or(Value::Null);
    }
    json!({"candidate_id":candidate, "outcome":observation.map(|o| &o["outcome"]), "metrics":metrics})
}

/// Evaluate one bounded import. All errors use the existing InvalidRequest contract.
/// Inputs and live policy remain unchanged. Missing evidence always stays missing.
pub fn evaluate(request: &Value) -> Result<Value, ContractError> {
    let bad = ContractError::InvalidRequest;
    contracts::bounded(request, MAX_JSON_BYTES)?;
    contracts::validate(request, &input_schema())?;
    let mut cases = BTreeMap::new();
    for case in request["cases"].as_array().ok_or(bad)? {
        if cases.insert(id(&case["id"]), case).is_some()
            || case["acceptable_candidate_ids"]
                .as_array()
                .ok_or(bad)?
                .is_empty()
                != (case["expected_hold"] == true)
        {
            return Err(bad);
        }
    }
    let mut decisions = BTreeMap::new();
    for decision in request["decisions"].as_array().ok_or(bad)? {
        let case_id = id(&decision["case_id"]);
        let ready = decision["status"] == "ready";
        if !cases.contains_key(case_id)
            || decisions.insert(case_id, decision).is_some()
            || ready != decision.get("candidate_id").is_some()
            || (!ready && METRICS[1..].iter().any(|m| decision.get(*m).is_some()))
        {
            return Err(bad);
        }
    }
    let mut observations = BTreeMap::new();
    for observation in request["observations"].as_array().ok_or(bad)? {
        let case_id = id(&observation["case_id"]);
        let candidate = id(&observation["candidate_id"]);
        let case = cases.get(case_id).ok_or(bad)?;
        let decision = decisions.get(case_id);
        let declared = case["acceptable_candidate_ids"]
            .as_array()
            .ok_or(bad)?
            .iter()
            .chain(case["baseline_candidates"].as_object().ok_or(bad)?.values())
            .any(|v| v == candidate)
            || decision.is_some_and(|d| d.get("candidate_id").is_some_and(|v| v == candidate));
        if !declared
            || observations
                .insert((case_id, candidate), observation)
                .is_some()
        {
            return Err(bad);
        }
        // These fields describe the same selected run. Never silently pick a conflicting source.
        if let Some(d) = decision.filter(|d| d["candidate_id"] == candidate) {
            for metric in &METRICS[1..] {
                if let (Some(a), Some(b)) = (d.get(*metric), observation.get(*metric))
                    && a.as_f64() != b.as_f64()
                {
                    return Err(bad);
                }
            }
        }
    }
    let mut rows = Vec::new();
    let mut matched = 0;
    let mut expected_holds = 0;
    let mut observed_holds = 0;
    let mut correct_holds = 0;
    let mut unexpected_holds = 0;
    for (case_id, case) in &cases {
        let decision = decisions.get(case_id).copied();
        let expected_hold = case["expected_hold"] == true;
        let agrees = decision.map(|d| {
            if expected_hold {
                d["status"] == "hold"
            } else {
                d["status"] == "ready"
                    && case["acceptable_candidate_ids"]
                        .as_array()
                        .expect("validated candidates")
                        .contains(&d["candidate_id"])
            }
        });
        matched += usize::from(agrees == Some(true));
        expected_holds += usize::from(expected_hold);
        observed_holds += usize::from(expected_hold && decision.is_some());
        correct_holds += usize::from(expected_hold && agrees == Some(true));
        unexpected_holds +=
            usize::from(!expected_hold && decision.is_some_and(|d| d["status"] == "hold"));
        let mut measurements = json!({});
        for strategy in STRATEGIES {
            let candidate = if strategy == "switchbite" {
                decision.and_then(|d| d.get("candidate_id")).map(id)
            } else {
                case["baseline_candidates"].get(strategy).map(id)
            };
            let observation = candidate.and_then(|c| observations.get(&(*case_id, c)));
            measurements[strategy] = measurement(
                candidate,
                observation,
                if strategy == "switchbite" {
                    decision
                } else {
                    None
                },
            );
        }
        rows.push(
            json!({"case":case, "decision":decision, "route_agrees":agrees,
            "measurements":measurements}),
        );
    }
    let total = cases.len();
    let mut strategies = json!({});
    for strategy in STRATEGIES {
        let selected = rows
            .iter()
            .filter(|r| r["measurements"][strategy]["candidate_id"].is_string())
            .count();
        let mut metrics = json!({});
        for metric in METRICS {
            let values: Vec<_> = rows
                .iter()
                .filter_map(|r| r["measurements"][strategy]["metrics"][metric].as_f64())
                .collect();
            metrics[metric] = json!({"sample_count":values.len(), "missing_count":total-values.len(),
                "value":mean(&values)});
        }
        strategies[strategy] = json!({"selected_count":selected,
            "missing_selection_count":total-selected, "metrics":metrics});
    }
    let mut pairs = json!({});
    for baseline in &STRATEGIES[1..] {
        let mut metrics = json!({});
        for metric in METRICS {
            let mut switchbite = Vec::new();
            let mut other = Vec::new();
            let mut case_ids = BTreeSet::new();
            for row in &rows {
                if let (Some(a), Some(b)) = (
                    row["measurements"]["switchbite"]["metrics"][metric].as_f64(),
                    row["measurements"][*baseline]["metrics"][metric].as_f64(),
                ) {
                    switchbite.push(a);
                    other.push(b);
                    case_ids.insert(id(&row["case"]["id"]));
                }
            }
            let deltas: Vec<_> = switchbite.iter().zip(&other).map(|(a, b)| a - b).collect();
            metrics[metric] = json!({"sample_count":switchbite.len(),
                "missing_count":total-switchbite.len(), "switchbite_mean":mean(&switchbite),
                "baseline_mean":mean(&other), "delta":mean(&deltas), "case_ids":case_ids});
        }
        pairs[*baseline] = metrics;
    }
    let result = json!({"schema_version":1, "kind":request["kind"], "proposal_only":true,
        "evidence":if request["kind"] == "synthetic" {"synthetic_fixture"} else {"imported_unverified"},
        "case_count":total,
        "routing":{"agreement":{"sample_count":decisions.len(), "missing_count":total-decisions.len(),
            "matched_count":matched, "value":ratio(matched, decisions.len())},
            "hold_coverage":{"expected_count":expected_holds, "observed_count":observed_holds,
                "correctly_held_count":correct_holds, "missing_count":expected_holds-observed_holds,
                "value":ratio(correct_holds, expected_holds)}, "unexpected_hold_count":unexpected_holds},
        "strategies":strategies, "paired_comparisons":pairs, "cases":rows});
    contracts::bounded(&result, 262_144)?;
    contracts::validate(&result, &output_schema())?;
    Ok(result)
}
