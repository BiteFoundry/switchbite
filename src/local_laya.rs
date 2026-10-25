//! Explicit loopback-only Laya transport. Never reads a cloud credential or
//! falls back to Jev. The local server owns model loading and residency.
use crate::{
    Judge,
    contracts::{self, ContractError, MAX_JSON_BYTES, MAX_JUDGE_BYTES},
    runtime::RuntimeError,
};
use serde_json::{Value, json};
use std::{io::Read, net::IpAddr, sync::OnceLock, time::Duration};

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:11435/v1/systemone";
pub const DEFAULT_MODEL: &str = "laya:multilingual";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayaConfig {
    endpoint: String,
    model: String,
}
impl LayaConfig {
    pub fn new(endpoint: &str, model: &str) -> Result<Self, RuntimeError> {
        let mut url = reqwest::Url::parse(endpoint).map_err(|_| bad_config())?;
        let host = url.host_str().ok_or_else(bad_config)?;
        let ip = host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .map_err(|_| bad_config())?;
        if url.scheme() != "http"
            || !ip.is_loopback()
            || url.port_or_known_default() == Some(0)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/" | "/v1/systemone")
        {
            return Err(bad_config());
        }
        if matches!(url.path(), "" | "/") {
            url.set_path("/v1/systemone");
        }
        contracts::validate_model(model).map_err(|_| bad_config())?;
        if !(model
            .strip_prefix("laya:")
            .is_some_and(|tag| !tag.is_empty())
            || matches!(
                model,
                "convaiinnovations/laya"
                    | "convaiinnovations/laya-multilingual"
                    | "convaiinnovations/laya-typed-decisions"
            ))
            || model.ends_with(":latest")
        {
            return Err(bad_config());
        }
        Ok(Self {
            endpoint: url.to_string(),
            model: model.into(),
        })
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn model(&self) -> &str {
        &self.model
    }
}
fn bad_config() -> RuntimeError {
    RuntimeError::from_code("local_judge_config")
}

pub struct LocalLayaJudge {
    config: LayaConfig,
    client: OnceLock<Result<reqwest::blocking::Client, RuntimeError>>,
}
impl LocalLayaJudge {
    pub fn new(config: LayaConfig) -> Self {
        Self {
            config,
            client: OnceLock::new(),
        }
    }
    pub fn config(&self) -> &LayaConfig {
        &self.config
    }
    pub fn model(&self) -> &str {
        self.config.model()
    }
    pub fn judge_detailed(&self, body: &Value) -> Result<Value, RuntimeError> {
        if body["model"] != self.model()
            || !body["state"].is_object()
            || !body["questions"].is_object()
        {
            return Err(RuntimeError::from_code("invalid_request"));
        }
        contracts::bounded(body, MAX_JUDGE_BYTES)
            .map_err(|_| RuntimeError::from_code("invalid_request"))?;
        let (wire_body, neutral) = prepare_request(body);
        let client = self
            .client
            .get_or_init(|| {
                reqwest::blocking::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .connect_timeout(Duration::from_secs(2))
                    .timeout(Duration::from_secs(30))
                    .build()
                    .map_err(|_| RuntimeError::from_code("local_judge_unavailable"))
            })
            .as_ref()
            .map_err(|e| *e)?;
        // No Authorization, environment keys, cookies, or ambient proxy grants.
        let response = client
            .post(self.config.endpoint())
            .header("Content-Type", "application/json")
            .body(
                serde_json::to_vec(&wire_body)
                    .map_err(|_| RuntimeError::from_code("invalid_request"))?,
            )
            .send()
            .map_err(|e| {
                RuntimeError::from_code(if e.is_timeout() {
                    "provider_timeout"
                } else {
                    "local_judge_unavailable"
                })
            })?;
        let status = response.status().as_u16();
        let length = response.content_length();
        if status == 404 {
            return Err(RuntimeError::from_code("local_model_unavailable"));
        }
        if status == 422 {
            let mut bytes = Vec::new();
            let _ = response.take(4097).read_to_end(&mut bytes);
            let code = contracts::parse_json(&bytes, 4096).ok();
            return Err(RuntimeError::from_code(
                if code.as_ref().is_some_and(|v| {
                    matches!(
                        v["code"].as_str(),
                        Some("STATE_TRUNCATED" | "INPUT_TOO_LONG" | "TOO_MANY_OPTIONS")
                    )
                }) {
                    "judge_context_too_long"
                } else {
                    "invalid_request"
                },
            ));
        }
        if !(200..300).contains(&status) {
            return Err(RuntimeError::from_code(match status {
                401 | 403 => "provider_auth_failed",
                429 | 503 => "provider_rate_limited",
                408 | 504 => "provider_timeout",
                500..=599 => "local_judge_unavailable",
                _ => "provider_invalid_response",
            }));
        }
        if length.is_some_and(|n| n > MAX_JSON_BYTES as u64) {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_JSON_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        let mut raw = contracts::parse_json(&bytes, MAX_JSON_BYTES).map_err(|_| invalid())?;
        if !neutral.is_empty() {
            let answers = raw
                .get_mut("answers")
                .and_then(Value::as_object_mut)
                .ok_or_else(invalid)?;
            for (key, value) in neutral {
                if answers.insert(key, value).is_some() {
                    return Err(invalid());
                }
            }
        }
        normalize_response(raw, self.model())
    }
    /// Reports configuration, then performs a small synthetic decision only.
    /// Does not read project content or execute any worker.
    pub fn probe(&self) -> Result<Value, RuntimeError> {
        let body = json!({"model":self.model(), "state":{"task":"Write a unit test for an existing Rust function."},
            "questions":{"implementation":{"type":"choice","instructions":"What work does task request?",
                "criteria":{"A":"Implement or test software","B":"Write promotional copy"}}}});
        let began = std::time::Instant::now();
        let result = self.judge_detailed(&body)?;
        let answer = &result["answers"]["implementation"];
        if answer["type"] != "choice" || !matches!(answer["choice"].as_str(), Some("A" | "B")) {
            return Err(invalid());
        }
        Ok(
            json!({"provider":"local_laya","model":result["model"],"reachable":true,
            "synthetic_probe":true,"elapsed_ms":began.elapsed().as_millis(),
            "probe_choice":answer["choice"],"quality_qualified":false,"cloud_fallback":false}),
        )
    }
}
impl Judge for LocalLayaJudge {
    fn model(&self) -> &str {
        self.model()
    }
    fn judge(&self, body: &Value) -> Result<Value, ContractError> {
        self.judge_detailed(body)
            .map_err(|_| ContractError::JudgeFailed)
    }
}
fn invalid() -> RuntimeError {
    RuntimeError::from_code("provider_invalid_response")
}

/// An absent preference is a neutral policy fact. Do not ask the model to
/// infer a preference that was never stated, or let its uncertainty veto fit.
/// Explicit preferences and every capability/context question remain judged.
fn prepare_request(body: &Value) -> (Value, serde_json::Map<String, Value>) {
    let mut wire = body.clone();
    let mut neutral = serde_json::Map::new();
    if body
        .pointer("/state/user_preferences")
        .and_then(Value::as_str)
        .is_some_and(|s| s.trim().is_empty())
        && let Some(questions) = wire.get_mut("questions").and_then(Value::as_object_mut)
    {
        let ids = questions
            .iter()
            .filter(|(id, q)| {
                id.starts_with("preference_")
                    && q["type"] == "score"
                    && q["criteria"] == json!(crate::core::PREFERENCE_LEVELS)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            questions.remove(&id);
            neutral.insert(id,json!({"type":"score","score":1.0,"confidence":1.0,"probabilities":{"0":0.0,"1":1.0,"2":0.0},"legend":{"0":crate::core::PREFERENCE_LEVELS[0],"1":crate::core::PREFERENCE_LEVELS[1],"2":crate::core::PREFERENCE_LEVELS[2]}}));
        }
    }
    (wire, neutral)
}

/// Canonical distribution concentration is computed locally for the existing
/// policy. It is not a claim that a confidence is an empirical success rate.
fn normalize_response(mut raw: Value, model: &str) -> Result<Value, RuntimeError> {
    if raw.get("state_truncated") == Some(&json!(true))
        || raw.get("truncated") == Some(&json!(true))
        || raw.pointer("/usage/state_truncated") == Some(&json!(true))
        || raw
            .pointer("/usage/state_tokens_dropped")
            .and_then(Value::as_u64)
            .is_some_and(|n| n > 0)
    {
        return Err(RuntimeError::from_code("judge_context_too_long"));
    }
    if raw["model"] != model {
        return Err(invalid());
    }
    let answers = raw
        .get_mut("answers")
        .and_then(Value::as_object_mut)
        .ok_or_else(invalid)?;
    for answer in answers.values_mut() {
        match answer["type"].as_str() {
            Some("score") => {
                let p = distribution(&answer["probabilities"])?;
                if p.len() < 2 || p.len() > 10 {
                    return Err(invalid());
                }
                let values = (0..p.len())
                    .map(|i| p.get(&i.to_string()).copied().ok_or_else(invalid))
                    .collect::<Result<Vec<_>, _>>()?;
                let expected: f64 = values.iter().enumerate().map(|(i, p)| i as f64 * p).sum();
                if answer["score"].as_f64().is_none_or(|s| {
                    !s.is_finite()
                        || s < 0.0
                        || s > (values.len() - 1) as f64
                        || (s - expected).abs() > 0.05 * (values.len() - 1) as f64
                }) {
                    return Err(invalid());
                }
                let mode = (0..values.len())
                    .max_by(|a, b| values[*a].total_cmp(&values[*b]).then_with(|| b.cmp(a)))
                    .ok_or_else(invalid)?;
                let spread: f64 = values
                    .iter()
                    .enumerate()
                    .map(|(i, p)| p * (i as f64 - mode as f64).abs())
                    .sum();
                let even = (0..values.len())
                    .map(|i| (i as f64 - (values.len() - 1) as f64 / 2.0).abs())
                    .sum::<f64>()
                    / values.len() as f64;
                answer["confidence"] = json!((1.0 - spread / even).clamp(0.0, 1.0));
            }
            Some("choice") => {
                let p = distribution(&answer["probabilities"])?;
                if p.len() < 2 {
                    return Err(invalid());
                }
                let selected = answer["choice"].as_str().ok_or_else(invalid)?;
                let selected_probability = *p.get(selected).ok_or_else(invalid)?;
                let top = p.values().copied().fold(0.0, f64::max);
                if top - selected_probability > 0.0002 {
                    return Err(invalid());
                }
                answer["confidence"] =
                    json!(((p.len() as f64 * top - 1.0) / (p.len() - 1) as f64).clamp(0.0, 1.0));
            }
            Some("noul") => {
                unit(&answer["noul"])?;
            }
            _ => return Err(invalid()),
        }
    }
    let mut result = json!({"model":raw["model"],"answers":raw["answers"]});
    if let Some(usage) = raw.get("usage") {
        result["usage"] = usage.clone();
    }
    Ok(result)
}
fn unit(v: &Value) -> Result<f64, RuntimeError> {
    v.as_f64()
        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
        .ok_or_else(invalid)
}
fn distribution(v: &Value) -> Result<std::collections::BTreeMap<String, f64>, RuntimeError> {
    let values = v
        .as_object()
        .ok_or_else(invalid)?
        .iter()
        .map(|(k, v)| Ok((k.clone(), unit(v)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>, RuntimeError>>()?;
    if (values.values().sum::<f64>() - 1.0).abs() > 0.02 {
        return Err(invalid());
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_preferences_are_neutral_policy_facts_but_explicit_preferences_are_judged() {
        let mut body = json!({"state":{"user_preferences":""},"questions":{"preference_worker":{"type":"score","criteria":crate::core::PREFERENCE_LEVELS},"fit_worker":{"type":"score","criteria":crate::core::FIT_LEVELS}}});
        let (wire, neutral) = prepare_request(&body);
        assert!(wire["questions"].get("preference_worker").is_none());
        assert!(wire["questions"].get("fit_worker").is_some());
        assert_eq!(neutral["preference_worker"]["score"], 1.0);
        body["state"]["user_preferences"] = json!("Prefer Claude for implementation");
        let (wire, neutral) = prepare_request(&body);
        assert_eq!(wire, body);
        assert!(neutral.is_empty());
    }
    #[test]
    fn only_literal_loopback_http_and_pinned_laya_models() {
        assert_eq!(
            LayaConfig::new("http://127.0.0.1:11435", DEFAULT_MODEL)
                .unwrap()
                .endpoint(),
            DEFAULT_ENDPOINT
        );
        assert!(LayaConfig::new("http://[::1]:1234", DEFAULT_MODEL).is_ok());
        for u in [
            "https://127.0.0.1:11435",
            "http://localhost:11435",
            "http://example.org",
            "http://192.168.1.1",
            "http://user:pass@127.0.0.1",
            "http://127.0.0.1/?key=x",
            "http://127.0.0.1/#x",
            "http://127.0.0.1/private",
            "http://127.0.0.1:0",
        ] {
            assert!(LayaConfig::new(u, DEFAULT_MODEL).is_err(), "{u}");
        }
        for m in ["jev-1.14.0", "laya", "laya:latest", "laya:bad model"] {
            assert!(LayaConfig::new(DEFAULT_ENDPOINT, m).is_err());
        }
    }
    #[test]
    fn score_confidence_is_ordinal_not_provider_entropy_or_pmax() {
        let r = json!({"model":DEFAULT_MODEL,"answers":{"fit":{"type":"score","score":1.3,"confidence":0.25,"probabilities":{"0":0.2,"1":0.3,"2":0.5}}}});
        assert_eq!(
            normalize_response(r, DEFAULT_MODEL).unwrap()["answers"]["fit"]["confidence"],
            0.0
        );
    }
    #[test]
    fn invalid_models_probabilities_and_scores_fail_closed() {
        for r in [
            json!({"model":"jev-1.14.0","answers":{}}),
            json!({"model":DEFAULT_MODEL,"answers":{"x":{"type":"score","score":1.0,"probabilities":{"0":0.2,"1":0.2}}}}),
            json!({"model":DEFAULT_MODEL,"answers":{"x":{"type":"score","score":9,"probabilities":{"0":0.5,"1":0.5}}}}),
            json!({"model":DEFAULT_MODEL,"answers":{"x":{"type":"noul","noul":2}}}),
        ] {
            assert!(normalize_response(r, DEFAULT_MODEL).is_err());
        }
    }
}
