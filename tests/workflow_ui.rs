use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use switchbite::{workflow::Workflow, workflow_ui};

fn request(port: u16, message: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(message.as_bytes()).unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}
#[test]
fn viewer_restricts_origin_and_session_and_only_exposes_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let flow = Workflow::new(root.join(".switchbite/workflow"));
    let state=flow.start(&json!({"workspace":root.to_str().unwrap(),"goal":"Build","pool":[],"host":{"secret":"secret-value"}})).unwrap();
    let run = state["run_id"].as_str().unwrap().to_string();
    let socket = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let ws = root.clone();
    thread::spawn(move || {
        let _ = workflow_ui::serve(&ws, &run, port);
    });
    let mut connected = false;
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            connected = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(connected);
    let host = format!("127.0.0.1:{port}");
    let page = request(port, &format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n"));
    assert!(page.starts_with("HTTP/1.1 200"));
    let token = page
        .split("const token='")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    assert_eq!(token.len(), 64);
    let state = request(
        port,
        &format!("GET /api/state HTTP/1.1\r\nHost: {host}\r\n\r\n"),
    );
    assert!(!state.contains("secret-value"));
    let body = state.split("\r\n\r\n").nth(1).unwrap();
    let value: Value = serde_json::from_str(body).unwrap();
    assert_eq!(value["status"], "running");
    std::fs::write(root.join(".switchbite/workflow").join(value["run_id"].as_str().unwrap()).join("web-session.json"),
        serde_json::to_vec(&json!({"coordinator":{"status":"needs_attention","error":{"code":"invalid_receipt","message":"Repair plan needs attention"}},"trace":{}})).unwrap()).unwrap();
    let updated = request(
        port,
        &format!("GET /api/state HTTP/1.1\r\nHost: {host}\r\n\r\n"),
    );
    let updated: Value = serde_json::from_str(updated.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(updated["attention"]["code"], "invalid_receipt");
    let bad_host = request(
        port,
        "GET /api/state HTTP/1.1\r\nHost: attacker.example\r\n\r\n",
    );
    assert!(bad_host.starts_with("HTTP/1.1 403"));
    let body = r#"{"action":"pause"}"#;
    let send = |origin: &str, key: &str| {
        request(
            port,
            &format!(
                "POST /api/control HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nX-Switchbite-Session: {key}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    };
    assert!(send("http://attacker.example", token).starts_with("HTTP/1.1 403"));
    assert!(send(&format!("http://{host}"), "wrong").starts_with("HTTP/1.1 403"));
    assert!(send(&format!("http://{host}"), token).starts_with("HTTP/1.1 200"));
    let after = request(
        port,
        &format!("GET /api/state HTTP/1.1\r\nHost: {host}\r\n\r\n"),
    );
    assert!(after.contains("\"status\":\"paused\""));
}
