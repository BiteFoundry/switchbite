use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use switchbite::{activity::snapshot, workflow::Workflow};

fn setup() -> (tempfile::TempDir, String, Value, String) {
    let root = tempfile::tempdir().unwrap();
    let workflow = Workflow::new(root.path().to_path_buf());
    let started = workflow.start(&json!({"workspace":"workspace","pool":[{"id":"a","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"one","objective":"one","outputs":["src/one"]}]}})).unwrap();
    let run = started["run_id"].as_str().unwrap().to_owned();
    let execution = "owned-execution";
    let hash = format!("{:x}", Sha256::digest(execution.as_bytes()))[..24].to_owned();
    let output = root.path().join(&run).join("host/output");
    fs::create_dir_all(&output).unwrap();
    let state = json!({"status":"running","tasks":[{"id":"one","status":"running","execution_id":execution,"repair_rounds":0,"candidate":{"provider":"codex"}}],"actions":[]});
    (root, run, state, hash)
}
#[test]
fn reads_only_owned_stream_and_sanitizes_provider_events() {
    let (root, run, state, hash) = setup();
    let output = root.path().join(&run).join("host/output");
    fs::write(output.join(format!("{hash}.out")), concat!(
        "{\"type\":\"init\",\"prompt\":\"private prompt\"}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"hello password=hunter22\"}}\n",
        "{\"type\":\"unknown\",\"config\":\"private config\"}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"reasoning\",\"text\":\"private thought\"}}\n",
        "{\"type\":\"text\",\"part\":{\"text\":\"incomplete\"}}"
    )).unwrap();
    let value = snapshot(root.path(), &run, &state, None, None, 10_000).unwrap();
    let entries = value["selected_stream"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries[0]["text"].as_str().unwrap().contains("[REDACTED]"));
    let serialized = value.to_string();
    for secret in [
        "hunter22",
        "private prompt",
        "private config",
        "private thought",
        "incomplete",
    ] {
        assert!(!serialized.contains(secret));
    }
    let id = value["selected_stream"]["id"].as_str().unwrap();
    assert_eq!(
        snapshot(root.path(), &run, &state, None, Some(id), 10_000).unwrap()["selected_stream"]["entries"]
            [0]["id"],
        entries[0]["id"]
    );
    assert!(
        snapshot(
            root.path(),
            &run,
            &state,
            None,
            Some("../../secret"),
            10_000
        )
        .is_err()
    );
    assert!(snapshot(root.path(), "../secret", &state, None, None, 10_000).is_err());
}
#[test]
fn limits_tail_and_entries_without_exposing_invalid_json() {
    let (root, run, state, hash) = setup();
    let output = root.path().join(&run).join("host/output");
    let data = format!("{}{{\"prompt\":\"secret\"\n", "visible line\n".repeat(8000));
    fs::write(output.join(format!("{hash}.out")), data).unwrap();
    let value = snapshot(root.path(), &run, &state, None, None, 10_000).unwrap();
    assert_eq!(value["selected_stream"]["truncated"], true);
    assert_eq!(
        value["selected_stream"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        128
    );
    assert!(!value.to_string().contains("secret"));
}

#[test]
fn parses_actual_antigravity_and_multiple_provider_content_items() {
    let (root, run, state, hash) = setup();
    let output = root.path().join(&run).join("host/output");
    fs::write(output.join(format!("{hash}.out")), concat!(
        "{\"event\":\"step_update\",\"step_update\":{\"step_type\":\"agent_response\",\"text_delta\":\"AGY delta\"}}\n",
        "{\"event\":\"step_update\",\"step_update\":{\"step_type\":\"thinking\",\"text_delta\":\"hidden reasoning\"}}\n",
        "{\"event\":\"result\",\"result\":{\"response\":\"AGY done\"}}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test\",\"aggregated_output\":\"test passed\"}}\n",
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"first\"},{\"type\":\"thinking\",\"thinking\":\"secret\"},{\"type\":\"tool_use\",\"name\":\"Read\"},{\"type\":\"text\",\"text\":\"last\"}]}}\n"
    )).unwrap();
    let value = snapshot(root.path(), &run, &state, None, None, 10_000).unwrap();
    let entries = value["selected_stream"]["entries"].as_array().unwrap();
    let texts: Vec<_> = entries
        .iter()
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        [
            "AGY delta",
            "AGY done",
            "Command: cargo test",
            "test passed",
            "first",
            "Tool: Read",
            "last"
        ]
    );
    assert_ne!(entries[2]["id"], entries[3]["id"]);
    assert!(!value.to_string().contains("hidden reasoning"));
    assert!(!value.to_string().contains("secret"));
}
