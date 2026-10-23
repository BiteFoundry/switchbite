mod common;
use common::*;
use serde_json::json;
use switchbite::{Plugin, cli_tools, contracts};

#[test]
fn agent_schema_normalizes_only_protocol_defaults_and_preserves_authority() {
    let mut input = replay();
    input.as_object_mut().unwrap().remove("version");
    input["task"].as_object_mut().unwrap().remove("extensions");
    input["candidates"][0]
        .as_object_mut()
        .unwrap()
        .remove("extensions");
    let canonical = cli_tools::normalize("decide", &input).unwrap();
    assert_eq!(canonical["version"], 1);
    assert_eq!(canonical["task"]["extensions"], json!({}));
    assert_eq!(canonical["candidates"][0]["extensions"], json!({}));
    assert_eq!(
        Plugin::default().invoke("decide", &canonical).unwrap()["candidate_id"],
        "translator:ko/en"
    );
    for value in [json!(true), json!(1.0), json!(2), json!("1")] {
        let mut bad = input.clone();
        bad["version"] = value;
        assert!(cli_tools::normalize("decide", &bad).is_err());
    }
    let mut missing = input.clone();
    missing["candidates"][0]
        .as_object_mut()
        .unwrap()
        .remove("available");
    assert!(cli_tools::normalize("decide", &missing).is_err());
    let mut unknown = input;
    unknown["launch_worker"] = json!(true);
    assert!(cli_tools::normalize("decide", &unknown).is_err());
}

#[test]
fn cli_metadata_is_typed_read_only_and_does_not_expose_configuration_secrets() {
    let tools = cli_tools::tools();
    assert_eq!(tools.len(), 4);
    for tool in &tools {
        assert_eq!(
            tool.annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );
        assert_eq!(
            tool.annotations.as_ref().unwrap().destructive_hint,
            Some(false)
        );
        assert!(tool.output_schema.is_some());
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&json!(false))
        );
    }
    assert!(
        tools
            .iter()
            .find(|t| t.name == "switchbite_decide")
            .unwrap()
            .input_schema
            .get("properties")
            .unwrap()
            .get("task")
            .is_some()
    );
    let p = Plugin::with_judge(|_: &serde_json::Value| Err(contracts::ContractError::JudgeFailed));
    let status = cli_tools::status(&p);
    assert_eq!(status["judge_configured"], true);
    assert_eq!(
        cli_tools::status(&Plugin::default())["judge_configured"],
        false
    );
    contracts::validate(&status, &cli_tools::status_schema()).unwrap();
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("TYPESAFE_API_KEY")
    );
}
