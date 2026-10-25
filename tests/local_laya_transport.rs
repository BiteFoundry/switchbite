use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};
use switchbite::local_laya::{DEFAULT_MODEL, LayaConfig, LocalLayaJudge};

fn fixture(
    status: &str,
    body: Value,
) -> (
    LocalLayaJudge,
    mpsc::Receiver<String>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let status = status.to_owned();
    let body = body.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let raw = read_request(&mut stream);
        tx.send(raw).unwrap();
        write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nLocation: http://192.0.2.1/private\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    });
    (
        LocalLayaJudge::new(LayaConfig::new(&endpoint, DEFAULT_MODEL).unwrap()),
        rx,
        handle,
    )
}
fn read_request(stream: &mut TcpStream) -> String {
    let mut raw = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        raw.extend_from_slice(&buf[..n]);
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&raw[..end]);
            let len = header
                .lines()
                .find_map(|l| {
                    l.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if raw.len() >= end + 4 + len {
                break;
            }
        }
        assert!(raw.len() < 131072);
    }
    String::from_utf8(raw).unwrap()
}
fn request() -> Value {
    json!({"model":DEFAULT_MODEL,"state":{"task":"Create an HTML file"},"questions":{"fit":{"type":"score","instructions":"How well does the worker fit?","criteria":["unsuitable","acceptable","strong"]}}})
}
#[test]
fn local_http_has_no_auth_and_preserves_model_while_normalizing_concentration() {
    let (judge, rx, handle) = fixture(
        "200 OK",
        json!({"model":DEFAULT_MODEL,"answers":{"fit":{"type":"score","score":1.3,"confidence":0.25,"probabilities":{"0":0.2,"1":0.3,"2":0.5}}},"usage":{"input_tokens":25,"output_tokens":0}}),
    );
    let result = judge.judge_detailed(&request()).unwrap();
    assert_eq!(result["model"], DEFAULT_MODEL);
    assert_eq!(result["answers"]["fit"]["confidence"], 0.0);
    let raw = rx.recv().unwrap();
    assert!(raw.starts_with("POST /v1/systemone HTTP/1.1"));
    assert!(!raw.to_ascii_lowercase().contains("authorization:"));
    assert!(!raw.contains("TYPESAFE_API_KEY"));
    let sent: Value = serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(sent, request());
    handle.join().unwrap();
}
#[test]
fn unavailable_local_server_never_falls_back_to_cloud() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let judge =
        LocalLayaJudge::new(LayaConfig::new(&format!("http://{address}"), DEFAULT_MODEL).unwrap());
    // Windows may time out instead of returning immediate ECONNREFUSED.
    assert!(matches!(
        judge.judge_detailed(&request()).unwrap_err().code,
        "local_judge_unavailable" | "provider_timeout"
    ));
}
#[test]
fn redirects_truncation_and_foreign_models_do_not_produce_judgments() {
    for (status, body, expected) in [
        ("302 Found", json!({}), "provider_invalid_response"),
        (
            "422 Unprocessable Entity",
            json!({"code":"STATE_TRUNCATED","error":"private upstream text"}),
            "judge_context_too_long",
        ),
        (
            "200 OK",
            json!({"model":"jev-1.14.0","answers":{}}),
            "provider_invalid_response",
        ),
        (
            "200 OK",
            json!({"model":DEFAULT_MODEL,"answers":{},"state_truncated":true}),
            "judge_context_too_long",
        ),
    ] {
        let (judge, rx, handle) = fixture(status, body);
        let error = judge.judge_detailed(&request()).unwrap_err();
        assert_eq!(error.code, expected);
        assert!(!error.to_value().to_string().contains("private upstream"));
        rx.recv().unwrap();
        handle.join().unwrap();
    }
}
