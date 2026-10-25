mod common;

use serde_json::{Value, json};
use std::sync::Arc;
use switchbite::{ContractError, Judge, Plugin, agent, contracts, routing};

const LOCAL_MODEL: &str = "laya-local-0.1";

struct LocalJudge;
impl Judge for LocalJudge {
    fn model(&self) -> &str {
        LOCAL_MODEL
    }
    fn judge(&self, body: &Value) -> Result<Value, ContractError> {
        assert_eq!(body["model"], LOCAL_MODEL);
        let mut response = common::judgment(body);
        response["model"] = body["model"].clone();
        Ok(response)
    }
}

struct WrongIdentityJudge;
impl Judge for WrongIdentityJudge {
    fn model(&self) -> &str {
        LOCAL_MODEL
    }
    fn judge(&self, body: &Value) -> Result<Value, ContractError> {
        // Simulate a provider returning a typed result for a different model.
        Ok(common::judgment(body))
    }
}

fn route_request() -> Value {
    json!({"task":{"id":"task:identity","objective":"Translate the supplied text."},
        "pool":[{"id":"translator","description":"Translates text.","capabilities":["translate"],"available":true}]})
}

#[test]
fn configured_model_reaches_request_result_and_status() {
    let plugin = Plugin::with_judge(LocalJudge);
    let result = plugin.invoke("decide", &common::request()).unwrap();
    assert_eq!(result["scores"]["model"], LOCAL_MODEL);
    assert_eq!(switchbite::cli_tools::status(&plugin)["model"], LOCAL_MODEL);
    assert_eq!(
        switchbite::judgment_body(&common::request()).unwrap()["model"],
        contracts::MODEL
    );
    assert_ne!(
        result["scores"]["request_sha256"],
        Plugin::with_judge(|body: &Value| Ok(common::judgment(body)))
            .invoke("decide", &common::request())
            .unwrap()["scores"]["request_sha256"]
    );
}

#[test]
fn default_judge_never_accepts_a_foreign_replay() {
    let mut request = common::replay();
    request["judgment"]["model"] = json!(LOCAL_MODEL);
    assert_eq!(
        Plugin::default().invoke("decide", &request),
        Err(ContractError::InvalidJudgment)
    );
    assert!(
        Plugin::with_judge(LocalJudge)
            .invoke("decide", &request)
            .unwrap()["ready"]
            .as_bool()
            .unwrap()
    );
    request["judgment"]["model"] = json!(contracts::MODEL);
    assert_eq!(
        Plugin::with_judge(LocalJudge).invoke("decide", &request),
        Err(ContractError::InvalidJudgment)
    );
}

#[test]
fn live_provider_model_must_match_the_configured_identity() {
    assert_eq!(
        Plugin::with_judge(WrongIdentityJudge).invoke("decide", &common::request()),
        Err(ContractError::InvalidJudgment)
    );
    assert_eq!(
        routing::route(&route_request(), Some(&WrongIdentityJudge), 10),
        Err(ContractError::InvalidJudgment)
    );
}

#[test]
fn identity_preservation_does_not_relax_typed_score_checks() {
    let mut request = common::request();
    let body = switchbite::core::judgment_body_for_model(&request, LOCAL_MODEL).unwrap();
    let good = LocalJudge.judge(&body).unwrap();
    let key = common::fit_key("translator:ko/en");
    for field in ["confidence", "score", "probabilities", "legend"] {
        let mut bad = good.clone();
        bad["answers"][&key][field] = match field {
            "probabilities" => json!({"0":0.0,"1":0.0,"2":0.0,"3":0.1}),
            "legend" => json!({"0":"wrong","1":"wrong","2":"wrong","3":"wrong"}),
            _ => json!(0.0),
        };
        request["judgment"] = bad;
        assert_eq!(
            Plugin::with_judge(LocalJudge).invoke("decide", &request),
            Err(ContractError::InvalidJudgment),
            "{field}"
        );
    }
}

#[test]
fn routing_binds_identity_and_agent_bridge_preserves_it() {
    let request = route_request();
    let result = routing::route(&request, Some(&LocalJudge), 10).unwrap();
    assert_eq!(result["status"], "ready");
    assert_eq!(result["snapshot"]["judge_model"], LOCAL_MODEL);
    assert_ne!(
        result["snapshot"]["input_sha256"],
        routing::snapshot(&request, 10).unwrap()["input_sha256"]
    );
    let callback: Arc<agent::AgentJudge> = Arc::new(|body| {
        LocalJudge
            .judge(body)
            .map_err(|e| switchbite::runtime::RuntimeError::from_code(e.code()))
    });
    let configured = Arc::new(agent::ConfiguredJudge::new(LOCAL_MODEL, callback).unwrap());
    assert_eq!(
        agent::configured_status(Some(&configured))["model"],
        LOCAL_MODEL
    );
    assert_eq!(
        agent::route_with_judge(&request, Some(configured), 10).unwrap(),
        result
    );
}

#[test]
fn nonlegacy_replay_requires_matching_model_binding() {
    let request = route_request();
    let mut replay = routing::snapshot_for_model(&request, 10, LOCAL_MODEL).unwrap();
    // A replay is validated without calling its provider.
    let legacy = json!({"version":1,"task":{"id":"task:identity","objective":"Translate the supplied text.",
        "requirements":[],"context_refs":[],"extensions":{}},"candidates":[{"id":"translator",
        "description":"Translates text.","capabilities":["translate"],"available":true,"extensions":{}}],"preferences":""});
    replay["judgment"] = LocalJudge
        .judge(&switchbite::core::judgment_body_for_model(&legacy, LOCAL_MODEL).unwrap())
        .unwrap();
    let mut replay_request = request;
    replay_request["replay"] = replay;
    assert_eq!(
        routing::route(&replay_request, Some(&LocalJudge), 11).unwrap()["status"],
        "ready"
    );
    assert_eq!(
        routing::route(&replay_request, None, 11).unwrap()["reason_codes"],
        json!(["replay_mismatch"])
    );
    replay_request["replay"]["judgment"]["model"] = json!(contracts::MODEL);
    assert_eq!(
        routing::route(&replay_request, Some(&LocalJudge), 11),
        Err(ContractError::InvalidJudgment)
    );
}

#[test]
fn configured_identity_is_bounded_and_rejects_transport_details() {
    for model in [
        "".to_owned(),
        "x".repeat(129),
        "laya\nsecret".into(),
        "http://localhost".into(),
        "http://localhost?token=secret".into(),
    ] {
        assert!(contracts::validate_model(&model).is_err());
    }
}
