mod common;
use common::*;
use serde_json::{Value, json};
use std::{path::Path, process::Command};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    time::{Duration, timeout},
};

const BIN: &str = env!("CARGO_BIN_EXE_switchbite");
#[test]
fn doctor_names_config_overrides_without_disclosing_values() {
    let output = Command::new(BIN)
        .env_clear()
        .env("OPENCODE_CLI_CONFIG_CONTENT", "PRIVATE-CONFIG-SENTINEL")
        .args(["doctor", "--client", "opencode"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("PRIVATE-CONFIG-SENTINEL"));
    let result: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        result["configuration_overrides_present"],
        json!(["OPENCODE_CLI_CONFIG_CONTENT"])
    );
    assert_eq!(result["judge_configured"], false);
}
fn cli(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn native_cli_in_unrelated_directory_is_offline_and_has_no_execution_side_effects() {
    let dir = tempfile::tempdir().unwrap();
    let described = cli(dir.path(), &["describe"]);
    assert!(described.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&described.stdout).unwrap(),
        switchbite::describe()
    );
    for (op, request) in [
        ("decide", replay()),
        (
            "validate_plan",
            json!({"version":1,"nodes":[node("a",&[])]}),
        ),
        ("feedback", feedback()),
        (
            "decide",
            serde_json::from_slice(include_bytes!("../examples/crm-replay.json")).unwrap(),
        ),
        (
            "validate_plan",
            serde_json::from_slice(include_bytes!("../examples/plan.json")).unwrap(),
        ),
        (
            "feedback",
            serde_json::from_slice(include_bytes!("../examples/feedback.json")).unwrap(),
        ),
    ] {
        let path = dir.path().join("request.json");
        std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
        let out = cli(
            dir.path(),
            &["invoke", op, "--request-file", path.to_str().unwrap()],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            switchbite::Plugin::default().invoke(op, &request).unwrap()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    let path = dir.path().join("request.json");
    for raw in [
        b"{\"SECRET\":1,\"SECRET\":2}".to_vec(),
        vec![b'x'; 65537],
        b"NaN SECRET".to_vec(),
    ] {
        std::fs::write(&path, raw).unwrap();
        let out = cli(
            dir.path(),
            &["invoke", "decide", "--request-file", path.to_str().unwrap()],
        );
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("SECRET"));
    }
    assert!(!cli(dir.path(), &["run", "SECRET"]).status.success());
}

struct Host {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    dir: tempfile::TempDir,
}
impl Host {
    fn start() -> Self {
        Self::start_args(&[])
    }
    fn start_args(args: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut child = tokio::process::Command::new(BIN)
            .args(args)
            .env_clear()
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
            dir,
        }
    }
    async fn send(&mut self, v: Value) {
        let mut raw = serde_json::to_vec(&v).unwrap();
        raw.push(b'\n');
        self.input.write_all(&raw).await.unwrap();
    }
    async fn receive(&mut self) -> Value {
        let mut line = String::new();
        let size = timeout(Duration::from_secs(15), self.output.read_line(&mut line))
            .await
            .expect("MCP response timeout")
            .unwrap();
        assert!(size > 0, "MCP server closed unexpectedly");
        serde_json::from_str(&line).unwrap()
    }
    async fn call(&mut self, id: i64, name: &str, args: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})).await;
        let response = self.receive().await;
        assert_eq!(response["id"], id);
        response
    }
    async fn initialize(&mut self, version: &str) {
        self.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"independent-crm-host","version":"1"}}})).await;
        let r = self.receive().await;
        assert_eq!(r["result"]["protocolVersion"], version);
        assert_eq!(
            r["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
    }
    async fn finish(mut self) {
        self.input.shutdown().await.unwrap();
        drop(self.input);
        assert!(
            timeout(Duration::from_secs(5), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        assert_eq!(std::fs::read_dir(self.dir.path()).unwrap().count(), 0);
    }
}
#[tokio::test]
async fn actual_mcp_stdio_legacy_lifecycle_real_domain_structured_content() {
    for version in ["2024-11-05", "2025-06-18"] {
        let mut h = Host::start();
        h.initialize(version).await;
        h.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
            .await;
        let listed = h.receive().await;
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().any(|t| t["name"] == "invoke"));
        let desc = h.call(3, "describe", json!({})).await;
        assert_eq!(desc["result"]["structuredContent"], switchbite::describe());
        for (i, (op, request)) in [
            ("decide", replay()),
            (
                "validate_plan",
                json!({"version":1,"nodes":[node("send",&["translate"]),node("translate",&[])]}),
            ),
            ("feedback", feedback()),
        ]
        .into_iter()
        .enumerate()
        {
            let expected = switchbite::Plugin::default().invoke(op, &request).unwrap();
            let response = h
                .call(
                    4 + i as i64,
                    "invoke",
                    json!({"operation":op,"request":request}),
                )
                .await;
            assert_eq!(response["result"]["isError"], false);
            assert_eq!(response["result"]["structuredContent"], expected);
            assert_eq!(
                serde_json::from_str::<Value>(
                    response["result"]["content"][0]["text"].as_str().unwrap()
                )
                .unwrap(),
                expected
            );
            assert!(
                response["result"]["structuredContent"]
                    .get("result")
                    .is_none()
            );
        }
        let offline = h
            .call(
                10,
                "invoke",
                json!({"operation":"decide","request":request()}),
            )
            .await;
        assert_eq!(
            offline["result"]["structuredContent"]["hold_reasons"],
            json!(["judge_unavailable"])
        );
        let cycle=h.call(11,"invoke",json!({"operation":"validate_plan","request":{"version":1,"nodes":[node("a",&["b"]),node("b",&["a"])]}})).await;
        assert_eq!(
            cycle["result"]["structuredContent"]["issues"],
            json!(["dependency_cycle"])
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn actual_cli_profile_discovers_typed_tools_and_invokes_without_version_boilerplate() {
    let mut h = Host::start_args(&["serve", "--profile", "cli", "--offline"]);
    h.initialize("2025-11-25").await;
    h.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .await;
    let listed = h.receive().await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4);
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["outputSchema"]["type"], "object");
    }
    let status = h.call(3, "switchbite_status", json!({})).await;
    assert_eq!(
        status["result"]["structuredContent"]["judge_configured"],
        false
    );
    for (i, (name, mut data)) in [
        ("switchbite_decide", replay()),
        (
            "switchbite_validate_plan",
            json!({"version":1,"nodes":[node("a",&[])]}),
        ),
        ("switchbite_feedback", feedback()),
    ]
    .into_iter()
    .enumerate()
    {
        data.as_object_mut().unwrap().remove("version");
        let result = h.call(4 + i as i64, name, data).await;
        assert_eq!(result["result"]["isError"], false, "{result}");
        assert_eq!(result["result"]["structuredContent"]["kind"], "proposal");
    }
    let refused = h
        .call(
            8,
            "invoke",
            json!({"operation":"feedback","request":feedback()}),
        )
        .await;
    assert_eq!(refused["result"]["isError"], true);
    h.finish().await;
}
#[tokio::test]
async fn mcp_rejects_unknown_malformed_oversized_and_stale_requests_without_echoing() {
    let mut h = Host::start();
    h.initialize("2025-06-18").await;
    let mut unknown = request();
    unknown["task"]["SECRET"] = json!("SECRET");
    let mut stale = replay();
    stale["candidates"][0]["id"] = json!("different-id");
    let mut oversized = feedback();
    oversized["extensions"] =
        json!({"a":"x".repeat(24000),"b":"x".repeat(24000),"c":"x".repeat(24000)});
    for (i, args) in [
        json!({"operation":"decide","request":unknown}),
        json!({"operation":"decide","request":stale}),
        json!({"operation":"feedback","request":oversized}),
        json!({"operation":"run","request":{}}),
        json!({"operation":"decide","request":{}}),
        json!({"operation":"decide","request":request(),"command":"SECRET"}),
    ]
    .into_iter()
    .enumerate()
    {
        let response = h.call(20 + i as i64, "invoke", args).await;
        assert_eq!(response["result"]["isError"], true);
        assert!(!response.to_string().contains("SECRET"));
        assert!(response["result"]["structuredContent"]["error"]["code"].is_string());
    }
    h.finish().await;
}
#[tokio::test]
async fn mcp_framing_closes_on_duplicate_members_malformed_or_excessive_lines() {
    for raw in [
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"id\":2}\n".to_vec(),
        b"NaN SECRET\n".to_vec(),
        vec![b'x'; 131_073],
    ] {
        let mut h = Host::start();
        // Broken pipe is expected if the input guard closes while receiving.
        let _ = h.input.write_all(&raw).await;
        let _ = h.input.shutdown().await;
        drop(h.input);
        let _ = timeout(Duration::from_secs(5), h.child.wait())
            .await
            .expect("frame guard did not close")
            .unwrap();
        let mut output = String::new();
        use tokio::io::AsyncReadExt;
        h.output.read_to_string(&mut output).await.unwrap();
        assert!(output.is_empty());
    }
}

#[test]
fn package_exact_allowlist_hashes_determinism_and_existing_output_preservation() {
    let dir = tempfile::tempdir().unwrap();
    let one = dir.path().join("one.zip");
    let two = dir.path().join("two.zip");
    let first = switchbite::package::build(Path::new(BIN), &one).unwrap();
    let second = switchbite::package::build(Path::new(BIN), &two).unwrap();
    assert_eq!(first["sha256"], second["sha256"]);
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&one).unwrap()).unwrap();
    assert_eq!(zip.len(), 14);
    let manifest: Value = {
        use std::io::Read;
        let mut raw = Vec::new();
        zip.by_name("plugin.json")
            .unwrap()
            .read_to_end(&mut raw)
            .unwrap();
        serde_json::from_slice(&raw).unwrap()
    };
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        manifest["entrypoints"]["x86_64-pc-windows-msvc"],
        "bin/switchbite.exe"
    );
    assert!(manifest.get("entrypoint").is_none());
    assert_eq!(manifest["id"], "switchbite");
    assert_eq!(manifest["files"].as_object().unwrap().len(), 13);
    for name in [
        "LICENSE",
        "README.md",
        "THIRD_PARTY_NOTICES.html",
        "RUST_STANDARD_LIBRARY_NOTICES.html",
    ] {
        assert!(zip.by_name(name).unwrap().size() > 100);
    }
    for (name, hash) in manifest["files"].as_object().unwrap() {
        use std::io::Read;
        let mut raw = Vec::new();
        zip.by_name(name).unwrap().read_to_end(&mut raw).unwrap();
        assert_eq!(switchbite::core::sha256(&raw), hash.as_str().unwrap());
    }
    assert!(switchbite::package::build(Path::new(BIN), &one).is_err());
    assert_eq!(
        first["sha256"],
        switchbite::core::sha256(&std::fs::read(&one).unwrap())
    );
    let fake = dir.path().join("fake.exe");
    std::fs::write(&fake, b"not native").unwrap();
    assert!(switchbite::package::build(&fake, &dir.path().join("bad.zip")).is_err());
}
