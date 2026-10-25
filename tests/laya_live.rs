//! Opt-in qualification of the actual installed MCP executable against local
//! Laya. Synthetic data only; never starts a worker or a cloud judge.
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[tokio::test]
#[ignore = "requires an explicitly installed project plugin and local Laya model"]
async fn installed_orchestrator_plugin_discovers_and_routes_with_real_local_laya() {
    let workspace = PathBuf::from(
        std::env::var_os("SWITCHBITE_LAYA_WORKSPACE").expect("set explicit test workspace"),
    );
    let config: Value =
        serde_json::from_slice(&std::fs::read(workspace.join(".mcp.json")).unwrap()).unwrap();
    let server = config["mcpServers"]
        .get(switchbite::stackbite_plugin::SERVER_NAME)
        .unwrap_or(&config["mcpServers"]["switchbite"]);
    let command = server["command"].as_str().unwrap();
    let args = server["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(PathBuf::from(command).is_absolute());
    assert!(args.windows(2).any(|a| a == ["--judge", "laya"]));
    let mut child = Command::new(command)
        .args(&args)
        .current_dir(&workspace)
        .env_remove("TYPESAFE_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut writer = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"stackbite-coordinator-local-qualification","version":"1"}}});
    writer
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .unwrap();
    async fn recv(reader: &mut BufReader<tokio::process::ChildStdout>) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(40), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert!(line.len() < 262144);
        serde_json::from_str(&line).unwrap()
    }
    let initialized = recv(&mut reader).await;
    assert!(
        initialized["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("newly ready")
    );
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    writer
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n")
        .await
        .unwrap();
    let listed = recv(&mut reader).await;
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 4);
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"switchbite_status\",\"arguments\":{}}}\n").await.unwrap();
    let status = recv(&mut reader).await;
    assert_eq!(
        status["result"]["structuredContent"]["model"],
        "laya:multilingual"
    );
    assert_eq!(
        status["result"]["structuredContent"]["judge_configured"],
        true
    );
    let request = json!({"task":{"id":"local-html","objective":"Create one self-contained HTML mockup page with inline CSS in the supplied project. Requirements and acceptance are fully specified: a title, three status cards, and responsive layout. No external research, packages, shell commands or other files are needed.","requirements":["html.write"],"context_refs":["The coordinator supplies the exact output path and full page contract in the worker assignment. This routing call performs no file edits."]},"pool":[{"id":"claude:html-writer","description":"Claude Code implementation worker with Read, Write and Edit tools for building complete self-contained HTML and CSS mockups. This declared profile is available for the bounded synthetic routing probe.","capabilities":["html.write"],"available":true}],"policy":{"profile":"mockup"}});
    let call = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"switchbite_route","arguments":request}});
    writer
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    let routed = recv(&mut reader).await;
    assert_ne!(routed["result"]["isError"], true);
    let decision = &routed["result"]["structuredContent"];
    switchbite::contracts::validate(decision, &switchbite::routing::output_schema()).unwrap();
    assert!(matches!(
        decision["status"].as_str(),
        Some("ready" | "hold")
    ));
    assert_eq!(decision["snapshot"]["judge_model"], "laya:multilingual");
    if decision["status"] == "ready" {
        assert_eq!(decision["candidate_id"], "claude:html-writer");
    }
    let proof = json!({"synthetic_only":true,"installed_executable":command,"tools_discovered":4,"status":status["result"]["structuredContent"],"route":decision,"cloud_api_key_removed":true,"worker_execution":false});
    std::fs::write(
        workspace.join("mcp-live-verification.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    println!("{}", proof);
    writer.shutdown().await.unwrap();
    drop(writer);
    let exited = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(exited.success());
}
