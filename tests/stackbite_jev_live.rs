//! Opt-in installed-project MCP qualification. Synthetic routing only; no worker dispatch.
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[tokio::test]
#[ignore = "requires SWITCHBITE_JEV_WORKSPACE and an authorized TYPESAFE_API_KEY"]
async fn installed_jev_workflow_profile_exposes_controls_and_routes_without_dispatch() {
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
    assert_eq!(
        &args[..6],
        [
            "serve",
            "--profile",
            "workflow",
            "--judge",
            "jev",
            "--workspace"
        ]
    );
    assert_eq!(args.len(), 7);
    assert_eq!(PathBuf::from(args[6]), workspace);
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
    assert!(instructions.contains("plan"));
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    let listed = rpc(&mut input, &mut output, 2, "tools/list", json!({})).await;
    let mut names: Vec<_> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["switchbite_next", "switchbite_observe", "switchbite_start"]
    );
    let started = rpc(&mut input, &mut output, 3, "tools/call", json!({"name":"switchbite_start","arguments":{"request":{"workspace":workspace.to_str().unwrap(),"goal":"Plan a synthetic writing task","pool":[]}}})).await;
    assert_ne!(started["isError"], true);
    let run = started["structuredContent"]["run_id"].as_str().unwrap();
    let plan_action = &started["structuredContent"]["actions"][0];
    assert_eq!(plan_action["kind"], "plan");
    let action_id = plan_action["id"].as_str().unwrap();
    let mut controls = Vec::new();
    for (id, op, expected) in [
        (4, "status", "running"),
        (5, "pause", "paused"),
        (6, "resume", "running"),
    ] {
        let response = rpc(
            &mut input,
            &mut output,
            id,
            "tools/call",
            json!({"name":"switchbite_next","arguments":{"run_id":run,"operation":op}}),
        )
        .await;
        assert_ne!(response["isError"], true);
        assert_eq!(response["structuredContent"]["status"], expected);
        controls.push(json!({"operation":op,"status":expected}));
    }
    let observed = rpc(&mut input, &mut output, 7, "tools/call", json!({"name":"switchbite_observe","arguments":{"run_id":run,"event":{"action_id":action_id,"event_id":"qualification-plan","result":{"plan":{"nodes":[{"id":"draft","objective":"Draft a synthetic report","depends_on":[],"acceptance":["Report drafted"],"write_paths":["draft.txt"]}]}}}}})).await;
    assert_ne!(observed["isError"], true);
    assert_eq!(observed["structuredContent"]["tasks"][0]["id"], "draft");
    let cancelled = rpc(
        &mut input,
        &mut output,
        8,
        "tools/call",
        json!({"name":"switchbite_next","arguments":{"run_id":run,"operation":"cancel"}}),
    )
    .await;
    assert_ne!(cancelled["isError"], true);
    assert_eq!(cancelled["structuredContent"]["status"], "cancelled");
    controls.push(json!({"operation":"cancel","status":"cancelled"}));

    // Candidate has no executable or local host: a ready route returns a host
    // action and cannot start a worker in this synthetic qualification.
    let route_start = rpc(&mut input, &mut output, 9, "tools/call", json!({"name":"switchbite_start","arguments":{"request":{"workspace":workspace.to_str().unwrap(),"plan":{"nodes":[{"id":"writing","objective":"Draft a short synthetic report","requirements":["writing"],"write_paths":["synthetic-report.txt"]}]},"pool":[{"id":"synthetic-writer","description":"Available writing worker for a short report","capabilities":["writing"],"available":true}]}}})).await;
    assert_ne!(route_start["isError"], true);
    let route_run = route_start["structuredContent"]["run_id"].as_str().unwrap();
    let routed = rpc(
        &mut input,
        &mut output,
        10,
        "tools/call",
        json!({"name":"switchbite_next","arguments":{"run_id":route_run}}),
    )
    .await;
    assert_ne!(
        routed["isError"], true,
        "routing failed: {}",
        routed["structuredContent"]
    );
    let events = routed["structuredContent"]["events"].as_array().unwrap();
    let route_event = events
        .iter()
        .find(|event| event["stage"] == "route")
        .expect("route receipt");
    assert!(matches!(
        route_event["status"].as_str(),
        Some("ready" | "hold")
    ));
    assert!(!workspace.join("synthetic-report.txt").exists());
    let route_cancel = rpc(
        &mut input,
        &mut output,
        11,
        "tools/call",
        json!({"name":"switchbite_next","arguments":{"run_id":route_run,"operation":"cancel"}}),
    )
    .await;
    assert_ne!(route_cancel["isError"], true);
    let proof = json!({"synthetic_only":true,"worker_execution":false,"configured_judge":"jev","tools_discovered":names,"goal_plan_observed":true,"controls":controls,"routing":{"status":route_event["status"],"reason_code":route_event["reason_code"],"next_action":route_event["next_action"]}});
    std::fs::write(
        workspace.join("mcp-jev-verification.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    println!("{}", proof);
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
