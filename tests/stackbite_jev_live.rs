//! Opt-in installed-project MCP qualification. Synthetic routing only; no worker dispatch.
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[tokio::test]
#[ignore = "requires SWITCHBITE_JEV_WORKSPACE and an authorized TYPESAFE_API_KEY"]
async fn installed_jev_plugin_validates_coordinator_plan_and_routes_task_profiles() {
    assert!(
        !std::env::var("TYPESAFE_API_KEY")
            .unwrap_or_default()
            .trim()
            .is_empty()
    );
    let workspace = PathBuf::from(
        std::env::var_os("SWITCHBITE_JEV_WORKSPACE").expect("explicit test workspace"),
    );
    let config: Value =
        serde_json::from_slice(&std::fs::read(workspace.join(".mcp.json")).unwrap()).unwrap();
    let server = &config["mcpServers"][switchbite::stackbite_plugin::SERVER_NAME];
    let executable = server["command"].as_str().unwrap();
    let args: Vec<_> = server["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert!(PathBuf::from(executable).is_absolute());
    assert_eq!(args, ["serve", "--profile", "agent", "--judge", "jev"]);
    assert_eq!(server["env"]["TYPESAFE_API_KEY"], "${TYPESAFE_API_KEY:-}");
    let mut child = Command::new(executable)
        .args(args)
        .current_dir(&workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    async fn rpc(
        input: &mut tokio::process::ChildStdin,
        output: &mut BufReader<tokio::process::ChildStdout>,
        id: u64,
        method: &str,
        params: Value,
    ) -> Value {
        let request = json!({"jsonrpc":"2.0", "id":id,"method":method,"params":params});
        input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(45), output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert!(line.len() < 262144);
        let result: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(result["id"], id);
        assert!(result.get("error").is_none(), "protocol failure");
        result["result"].clone()
    }
    let initialized = rpc(&mut input, &mut output, 1, "initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"stackbite-jev-plugin-qualification","version":"1"}})).await;
    let instructions = initialized["instructions"].as_str().unwrap();
    assert!(instructions.contains("decompose"));
    assert!(instructions.contains("verified successful receipts"));
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    let tools = rpc(&mut input, &mut output, 2, "tools/list", json!({})).await;
    assert_eq!(tools["tools"].as_array().unwrap().len(), 4);
    let status = rpc(
        &mut input,
        &mut output,
        3,
        "tools/call",
        json!({"name":"switchbite_status","arguments":{}}),
    )
    .await;
    assert_eq!(status["structuredContent"]["model"], "jev-1.14.0");
    assert_eq!(status["structuredContent"]["judge_configured"], true);
    let example: Value =
        serde_json::from_str(include_str!("../examples/stackbite-workflow.json")).unwrap();
    let plan = rpc(
        &mut input,
        &mut output,
        4,
        "tools/call",
        json!({"name":"switchbite_validate_plan","arguments":example["plan"]}),
    )
    .await;
    assert_eq!(plan["structuredContent"]["valid"], true);
    let mut decisions = Vec::new();
    // These are independent synthetic routing probes, not a scheduler advancing
    // the supplied DAG and not evidence of completed dependency tasks.
    for (index, route) in example["route_examples"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let mut request = route.clone();
        request["pool"] = example["pool"].clone();
        request["preferences"] = example["preferences"].clone();
        let response = rpc(
            &mut input,
            &mut output,
            index as u64 + 5,
            "tools/call",
            json!({"name":"switchbite_route","arguments":request}),
        )
        .await;
        assert_ne!(
            response["isError"], true,
            "routing error: {}",
            response["structuredContent"]
        );
        let decision = response["structuredContent"].clone();
        switchbite::contracts::validate(&decision, &switchbite::routing::output_schema()).unwrap();
        assert!(matches!(
            decision["status"].as_str(),
            Some("ready" | "hold")
        ));
        if decision["status"] == "ready" {
            assert_eq!(decision["next_action"], "recheck_and_dispatch");
            assert!(
                example["pool"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["id"] == decision["candidate_id"])
            );
            if route["task"]["id"] == "review" {
                assert_ne!(decision["candidate_id"], "claude:implementation");
            }
        }
        decisions.push(json!({"task":route["task"]["id"], "decision":decision}));
    }
    let proof = json!({"synthetic_only":true,"worker_execution":false,"dependency_completion_claimed":false,"installed_executable":executable,"tools_discovered":4,"status":status["structuredContent"],"plan":plan["structuredContent"],"routing_probes":decisions});
    std::fs::write(
        workspace.join("mcp-jev-verification.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    println!(
        "{}",
        json!({"model":"jev-1.14.0","plan_valid":true,"worker_execution":false,"routes":decisions.iter().map(|d| json!({"task":d["task"],"status":d["decision"]["status"],"next_action":d["decision"]["next_action"],"candidate_id":d["decision"]["candidate_id"]})).collect::<Vec<_>>() })
    );
    input.shutdown().await.unwrap();
    drop(input);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[test]
fn coordinator_example_uses_the_real_plan_and_route_contracts() {
    let example: Value =
        serde_json::from_str(include_str!("../examples/stackbite-workflow.json")).unwrap();
    let plan = switchbite::cli_tools::normalize("validate_plan", &example["plan"]).unwrap();
    assert_eq!(
        switchbite::Plugin::default()
            .invoke("validate_plan", &plan)
            .unwrap()["valid"],
        true
    );
    for route in example["route_examples"].as_array().unwrap() {
        let mut request = route.clone();
        request["pool"] = example["pool"].clone();
        request["preferences"] = example["preferences"].clone();
        switchbite::agent::validate_arguments(&request).unwrap();
    }
}
