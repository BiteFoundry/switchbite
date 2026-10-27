#![cfg(windows)]
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn git(repo: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap().trim().to_owned()
}
fn http(
    port: u16,
    method: &str,
    path: &str,
    token: &str,
    origin: &str,
    body: &str,
) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nX-Switchbite-Session: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        body.to_owned(),
    )
}
fn make_fake(dir: &Path) -> PathBuf {
    let source = dir.join("fake.rs");
    fs::write(&source, r###"use std::{env,fs,io::Write};
fn main() {
 let args:Vec<String>=env::args().collect();
 if args.iter().any(|s|s=="--version") { println!("codex fixture 1.0"); return; }
 assert!(env::var_os("CODEX_HOME").is_some(),"native CLI auth directory was not preserved");
 let mut input=String::new();
 let prompt=if args.last().map(String::as_str)==Some("-") { use std::io::Read; std::io::stdin().read_to_string(&mut input).unwrap(); &input } else { args.last().unwrap() };
 let log=env::var("FAKE_LOG").unwrap();
 writeln!(fs::OpenOptions::new().create(true).append(true).open(&log).unwrap(),"launch").unwrap();
 println!("{{\"type\":\"thread.started\",\"thread_id\":\"fixture\"}}"); println!("{{\"type\":\"turn.started\"}}");
 if prompt.contains("switchbite-observation.json") {
  fs::write("switchbite-observation.json",r#"{"plan":{"nodes":[{"id":"build","objective":"Create result","role":"implementation","outputs":["result.txt"],"write_paths":["result.txt"],"acceptance":["result exists"],"routing":{"provider":"codex","model":"fake-build","effort":"high"}},{"id":"review","objective":"Review result and write report","role":"review","depends_on":["build"],"outputs":["review.md"],"write_paths":["review.md"],"acceptance":["report exists"],"routing":{"provider":"codex","model":"fake-review","effort":"high"}}]}}"#).unwrap();
 } else if args.iter().any(|s|s=="fake-build") {
  let counter=std::path::Path::new(&log).with_extension("attempts");
  let first=!counter.exists();fs::write(counter,"started").unwrap();
  if first {
   println!(r#"{{"type":"item.completed","item":{{"type":"agent_message","text":"Compiling workspace password=hunter22"}}}}"#);
   eprintln!("Permission required: approve this command");std::io::stdout().flush().unwrap();
   std::thread::sleep(std::time::Duration::from_secs(10));
  }
  fs::write("result.txt","built").unwrap();
 }
 else if args.iter().any(|s|s=="fake-review") { assert_eq!(fs::read_to_string("result.txt").unwrap(),"built"); fs::write("review.md","reviewed").unwrap(); }
 else { panic!("unexpected model"); }
 println!(r#"{{"type":"item.completed","item":{{"type":"agent_message","text":"Worker finished"}}}}"#);
 println!("{{\"type\":\"turn.completed\"}}");
}"###).unwrap();
    let exe = dir.join("codex.exe");
    assert!(
        Command::new("rustc")
            .arg(source)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );
    exe
}
#[test]
fn web_submission_runs_real_workflow_with_isolated_fake_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("project");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@localhost"]);
    fs::write(repo.join("original.txt"), "unchanged").unwrap();
    git(&repo, &["add", "original.txt"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    make_fake(&bin);
    let home = tmp.path().join("home");
    let codex = home.join("codex");
    let appdata = home.join("appdata");
    let local = home.join("local");
    for p in [&home, &codex, &appdata, &local] {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(codex.join("models_cache.json"),json!({"models":[{"slug":"fake-build","visibility":"list","supported_reasoning_levels":[{"effort":"high"}]},{"slug":"fake-review","visibility":"list","supported_reasoning_levels":[{"effort":"high"}]}]}).to_string()).unwrap();
    let git_dir = env::split_paths(&env::var_os("PATH").unwrap())
        .find(|p| p.join("git.exe").is_file())
        .expect("git on PATH");
    let system = PathBuf::from(env::var_os("SystemRoot").unwrap());
    let path = env::join_paths([
        bin.clone(),
        git_dir,
        system.join("System32"),
        system.join("System32/WindowsPowerShell/v1.0"),
    ])
    .unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let log = tmp.path().join("calls.txt");
    let mut command = Command::new(env!("CARGO_BIN_EXE_switchbite"));
    command
        .args(["workflow", "app", "--port", &port.to_string(), "--offline"])
        .current_dir(&repo)
        .env_clear()
        .env("PATH", path)
        .env("SystemRoot", &system)
        .env("WINDIR", &system)
        .env("TEMP", tmp.path())
        .env("TMP", tmp.path())
        .env("USERPROFILE", &home)
        .env("HOME", &home)
        .env("CODEX_HOME", &codex)
        .env("APPDATA", &appdata)
        .env("LOCALAPPDATA", &local)
        .env("FAKE_LOG", &log)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut server = Server(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline && server.0.try_wait().unwrap().is_none(),
            "server did not start"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let origin = format!("http://127.0.0.1:{port}");
    let (_, html) = http(port, "GET", "/", "", "", "");
    let token = html
        .split("const token='")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    let (_, info) = http(port, "GET", "/api/app", "", "", "");
    let info: Value = serde_json::from_str(&info).unwrap();
    assert_eq!(http(port, "GET", "/api/activity", "", "", "").0, 403);
    assert_eq!(info["tools"].as_array().unwrap().len(), 6);
    assert_eq!(info["connection"]["mode"], "native_cli_new_session");
    let coordinator = info["coordinators"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["model"] == "fake-build" && c["effort"] == "high")
        .expect("fixture candidate")["id"]
        .clone();
    let body=json!({"workspace":repo,"goal":"Create a result and independent review","coordinator_id":coordinator,"tools":["codex"]}).to_string();
    assert_eq!(
        http(
            port,
            "POST",
            "/api/tools",
            token,
            "http://evil.invalid",
            r#"{"tools":["codex","claude"]}"#
        )
        .0,
        403
    );
    assert_eq!(
        http(
            port,
            "POST",
            "/api/tools",
            token,
            &origin,
            r#"{"tools":["unknown"]}"#
        )
        .0,
        400
    );
    assert_eq!(
        http(
            port,
            "POST",
            "/api/tools",
            token,
            &origin,
            r#"{"tools":[]}"#
        )
        .0,
        200
    );
    let no_tools =
        json!({"workspace":repo,"goal":"goal","coordinator_id":coordinator,"tools":[]}).to_string();
    assert_eq!(
        http(port, "POST", "/api/start", token, &origin, &no_tools).0,
        400
    );
    assert_eq!(
        http(
            port,
            "POST",
            "/api/tools",
            token,
            &origin,
            r#"{"tools":["codex","claude"]}"#
        )
        .0,
        200
    );
    let unchecked_coordinator =
        json!({"workspace":repo,"goal":"goal","coordinator_id":coordinator,"tools":["claude"]})
            .to_string();
    assert_eq!(
        http(
            port,
            "POST",
            "/api/start",
            token,
            &origin,
            &unchecked_coordinator
        )
        .0,
        400
    );
    let (status, refreshed) = http(port, "POST", "/api/tools/refresh", token, &origin, "{}");
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&refreshed).unwrap()["selected_tools"],
        json!(["codex", "claude"])
    );
    assert_eq!(
        http(
            port,
            "POST",
            "/api/start",
            token,
            "http://evil.invalid",
            &body
        )
        .0,
        403
    );
    let missing =
        json!({"workspace":tmp.path().join("missing"),"goal":"goal","coordinator_id":coordinator})
            .to_string();
    assert_eq!(
        http(port, "POST", "/api/start", token, &origin, &missing).0,
        400
    );
    let nongit = tmp.path().join("nongit");
    fs::create_dir(&nongit).unwrap();
    let bad = json!({"workspace":nongit,"goal":"goal","coordinator_id":coordinator}).to_string();
    assert_eq!(
        http(port, "POST", "/api/start", token, &origin, &bad).0,
        400
    );
    for invalid in [json!(0), json!(17), json!(1.5), json!("3"), Value::Null] {
        let invalid_body = json!({"workspace":repo,"goal":"goal","coordinator_id":coordinator,"tools":["codex"],"max_parallel":invalid}).to_string();
        assert_eq!(
            http(port, "POST", "/api/start", token, &origin, &invalid_body).0,
            400
        );
    }
    assert!(!log.exists());
    let (status, started) = http(port, "POST", "/api/start", token, &origin, &body);
    assert_eq!(status, 202);
    assert_eq!(
        serde_json::from_str::<Value>(&started).unwrap()["max_parallel"],
        3
    );
    assert_eq!(
        http(
            port,
            "POST",
            "/api/parallelism",
            token,
            &origin,
            r#"{"max_parallel":0}"#
        )
        .0,
        400
    );
    assert_eq!(
        http(
            port,
            "POST",
            "/api/parallelism",
            token,
            "http://evil.invalid",
            r#"{"max_parallel":3}"#
        )
        .0,
        403
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut retried = false;
    loop {
        let (_, body) = http(port, "GET", "/api/state", "", "", "");
        let state: Value = serde_json::from_str(&body).unwrap();
        if state["status"] == "completed" {
            assert!(
                retried,
                "expected live output intervention before completion"
            );
            assert_eq!(state["tasks"][0]["repair_rounds"], 1);
            let (code, body) = http(port, "GET", "/api/activity", token, &origin, "");
            assert_eq!(code, 200);
            let activity: Value = serde_json::from_str(&body).unwrap();
            assert!(
                activity["streams"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|stream| stream["stage"] == "worker" && stream["task_id"] == "review")
            );
            assert_eq!(state["tools"], json!(["codex"]));
            let stored = switchbite::workflow::Workflow::new(repo.join(".switchbite/workflow"))
                .request(state["run_id"].as_str().unwrap())
                .unwrap();
            assert_eq!(stored["catalog"]["providers"], json!(["codex"]));
            assert_eq!(stored["max_parallel"], 4);
            assert_eq!(state["progress"]["done"], 2);
            assert_eq!(state["progress"]["total"], 2);
            let result = Path::new(state["result"]["workspace"].as_str().unwrap());
            assert_eq!(
                state["result"]["commit"].as_str().unwrap(),
                git(result, &["rev-parse", "HEAD"]).trim()
            );
            assert_eq!(
                fs::read_to_string(result.join("result.txt")).unwrap(),
                "built"
            );
            assert_eq!(
                fs::read_to_string(result.join("review.md")).unwrap(),
                "reviewed"
            );
            break;
        }
        if !retried {
            let (code, body) = http(port, "GET", "/api/activity", token, &origin, "");
            assert_eq!(code, 200, "{body}");
            let activity: Value = serde_json::from_str(&body).unwrap();
            if activity["selected_stream"]["task_id"] == "build"
                && activity["selected_stream"]["health"] == "needs_input"
            {
                assert_eq!(activity["selected_stream"]["process_alive"], true);
                assert_eq!(activity["selected_stream"]["can_retry"], true);
                assert!(body.contains("Compiling workspace"));
                assert!(body.contains("REDACTED") && !body.contains("hunter22"));
                let selected = activity["selected_stream"]["id"].as_str().unwrap();
                let task_ids: Vec<_> = state["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|task| task["id"].clone())
                    .collect();
                let (code, paused) = http(
                    port,
                    "POST",
                    "/api/control",
                    token,
                    &origin,
                    r#"{"action":"pause"}"#,
                );
                assert_eq!(code, 200, "{paused}");
                let (code, changed) = http(
                    port,
                    "POST",
                    "/api/parallelism",
                    token,
                    &origin,
                    r#"{"max_parallel":4}"#,
                );
                assert_eq!(code, 200, "{changed}");
                let changed: Value = serde_json::from_str(&changed).unwrap();
                assert_eq!(changed["max_parallel"], 4);
                assert_eq!(changed["run_id"], state["run_id"]);
                assert_eq!(
                    changed["tasks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|task| task["id"].clone())
                        .collect::<Vec<_>>(),
                    task_ids
                );
                let (code, selected_output) = http(
                    port,
                    "GET",
                    &format!("/api/activity?stream={selected}"),
                    token,
                    &origin,
                    "",
                );
                assert_eq!(code, 200, "{selected_output}");
                assert_eq!(
                    serde_json::from_str::<Value>(&selected_output).unwrap()["selected_stream"]["id"],
                    selected
                );
                let (code, resumed) = http(
                    port,
                    "POST",
                    "/api/control",
                    token,
                    &origin,
                    r#"{"action":"resume"}"#,
                );
                assert_eq!(code, 200, "{resumed}");
                assert_eq!(
                    serde_json::from_str::<Value>(&resumed).unwrap()["max_parallel"],
                    4
                );
                assert_eq!(
                    http(
                        port,
                        "POST",
                        "/api/task/retry",
                        token,
                        "http://evil.invalid",
                        r#"{"task_id":"build"}"#
                    )
                    .0,
                    403
                );
                let (code, body) = http(
                    port,
                    "POST",
                    "/api/task/retry",
                    token,
                    &origin,
                    r#"{"task_id":"build"}"#,
                );
                assert_eq!(code, 200, "{body}");
                retried = true;
            }
        }
        assert!(
            Instant::now() < deadline && state.get("attention").is_none(),
            "workflow did not complete: {state}"
        );
        thread::sleep(Duration::from_millis(250));
    }
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(repo.join("original.txt")).unwrap(),
        "unchanged"
    );
    assert!(!repo.join("result.txt").exists());
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    server = Server(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline && server.0.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(50));
    }
    let (_, info) = http(port, "GET", "/api/app", "", "", "");
    assert_eq!(
        serde_json::from_str::<Value>(&info).unwrap()["selected_tools"],
        json!(["codex", "claude"])
    );
}

fn make_route_review_fake(dir: &Path) {
    let source = dir.join("route_fake.rs");
    fs::write(&source, r###"use std::{env,fs,io::Write};
fn main() {
 let args:Vec<String>=env::args().collect();
 if args.iter().any(|s|s=="--version") { println!("codex fixture 1.0"); return; }
 assert!(env::var_os("CODEX_HOME").is_some());
 let mut input=String::new();
 let prompt=if args.last().map(String::as_str)==Some("-") { use std::io::Read; std::io::stdin().read_to_string(&mut input).unwrap(); &input } else { args.last().unwrap() };
 let log=env::var("FAKE_LOG").unwrap();
 let mut out=fs::OpenOptions::new().create(true).append(true).open(log).unwrap();
 println!("{{\"type\":\"thread.started\",\"thread_id\":\"fixture\"}}");
 println!("{{\"type\":\"turn.started\"}}");
 if prompt.contains("switchbite-observation.json") && prompt.contains("\"kind\":\"review_route\"") {
  let snapshot=prompt.split("\"candidate_snapshot\":[").nth(1).expect("snapshot");
  let build=snapshot.find("\"model\":\"fake-build\"").expect("build candidate");
  let before=&snapshot[..build];
  let id=before.rsplit("\"id\":\"").next().unwrap().split('"').next().unwrap();
  writeln!(out,"review_route:{id}").unwrap();
  fs::write("switchbite-observation.json",format!(r#"{{"status":"ready","candidate_id":"{}","reason_codes":["coordinator_review"],"summary":"Eligible candidate"}}"#,id)).unwrap();
 } else if prompt.contains("switchbite-observation.json") {
  writeln!(out,"plan").unwrap();
  fs::write("switchbite-observation.json",r#"{"plan":{"nodes":[{"id":"build","objective":"Create result","role":"implementation","outputs":["result.txt"],"write_paths":["result.txt"],"acceptance":["result exists"]},{"id":"review","objective":"Review result","role":"review","depends_on":["build"],"outputs":["review.md"],"write_paths":["review.md"],"acceptance":["report exists"],"routing":{"provider":"codex","model":"fake-review","effort":"high"}}]}}"#).unwrap();
 } else if args.iter().any(|s|s=="fake-build") {
  writeln!(out,"build").unwrap(); fs::write("result.txt","built").unwrap();
 } else if args.iter().any(|s|s=="fake-review") {
  writeln!(out,"review").unwrap(); assert_eq!(fs::read_to_string("result.txt").unwrap(),"built"); fs::write("review.md","reviewed").unwrap();
 } else { panic!("unexpected model"); }
 println!("{{\"type\":\"turn.completed\"}}");
}"###).unwrap();
    assert!(
        Command::new("rustc")
            .arg(source)
            .arg("-o")
            .arg(dir.join("codex.exe"))
            .status()
            .unwrap()
            .success()
    );
}

fn low_confidence_judge() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let mut bytes = Vec::new();
        let (body_start, content_length) = loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..pos]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (pos + 4, length);
            }
        };
        while bytes.len() < body_start + content_length {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&chunk[..n]);
        }
        let request: Value =
            serde_json::from_slice(&bytes[body_start..body_start + content_length]).unwrap();
        let mut answers = serde_json::Map::new();
        for (key, question) in request["questions"].as_object().unwrap() {
            let answer = if question["type"] == "noul" {
                json!({"type":"noul","noul":1.0})
            } else if key.starts_with("fit_") {
                json!({"type":"score","score":1.5,"probabilities":{"0":0.25,"1":0.25,"2":0.25,"3":0.25}})
            } else {
                json!({"type":"score","score":1.0,"probabilities":{"0":0.0,"1":1.0,"2":0.0}})
            };
            answers.insert(key.clone(), answer);
        }
        let body = json!({"model":"laya:fixture","answers":answers}).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    (port, handle)
}

#[test]
fn held_route_is_reviewed_by_native_coordinator_and_workflow_completes() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("project");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@localhost"]);
    git(&repo, &["commit", "--allow-empty", "-q", "-m", "base"]);
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    make_route_review_fake(&bin);
    let home = tmp.path().join("home");
    let codex = home.join("codex");
    let appdata = home.join("appdata");
    let local = home.join("local");
    for p in [&home, &codex, &appdata, &local] {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(codex.join("models_cache.json"),json!({"models":[{"slug":"fake-build","visibility":"list","supported_reasoning_levels":[{"effort":"high"}]},{"slug":"fake-review","visibility":"list","supported_reasoning_levels":[{"effort":"high"}]}]}).to_string()).unwrap();
    let git_dir = env::split_paths(&env::var_os("PATH").unwrap())
        .find(|p| p.join("git.exe").is_file())
        .unwrap();
    let system = PathBuf::from(env::var_os("SystemRoot").unwrap());
    let path = env::join_paths([
        bin,
        git_dir,
        system.join("System32"),
        system.join("System32/WindowsPowerShell/v1.0"),
    ])
    .unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (judge_port, judge) = low_confidence_judge();
    let log = tmp.path().join("calls.txt");
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_switchbite"))
            .args([
                "workflow",
                "app",
                "--port",
                &port.to_string(),
                "--judge",
                "laya",
                "--endpoint",
                &format!("http://127.0.0.1:{judge_port}"),
                "--model",
                "laya:fixture",
            ])
            .current_dir(&repo)
            .env_clear()
            .env("PATH", path)
            .env("SystemRoot", &system)
            .env("WINDIR", &system)
            .env("TEMP", tmp.path())
            .env("TMP", tmp.path())
            .env("USERPROFILE", &home)
            .env("HOME", &home)
            .env("CODEX_HOME", &codex)
            .env("APPDATA", &appdata)
            .env("LOCALAPPDATA", &local)
            .env("FAKE_LOG", &log)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline && server.0.try_wait().unwrap().is_none(),
            "server did not start"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let origin = format!("http://127.0.0.1:{port}");
    let (_, html) = http(port, "GET", "/", "", "", "");
    let token = html
        .split("const token='")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    let (_, info) = http(port, "GET", "/api/app", "", "", "");
    let info: Value = serde_json::from_str(&info).unwrap();
    let coordinator = info["coordinators"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["model"] == "fake-build" && c["effort"] == "high")
        .unwrap()["id"]
        .clone();
    let body = json!({"workspace":repo,"goal":"Create a result and independent review","coordinator_id":coordinator,"tools":["codex"]}).to_string();
    assert_eq!(
        http(port, "POST", "/api/start", token, &origin, &body).0,
        202
    );
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let (_, body) = http(port, "GET", "/api/state", "", "", "");
        let state: Value = serde_json::from_str(&body).unwrap();
        if state["status"] == "completed" {
            assert_eq!(state["progress"]["done"], 2);
            assert_eq!(state["tools"], json!(["codex"]));
            let stored = switchbite::workflow::Workflow::new(repo.join(".switchbite/workflow"))
                .request(state["run_id"].as_str().unwrap())
                .unwrap();
            assert_eq!(stored["catalog"]["providers"], json!(["codex"]));
            let result = Path::new(state["result"]["workspace"].as_str().unwrap());
            assert_eq!(
                fs::read_to_string(result.join("result.txt")).unwrap(),
                "built"
            );
            assert_eq!(
                fs::read_to_string(result.join("review.md")).unwrap(),
                "reviewed"
            );
            break;
        }
        assert!(
            Instant::now() < deadline && state.get("attention").is_none(),
            "workflow did not complete: {state}"
        );
        thread::sleep(Duration::from_millis(250));
    }
    judge.join().unwrap();
    let calls = fs::read_to_string(log).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("review_route:"))
            .count(),
        1,
        "{calls}"
    );
    assert_eq!(
        calls.lines().filter(|line| *line == "build").count(),
        1,
        "{calls}"
    );
    assert_eq!(
        calls.lines().filter(|line| *line == "review").count(),
        1,
        "{calls}"
    );
}
