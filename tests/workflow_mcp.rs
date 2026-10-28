use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Client {
    async fn open(data: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_switchbite"))
            .args(["serve", "--profile", "workflow", "--offline"])
            .env("LOCALAPPDATA", data)
            .env("XDG_DATA_HOME", data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            input,
            output,
        };
        client.rpc(1,"initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"workflow-qualification","version":"1"}})).await;
        client
            .input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .unwrap();
        client
    }
    async fn rpc(&mut self, id: u64, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), self.output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }
    async fn tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        let value = self
            .rpc(id, "tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        assert_ne!(value["isError"], true, "{value}");
        value["structuredContent"].clone()
    }
    async fn close(mut self) {
        self.input.shutdown().await.unwrap();
        drop(self.input);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

#[tokio::test]
async fn report_and_coordinator_reply_survive_stdio_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    std::fs::create_dir(&workspace).unwrap();
    let data = temp.path().join("data");
    let mut first = Client::open(&data).await;
    let started = first.tool(2, "switchbite_start", json!({"request":{"workspace":workspace,"goal":"Make a report","pool":[],"plan":{"nodes":[{"id":"draft","objective":"Write the report","write_paths":["report.md"]}]}}})).await;
    let run = started["run_id"].as_str().unwrap();
    let report = json!({"id":"user-direction","source":"user","kind":"request_change","message":"Use the updated acceptance criteria."});
    let state = first
        .tool(
            3,
            "switchbite_report",
            json!({"run_id":run,"report":report}),
        )
        .await;
    assert_eq!(state["feedback"][0]["status"], "pending");
    first.close().await;
    let mut second = Client::open(&data).await;
    let state = second
        .tool(
            2,
            "switchbite_next",
            json!({"run_id":run,"operation":"status"}),
        )
        .await;
    let action = state["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "replan")
        .unwrap();
    second.tool(3, "switchbite_observe", json!({"run_id":run,"event":{"action_id":action["id"],"event_id":"coordinator-reply","result":{"plan":null,"reply":"Confirmed the revised criteria.","interrupt_tasks":[]}}})).await;
    second.close().await;
    let mut third = Client::open(&data).await;
    let state = third
        .tool(
            2,
            "switchbite_report",
            json!({"run_id":run,"report":report}),
        )
        .await;
    assert_eq!(state["feedback"].as_array().unwrap().len(), 1);
    assert_eq!(state["feedback"][0]["status"], "processed");
    assert_eq!(
        state["feedback"][0]["reply"],
        "Confirmed the revised criteria."
    );
    third.close().await;
}

#[tokio::test]
async fn generic_stdio_connection_keeps_two_workspaces_across_restart() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let data = temp.path().join("data");
    let mut client = Client::open(&data).await;
    let tools = client.rpc(2, "tools/list", json!({})).await;
    assert_eq!(tools["tools"].as_array().unwrap().len(), 4);
    let first = client
        .tool(
            3,
            "switchbite_start",
            json!({"request":{"workspace":a,"goal":"First project","pool":[]}}),
        )
        .await;
    let second = client
        .tool(
            4,
            "switchbite_start",
            json!({"request":{"workspace":b,"goal":"Second project","pool":[]}}),
        )
        .await;
    let arun = first["run_id"].as_str().unwrap();
    let brun = second["run_id"].as_str().unwrap();
    client
        .tool(
            5,
            "switchbite_next",
            json!({"run_id":arun,"operation":"pause"}),
        )
        .await;
    client.close().await;
    let mut restored = Client::open(&data).await;
    assert_eq!(
        restored
            .tool(
                2,
                "switchbite_next",
                json!({"run_id":arun,"operation":"status"})
            )
            .await["status"],
        "paused"
    );
    assert_eq!(
        restored
            .tool(
                3,
                "switchbite_next",
                json!({"run_id":brun,"operation":"status"})
            )
            .await["status"],
        "running"
    );
    restored.close().await;
}
