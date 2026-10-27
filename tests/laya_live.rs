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
    let server = &config["mcpServers"][switchbite::stackbite_plugin::SERVER_NAME];
    let command = server["command"].as_str().unwrap();
    let args = server["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(PathBuf::from(command).is_absolute());
    assert!(args.windows(2).any(|a| a == ["--judge", "laya"]));
    assert!(args.windows(2).any(|a| a == ["--profile", "workflow"]));
    assert!(
        args.windows(2)
            .any(|a| a[0] == "--workspace" && std::path::Path::new(a[1]) == workspace)
    );
    let model = args
        .windows(2)
        .find(|a| a[0] == "--model")
        .expect("configured Laya model")[1];
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
            .contains("plan")
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
    let mut names: Vec<_> = listed["result"]["tools"]
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

    // No executable or host: routing cannot launch the synthetic worker.
    let request = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"switchbite_start","arguments":{"request":{"workspace":workspace.to_str().unwrap(),"plan":{"nodes":[{"id":"local-html","objective":"Create a self-contained HTML page with inline CSS, a title, three status cards and responsive layout.","role":"mockup","requirements":["html.write"],"write_paths":["synthetic.html"]}]},"pool":[{"id":"synthetic-writer","description":"Available worker for self-contained HTML and CSS mockups","capabilities":["html.write"],"available":true}]}}}});
    writer
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let started = recv(&mut reader).await;
    assert_ne!(started["result"]["isError"], true);
    let run = started["result"]["structuredContent"]["run_id"]
        .as_str()
        .unwrap();
    let next = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"switchbite_next","arguments":{"run_id":run}}});
    writer
        .write_all(format!("{next}\n").as_bytes())
        .await
        .unwrap();
    let routed = recv(&mut reader).await;
    assert_ne!(
        routed["result"]["isError"], true,
        "local routing failed: {}",
        routed["result"]["structuredContent"]
    );
    let event = routed["result"]["structuredContent"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["stage"] == "route")
        .expect("route receipt");
    assert!(matches!(event["status"].as_str(), Some("ready" | "hold")));
    assert!(!workspace.join("synthetic.html").exists());
    let cancel = json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"switchbite_next","arguments":{"run_id":run,"operation":"cancel"}}});
    writer
        .write_all(format!("{cancel}\n").as_bytes())
        .await
        .unwrap();
    let cancelled = recv(&mut reader).await;
    assert_ne!(cancelled["result"]["isError"], true);
    assert_eq!(
        cancelled["result"]["structuredContent"]["status"],
        "cancelled"
    );
    let proof = json!({"synthetic_only":true,"configured_judge":"laya","configured_model":model,"tools_discovered":names,"routing":{"status":event["status"],"reason_code":event["reason_code"],"next_action":event["next_action"]},"cloud_api_key_removed":true,"worker_execution":false});
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
