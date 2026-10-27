//! Deterministic workflow facts projected into the existing routing API.
use serde_json::{Value, json};

pub const DEFAULT_PREFERENCES: &str = "Prefer Codex for orchestration and independent review; Claude for implementation; Antigravity for mockups and writing; Cursor and Grok as balanced alternatives. Choose OpenCode targets from their actual model capabilities. These are preferences among eligible targets; explicit task routing pins take priority.";

pub fn preferences(request: &Value) -> &str {
    request["preferences"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_PREFERENCES)
}

fn clipped(s: &str, bytes: usize) -> String {
    let mut end = s.len().min(bytes);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// Acceptance and output contracts survive projection; only optional context
/// summaries are bounded. An oversized contract needs a smaller task.
pub fn task(
    node: &Value,
    summary: Option<&str>,
    dependencies: &[Value],
) -> Result<Value, &'static str> {
    let mut details = serde_json::Map::new();
    for key in ["role", "acceptance", "outputs", "write_paths", "routing"] {
        if let Some(v) = node.get(key) {
            details.insert(key.into(), v.clone());
        }
    }
    let objective = node["objective"].as_str().ok_or("invalid_task")?;
    let mut body = format!("{objective}\nTask contract: {}", Value::Object(details));
    if body.len() > 24000 {
        return Err("routing_contract_too_large");
    }
    if let Some(s) = summary.filter(|s| !s.trim().is_empty()) {
        let remaining = 24000usize.saturating_sub(body.len() + 19);
        if remaining > 0 {
            body.push_str("\nGathered context: ");
            body.push_str(&clipped(s, remaining.min(2000)));
        }
    }
    for d in dependencies {
        let text = format!("\nIntegrated dependency: {d}");
        if body.len() + text.len() <= 24000 {
            body.push_str(&text);
        }
    }
    Ok(json!({"id":node["id"],"objective":body,
        "requirements":node.get("requirements").cloned().unwrap_or(json!([])),
        "context_refs":node.get("context_refs").cloned().unwrap_or(json!([]))}))
}

pub fn candidate(candidate: &Value) -> Value {
    let mut projected = candidate.clone();
    projected.as_object_mut().unwrap().retain(|k, _| {
        [
            "id",
            "description",
            "capabilities",
            "available",
            "estimated_cost",
            "estimated_latency_ms",
            "has_context",
        ]
        .contains(&k.as_str())
    });
    let mut identity = serde_json::Map::new();
    for key in ["provider", "model", "effort"] {
        if let Some(v) = candidate.get(key) {
            identity.insert(key.into(), v.clone());
        }
    }
    if !identity.is_empty() {
        let facts = format!("\nExecution profile: {}", Value::Object(identity));
        let description = candidate["description"].as_str().unwrap_or("Worker");
        projected["description"] = json!(format!(
            "{}{}",
            clipped(description, 2000usize.saturating_sub(facts.len())),
            facts
        ));
    }
    projected
}

pub fn matches_pin(candidate: &Value, node: &Value) -> bool {
    let Some(pin) = node.get("routing") else {
        return true;
    };
    ["candidate_id", "provider", "model", "effort"]
        .iter()
        .all(|key| {
            pin.get(key).is_none_or(|expected| {
                candidate[if *key == "candidate_id" { "id" } else { key }] == *expected
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_and_model_identity_reach_existing_router() {
        let node = json!({"id":"a","objective":"Build","acceptance":["JSON parses"],"outputs":["result.json"],"write_paths":["result.json"]});
        let t = task(
            &node,
            Some("Existing API"),
            &[json!({"id":"before","commit":"123"})],
        )
        .unwrap();
        let objective = t["objective"].as_str().unwrap();
        for value in ["JSON parses", "result.json", "Existing API", "before"] {
            assert!(objective.contains(value));
        }
        let c = candidate(
            &json!({"id":"x","description":"worker","capabilities":[],"available":true,"provider":"codex","model":"chosen-model","effort":"high","executable":"private-path"}),
        );
        assert!(c["description"].as_str().unwrap().contains("chosen-model"));
        assert!(c["description"].as_str().unwrap().contains("high"));
        assert!(!c.to_string().contains("private-path"));
    }

    #[test]
    fn large_required_contract_is_not_silently_removed() {
        let node = json!({"id":"x","objective":"Build","acceptance":["검사".repeat(5000)]});
        assert_eq!(
            task(&node, None, &[]).unwrap_err(),
            "routing_contract_too_large"
        );
        let node = json!({"id":"x","objective":"Build","acceptance":["Exact requirement"]});
        let a = task(&node, Some(&"가".repeat(30000)), &[]).unwrap();
        assert!(
            a["objective"]
                .as_str()
                .unwrap()
                .contains("Exact requirement")
        );
        assert!(a["objective"].as_str().unwrap().len() <= 24000);
        assert_eq!(a, task(&node, Some(&"가".repeat(30000)), &[]).unwrap());
    }

    #[test]
    fn explicit_target_fields_are_conjunctive() {
        let c = json!({"id":"x","provider":"codex","model":"m","effort":"high"});
        assert!(matches_pin(
            &c,
            &json!({"routing":{"provider":"codex","effort":"high"}})
        ));
        assert!(!matches_pin(&c, &json!({"routing":{"model":"other"}})));
    }
}
