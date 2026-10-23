mod common;
use common::judgment;
use serde_json::{Value, json};
use std::{process::Command, sync::Arc};
use switchbite::{agent, contracts, mcp_server, runtime::RuntimeConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn request() -> Value {
    json!({"task":{"id":"csv:implementation","objective":"Implement CSV validation and regression tests."},
        "pool":[{"id":"pool:implementation","description":"Implementation worker for Rust CSV parsing.","capabilities":["code.change"],"available":true}]})
}

#[test]
fn agent_surface_is_four_tools_without_replay_or_digest_inputs() {
    let tools = agent::tools();
    assert_eq!(tools.len(), 4);
    assert!(tools.iter().all(|t| t.name != "switchbite_decide"));
    let route = tools.iter().find(|t| t.name == "switchbite_route").unwrap();
    assert!(route.input_schema["properties"].get("replay").is_none());
    assert_eq!(route.input_schema["required"], json!(["task", "pool"]));
    assert!(route.output_schema.is_some());
    agent::validate_arguments(&request()).unwrap();
    let mut forged = request();
    forged["replay"] = json!({});
    assert!(agent::validate_arguments(&forged).is_err());
    let mut missing_fact = request();
    missing_fact["pool"][0].as_object_mut().unwrap().remove("available");
    assert!(agent::validate_arguments(&missing_fact).is_err());
}

#[tokio::test]
async fn coordinator_discovers_routes_and_receives_one_clear_next_step_without_execution() {
    let (client, server) = tokio::io::duplex(262_144);
    let (server_read, server_write) = tokio::io::split(server);
    let judge: Arc<agent::AgentJudge> = Arc::new(|body| Ok(judgment(body)));
    let running = tokio::spawn(mcp_server::serve_agent(Some(judge), RuntimeConfig::default(), server_read, server_write));
    let (client_read, mut writer) = tokio::io::split(client);
    let mut reader = BufReader::new(client_read);
    async fn receive(reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>) -> Value {
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(10), reader.read_line(&mut line)).await.unwrap().unwrap();
        serde_json::from_str(&line).unwrap()
    }
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"coordinator-test","version":"1"}}});
    writer.write_all(format!("{initialize}\n").as_bytes()).await.unwrap();
    let initialized = receive(&mut reader).await;
    assert!(initialized["result"]["instructions"].as_str().unwrap().contains("newly ready"));
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n").await.unwrap();
    assert_eq!(receive(&mut reader).await["result"]["tools"].as_array().unwrap().len(), 4);
    let call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"switchbite_route","arguments":request()}});
    writer.write_all(format!("{call}\n").as_bytes()).await.unwrap();
    let result = receive(&mut reader).await;
    let decision = &result["result"]["structuredContent"];
    assert_ne!(result["result"]["isError"], true);
    assert_eq!(decision["status"], "ready");
    assert_eq!(decision["next_action"], "recheck_and_dispatch");
    assert_eq!(decision["candidate_id"], "pool:implementation");
    contracts::validate(decision, &switchbite::routing::output_schema()).unwrap();
    let text = result["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("recheck_and_dispatch"));
    assert!(!text.contains("input_sha256"));
    writer.shutdown().await.unwrap();
    drop(writer);
    tokio::time::timeout(std::time::Duration::from_secs(5), running).await.unwrap().unwrap().unwrap();
}

#[test]
fn setup_is_an_explicit_read_only_handoff_and_legacy_profile_remains_available() {
    let temporary = tempfile::tempdir().unwrap();
    let executable = env!("CARGO_BIN_EXE_switchbite");
    let output = Command::new(executable).env_clear().current_dir(temporary.path())
        .args(["setup", "--client", "claude", "--offline"]).output().unwrap();
    assert!(output.status.success());
    let setup: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(setup["profile"], "agent");
    assert_eq!(setup["writes_configuration"], false);
    let config: Value = serde_json::from_str(setup["mcp_config"].as_str().unwrap()).unwrap();
    assert_eq!(config["mcpServers"]["switchbite"]["args"], json!(["serve","--profile","agent","--offline"]));
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
    let guide = Command::new(executable).env_clear().arg("guide").output().unwrap();
    assert!(guide.status.success());
    assert!(String::from_utf8(guide.stdout).unwrap().contains("switchbite_route"));
}
