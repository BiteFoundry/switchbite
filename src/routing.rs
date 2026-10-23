//! Pure policy-aware proposal routing. Hosts own rechecking and dispatch.
//! Replay hashes bind normalized inputs, not provenance or provider authenticity.
use crate::{
    Judge, Plugin,
    contracts::{self, ContractError, MAX_JSON_BYTES},
    core,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::LazyLock};

static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../schemas/route-v2.json")).expect("embedded route schema")
});

/// Advanced native input, including optional bound typed replay.
pub fn input_schema() -> Value {
    SCHEMA["$defs"]["input"].clone()
}

/// Agents supply task and observed pool facts; the host handles replay binding.
pub fn agent_input_schema() -> Value {
    let mut schema = input_schema();
    schema["properties"]
        .as_object_mut()
        .unwrap()
        .remove("replay");
    schema["properties"]["policy"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("judgment_validity_ms");
    schema
}

pub fn output_schema() -> Value {
    SCHEMA["$defs"]["output"].clone()
}

/// Profiles set thresholds and weights only, never worker capability facts.
pub fn default_policy(profile: &str) -> Result<Value, ContractError> {
    let (fit, preference, min_fit, context, confidence) = match profile {
        "balanced" => (0.7, 0.3, 0.5, 0.5, 0.3),
        "quick" => (0.6, 0.4, 0.5, 0.5, 0.3),
        "implementation" => (0.85, 0.15, 0.66, 0.5, 0.4),
        "review" => (0.9, 0.1, 0.66, 0.6, 0.5),
        "mockup" => (0.6, 0.4, 0.5, 0.5, 0.3),
        _ => return Err(ContractError::InvalidRequest),
    };
    Ok(
        json!({"id":profile,"profile":profile,"fit_weight":fit,"preference_weight":preference,
        "min_fit":min_fit,"min_context":context,"min_confidence":confidence,
        "tie_margin":0.02,"tie_behavior":"hold","switch_margin":0.1,"switch_penalty":0.05,
        "excluded_candidate_ids":[],"judgment_validity_ms":60000}),
    )
}

fn sort_strings(value: &mut Value) {
    value
        .as_array_mut()
        .unwrap()
        .sort_by(|a, b| a.as_str().cmp(&b.as_str()));
}

/// Canonical semantic input, excluding replay. Missing optional empty fields
/// are equivalent to explicit defaults. Unknown host observations stay absent.
pub fn normalize(request: &Value) -> Result<Value, ContractError> {
    contracts::bounded(request, MAX_JSON_BYTES)?;
    contracts::validate(request, &input_schema())?;
    let mut normalized = request.clone();
    let root = normalized.as_object_mut().unwrap();
    root.remove("replay");
    root.insert("version".into(), json!(2));
    root.entry("preferences").or_insert(json!(""));
    if root["preferences"].as_str().unwrap().trim().is_empty() {
        root.insert("preferences".into(), json!(""));
    }
    let task = root.get_mut("task").unwrap().as_object_mut().unwrap();
    for field in ["requirements", "context_refs"] {
        sort_strings(task.entry(field).or_insert(json!([])));
    }
    let pool = root.get_mut("pool").unwrap().as_array_mut().unwrap();
    let mut ids = BTreeSet::new();
    for candidate in pool.iter_mut() {
        if !ids.insert(candidate["id"].as_str().unwrap().to_owned()) {
            return Err(ContractError::InvalidRequest);
        }
        sort_strings(&mut candidate["capabilities"]);
    }
    pool.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let supplied = root.get("policy").cloned().unwrap_or(json!({}));
    let profile = supplied["profile"].as_str().unwrap_or("balanced");
    let mut policy = default_policy(profile)?;
    for (key, value) in supplied.as_object().unwrap() {
        policy[key] = value.clone();
    }
    let total = n(&policy, "fit_weight") + n(&policy, "preference_weight");
    if total <= 0.0 {
        return Err(ContractError::InvalidRequest);
    }
    let fit_weight = n(&policy, "fit_weight") / total;
    policy["fit_weight"] = json!(fit_weight);
    // Complementary weights make repeated normalization stable in f64.
    policy["preference_weight"] = json!(1.0 - fit_weight);
    sort_strings(&mut policy["excluded_candidate_ids"]);
    root.insert("policy".into(), policy);
    let continuity = root
        .entry("continuity")
        .or_insert(json!({}))
        .as_object_mut()
        .unwrap();
    continuity.entry("in_progress").or_insert(json!(false));
    contracts::bounded(&normalized, MAX_JSON_BYTES)?;
    Ok(normalized)
}

fn n(value: &Value, field: &str) -> f64 {
    value[field].as_f64().unwrap()
}

fn binding(normalized: &Value, now_ms: u64) -> Result<Value, ContractError> {
    let expiry = now_ms
        .checked_add(
            normalized["policy"]["judgment_validity_ms"]
                .as_u64()
                .unwrap(),
        )
        .filter(|expiry| *expiry <= i64::MAX as u64)
        .ok_or(ContractError::InvalidRequest)?;
    Ok(
        json!({"input_sha256":core::sha256(&serde_json::to_vec(normalized).map_err(|_| ContractError::InvalidRequest)?),
        "expires_at_ms":expiry}),
    )
}

/// Compute a host replay binding without a judge, environment access or I/O.
pub fn snapshot(request: &Value, now_ms: u64) -> Result<Value, ContractError> {
    binding(&normalize(request)?, now_ms)
}

fn held(snapshot: &Value, excluded: &[Value], reason: &str, action: &str, summary: &str) -> Value {
    json!({"version":2,"status":"hold","candidate_id":null,"next_action":action,"summary":summary,
        "reason_codes":[reason],"selected":null,"alternatives":[],"excluded":excluded,"snapshot":snapshot})
}

fn finish(value: Value) -> Result<Value, ContractError> {
    contracts::bounded(&value, MAX_JSON_BYTES)?;
    contracts::validate(&value, &output_schema())?;
    Ok(value)
}

/// Apply hard observed-fact gates before requesting any judgment.
fn eligible(normalized: &Value) -> (Vec<Value>, Vec<Value>) {
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    let policy = &normalized["policy"];
    for candidate in normalized["pool"].as_array().unwrap() {
        let mut reasons = Vec::new();
        if candidate["available"] != true {
            reasons.push("unavailable");
        }
        if !normalized["task"]["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| candidate["capabilities"].as_array().unwrap().contains(r))
        {
            reasons.push("required_capability_absent");
        }
        if policy["excluded_candidate_ids"]
            .as_array()
            .unwrap()
            .contains(&candidate["id"])
        {
            reasons.push("explicit_exclusion");
        }
        // Explicit producer identity excludes self-review even in custom profiles.
        if policy.get("producer_candidate_id") == candidate.get("id") {
            reasons.push("producer_excluded");
        }
        for (limit, estimate, unknown, exceeded) in [
            ("max_cost", "estimated_cost", "cost_unknown", "cost_limit"),
            (
                "max_latency_ms",
                "estimated_latency_ms",
                "latency_unknown",
                "latency_limit",
            ),
        ] {
            if let Some(max) = policy[limit].as_f64() {
                match candidate[estimate].as_f64() {
                    None => reasons.push(unknown),
                    Some(value) if value > max => reasons.push(exceeded),
                    _ => {}
                }
            }
        }
        if reasons.is_empty() {
            included.push(candidate.clone());
        } else {
            excluded.push(json!({"candidate_id":candidate["id"],"reasons":reasons}));
        }
    }
    (included, excluded)
}

fn legacy_request(normalized: &Value, candidates: &[Value]) -> Value {
    let mut task = normalized["task"].clone();
    task["extensions"] = json!({});
    let candidates: Vec<Value> = candidates.iter().map(|candidate| json!({
        "id":candidate["id"],"description":candidate["description"],
        "capabilities":candidate["capabilities"],"available":candidate["available"],"extensions":{}
    })).collect();
    json!({"version":1,"task":task,"candidates":candidates,"preferences":normalized["preferences"]})
}

fn scored(
    candidate: &Value,
    scores: &Value,
    context: f64,
    policy: &Value,
    current: &Value,
) -> Value {
    let fit = n(scores, "fit");
    let preference = n(scores, "preference");
    let fit_weight = n(policy, "fit_weight");
    let preference_weight = n(policy, "preference_weight");
    // Fit remains a hard eligibility axis. A zero preference weight does not
    // lower concentration for an axis that does not affect ranking.
    let confidence = if preference_weight > 0.0 {
        n(scores, "fit_confidence").min(n(scores, "preference_confidence"))
    } else {
        n(scores, "fit_confidence")
    };
    let penalty = if !current.is_null() && current != &candidate["id"] {
        n(policy, "switch_penalty")
    } else {
        0.0
    };
    json!({"candidate_id":candidate["id"],"policy_id":policy["id"],"reason_codes":["policy_scored"],
        "breakdown":{"fit":fit,"preference":preference,"context":context,
        "fit_confidence":scores["fit_confidence"],"preference_confidence":scores["preference_confidence"],
        "confidence":confidence,"confidence_kind":"typesafe_score_concentration",
        "fit_weight":fit_weight,"preference_weight":preference_weight,
        "weighted_fit":fit_weight*fit,"weighted_preference":preference_weight*preference,
        "switch_penalty":penalty,"composite":fit_weight*fit+preference_weight*preference-penalty}})
}

/// Return a proposal only. Never migrates running work or mutates the request/host.
pub fn route(
    request: &Value,
    judge: Option<&dyn Judge>,
    now_ms: u64,
) -> Result<Value, ContractError> {
    let normalized = normalize(request)?;
    let mut snapshot = binding(&normalized, now_ms)?;
    let (candidates, mut excluded) = eligible(&normalized);
    if let Some(replay) = request.get("replay") {
        let digest = replay["input_sha256"].as_str().unwrap();
        let valid_digest = digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let reason = if !valid_digest || replay["input_sha256"] != snapshot["input_sha256"] {
            Some("replay_mismatch")
        } else if replay["expires_at_ms"].as_u64().unwrap() <= now_ms {
            Some("replay_expired")
        } else if replay["expires_at_ms"].as_u64().unwrap()
            > snapshot["expires_at_ms"].as_u64().unwrap()
        {
            Some("replay_future")
        } else {
            None
        };
        if let Some(reason) = reason {
            return finish(held(
                &snapshot,
                &excluded,
                reason,
                "refresh_judgment",
                "Refresh the judgment for this task and pool.",
            ));
        }
        // Replaying never extends the original validity window.
        snapshot["expires_at_ms"] = replay["expires_at_ms"].clone();
    }
    if normalized["continuity"]["in_progress"] == true {
        return finish(held(
            &snapshot,
            &excluded,
            "work_in_progress",
            "wait_for_completion",
            "Wait for the current work to finish before routing again.",
        ));
    }
    if candidates.is_empty() {
        return finish(held(
            &snapshot,
            &excluded,
            "no_eligible_candidates",
            "refresh_pool",
            "Provide an available candidate satisfying the task and policy limits.",
        ));
    }
    if request.get("replay").is_none() && judge.is_none() {
        return finish(held(
            &snapshot,
            &excluded,
            "judge_unavailable",
            "configure_judge",
            "Configure a judge to compare the eligible candidates.",
        ));
    }
    let legacy = legacy_request(&normalized, &candidates);
    let judgment = if let Some(replay) = request.get("replay") {
        replay["judgment"].clone()
    } else {
        judge
            .unwrap()
            .judge(&core::judgment_body(&legacy)?)
            .map_err(|_| ContractError::JudgeFailed)?
    };
    // Plugin performs typed Jev validation. API1's fixed HOLD thresholds are
    // intentionally ignored; complete validated scores feed API2 policy.
    // Keep request and response bounds independent and classify provider data
    // through the typed-judgment validator, not caller-input validation.
    let validated =
        Plugin::with_judge(move |_: &Value| Ok(judgment.clone())).invoke("decide", &legacy)?;
    let context = n(&validated["scores"], "context_sufficient");
    let policy = &normalized["policy"];
    if context < n(policy, "min_context") {
        return finish(held(
            &snapshot,
            &excluded,
            "context_insufficient",
            "clarify_task",
            "Clarify the objective or supply the missing context.",
        ));
    }
    let current = &normalized["continuity"]["current_candidate_id"];
    let current_eligible = candidates
        .iter()
        .any(|c| &c["id"] == current && c["has_context"] != false);
    let no_current = Value::Null;
    let active_current = if current_eligible {
        current
    } else {
        &no_current
    };
    let mut ranked = Vec::new();
    let mut low_confidence = false;
    for candidate in &candidates {
        let scores = &validated["scores"]["candidates"][candidate["id"].as_str().unwrap()];
        let entry = scored(candidate, scores, context, policy, active_current);
        let mut reasons = Vec::new();
        if n(scores, "fit") < n(policy, "min_fit") {
            reasons.push("below_min_fit");
        }
        if n(&entry["breakdown"], "confidence") < n(policy, "min_confidence") {
            low_confidence = true;
            reasons.push("low_confidence");
        }
        if reasons.is_empty() {
            ranked.push(entry);
        } else {
            excluded.push(json!({"candidate_id":candidate["id"],"reasons":reasons}));
        }
    }
    excluded.sort_by(|a, b| a["candidate_id"].as_str().cmp(&b["candidate_id"].as_str()));
    ranked.sort_by(|a, b| {
        n(&b["breakdown"], "composite")
            .total_cmp(&n(&a["breakdown"], "composite"))
            .then_with(|| a["candidate_id"].as_str().cmp(&b["candidate_id"].as_str()))
    });
    if ranked.is_empty() {
        return finish(held(
            &snapshot,
            &excluded,
            if low_confidence {
                "low_confidence"
            } else {
                "no_fit"
            },
            if low_confidence {
                "request_review"
            } else {
                "refresh_pool"
            },
            "No candidate meets the policy thresholds.",
        ));
    }
    let top = n(&ranked[0]["breakdown"], "composite");
    let retained = ranked
        .iter()
        .position(|entry| current_eligible && &entry["candidate_id"] == current)
        .filter(|i| top - n(&ranked[*i]["breakdown"], "composite") <= n(policy, "switch_margin"));
    let mut chosen = 0;
    let mut reason = "highest_policy_score";
    if let Some(i) = retained {
        chosen = i;
        reason = "continuity_retained";
    } else {
        let ties: Vec<usize> = ranked
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                top - n(&entry["breakdown"], "composite") <= n(policy, "tie_margin")
            })
            .map(|(i, _)| i)
            .collect();
        if ties.len() > 1 {
            let behavior = policy["tie_behavior"].as_str().unwrap();
            if behavior == "hold" {
                let mut result = held(
                    &snapshot,
                    &excluded,
                    "tie",
                    "request_review",
                    "Review the tied candidates or choose an explicit tie policy.",
                );
                result["alternatives"] = json!(ranked.into_iter().take(3).collect::<Vec<_>>());
                return finish(result);
            }
            let estimate = match behavior {
                "lowest_cost" => Some("estimated_cost"),
                "lowest_latency" => Some("estimated_latency_ms"),
                _ => None,
            };
            if let Some(field) = estimate {
                let observed = |i: usize| {
                    candidates
                        .iter()
                        .find(|c| c["id"] == ranked[i]["candidate_id"])
                        .unwrap()[field]
                        .as_f64()
                };
                if ties.iter().any(|i| observed(*i).is_none()) {
                    let mut result = held(
                        &snapshot,
                        &excluded,
                        "tie_estimate_unknown",
                        "refresh_pool",
                        "Supply the observed estimates required by the tie policy.",
                    );
                    result["alternatives"] = json!(ranked.into_iter().take(3).collect::<Vec<_>>());
                    return finish(result);
                }
                chosen = *ties
                    .iter()
                    .min_by(|a, b| {
                        observed(**a)
                            .unwrap()
                            .total_cmp(&observed(**b).unwrap())
                            .then_with(|| {
                                ranked[**a]["candidate_id"]
                                    .as_str()
                                    .cmp(&ranked[**b]["candidate_id"].as_str())
                            })
                    })
                    .unwrap();
            } else {
                chosen = *ties
                    .iter()
                    .min_by_key(|i| ranked[**i]["candidate_id"].as_str().unwrap())
                    .unwrap();
            }
            reason = match behavior {
                "lowest_cost" => "tie_resolved_lowest_cost",
                "lowest_latency" => "tie_resolved_lowest_latency",
                _ => "tie_resolved_stable_id",
            };
        }
    }
    let mut selected = ranked.remove(chosen);
    selected["reason_codes"] = json!([reason]);
    let summary = match reason {
        "continuity_retained" => {
            "Keep the current target: the alternative's improvement is below the switch margin."
        }
        "tie_resolved_lowest_cost" => {
            "Policy scores are tied; prefer the lowest observed cost estimate under your tie policy."
        }
        "tie_resolved_lowest_latency" => {
            "Policy scores are tied; prefer the lowest observed latency estimate under your tie policy."
        }
        "tie_resolved_stable_id" => {
            "Policy scores are tied; your explicit stable-ID policy determines the proposed target."
        }
        _ => {
            "Recommend the highest-scoring eligible target under this policy. Recheck its current availability before dispatch."
        }
    };
    finish(
        json!({"version":2,"status":"ready","candidate_id":selected["candidate_id"],
        "next_action":"recheck_and_dispatch","summary":summary,
        "reason_codes":[reason],"selected":selected,"alternatives":ranked.into_iter().take(3).collect::<Vec<_>>(),
        "excluded":excluded,"snapshot":snapshot}),
    )
}
