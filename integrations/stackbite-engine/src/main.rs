//! SwitchBite-owned process adapter over an unchanged StackBite provider-stream dependency.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use stackbite_provider_stream::transport::Stream;
use stackbite_provider_stream::{
    ChildTransport, Chunk, ExecutionBinding, Frame, FrameLimits, NdjsonFramer, Transport,
};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const MAX_PROMPT: usize = 16 * 1024;
const MAX_REQUEST: usize = 128 * 1024;
const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
const MAX_SEATS: usize = 256;
const MAX_ACTIVE: usize = 8;
const MODE: &str = "structured_one_shot";
type Result<T> = std::result::Result<T, &'static str>;

fn random_hex(n: usize) -> Result<String> {
    let mut b = vec![0; n];
    getrandom::fill(&mut b).map_err(|_| "entropy_unavailable")?;
    Ok(b.iter().map(|b| format!("{b:02x}")).collect())
}
fn uuid() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| "entropy_unavailable")?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    let h: String = b.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}
fn digest(prompt: &str) -> String {
    format!("{:x}", Sha256::digest(prompt.as_bytes()))
}
fn safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}
fn error(code: &str) -> Value {
    json!({"ok":false,"error":code})
}
fn absolute_existing(s: &str, dir: bool) -> Result<PathBuf> {
    let p = Path::new(s);
    if !p.is_absolute() {
        return Err("absolute_path_required");
    }
    let p = p.canonicalize().map_err(|_| "path_unavailable")?;
    if (dir && !p.is_dir()) || (!dir && !p.is_file()) {
        return Err("wrong_path_kind");
    }
    Ok(p)
}
fn create_json(path: &Path, value: &Value) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "state_claim_failed")?;
    serde_json::to_writer(&mut f, value).map_err(|_| "state_write_failed")?;
    f.write_all(b"\n")
        .and_then(|_| f.sync_all())
        .map_err(|_| "state_write_failed")
}
fn read_capped(path: &Path, max: usize) -> Result<Vec<u8>> {
    let f = File::open(path).map_err(|_| "file_unavailable")?;
    let mut b = Vec::new();
    f.take(max as u64 + 1)
        .read_to_end(&mut b)
        .map_err(|_| "file_read_failed")?;
    if b.len() > max {
        return Err("file_too_large");
    }
    Ok(b)
}
fn read_line_capped(stream: &mut TcpStream) -> Result<Value> {
    let mut b = Vec::new();
    BufReader::new(stream)
        .take(MAX_REQUEST as u64 + 1)
        .read_until(b'\n', &mut b)
        .map_err(|_| "request_read_failed")?;
    if b.len() > MAX_REQUEST || b.last() != Some(&b'\n') {
        return Err("request_limit");
    }
    serde_json::from_slice(&b).map_err(|_| "invalid_json")
}
fn constant_eq(a: &str, b: &str) -> bool {
    let mut d = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        d |= usize::from(
            a.as_bytes().get(i).copied().unwrap_or(0) ^ b.as_bytes().get(i).copied().unwrap_or(0),
        );
    }
    d == 0
}

#[derive(Clone, Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
    room_id: String,
    seat_id: String,
    runtime_mode: String,
}
#[derive(Clone)]
struct Seat {
    binding: ExecutionBinding,
    start_signature: Value,
    model: String,
    effort: String,
    send_request: Option<String>,
    prompt: Option<String>,
    prompt_digest: Option<String>,
    session_id: Option<String>,
    receipt: Value,
}
impl Seat {
    fn view(&self) -> Value {
        json!({"seat_key":self.binding.seat_key,"generation":self.binding.generation,"provider":"claude","model":self.model,"effort":self.effort,"runtime_mode":MODE,"acknowledgement_kind":"backend_ready","backend_ready":true,"onboarding_acknowledged":false,"receipt":self.receipt})
    }
    fn launch(&self) -> Value {
        json!({"ok":true,"seat_key":self.binding.seat_key,"generation":self.binding.generation,"runtime_mode":MODE,"acknowledgement_kind":"backend_ready","backend_ready":true,"onboarding_acknowledged":false})
    }
    fn send_ack(&self) -> Value {
        json!({"ok":true,"seat_key":self.binding.seat_key,"generation":self.binding.generation,"turn":"1","request_id":self.send_request,"prompt_sha256":self.prompt_digest,"session_id":self.session_id,"runtime_mode":MODE,"accepted":true})
    }
}
struct State {
    endpoint: Endpoint,
    state_dir: PathBuf,
    cwd: PathBuf,
    claude: PathBuf,
    seats: BTreeMap<String, Seat>,
    starts: BTreeMap<String, String>,
    sends: BTreeMap<String, String>,
    active: usize,
}
type Shared = Arc<Mutex<State>>;

fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key).and_then(Value::as_str).ok_or("missing_argument")
}
fn dispatch(shared: &Shared, v: &Value) -> Result<Value> {
    let mut s = shared.lock().map_err(|_| "state_unavailable")?;
    match text(v, "command")? {
        "context" => Ok(
            json!({"ok":true,"room_id":s.endpoint.room_id,"seat_id":s.endpoint.seat_id,"process_alive":true,"runtime_mode":MODE,"isolated_clone":true,"cwd":s.cwd,"available_commands":["agent context","agent seats","agent start","agent launch-status","agent send","agent wait"],"api_authentication":"local private endpoint token; never supplied to provider"}),
        ),
        "seats" => Ok(
            json!({"ok":true,"room_id":s.endpoint.room_id,"runtime_mode":MODE,"seats":s.seats.values().map(Seat::view).collect::<Vec<_>>()}),
        ),
        "start" => {
            let rid = text(v, "request_id")?;
            if !safe_id(rid) || text(v, "provider")? != "claude" {
                return Err("invalid_start");
            }
            if let Some(seat) = s.starts.get(rid) {
                let old = &s.seats[seat];
                return if old.start_signature == v["args"] {
                    Ok(old.launch())
                } else {
                    Err("request_id_conflict")
                };
            }
            if s.sends.contains_key(rid) {
                return Err("request_id_conflict");
            }
            if s.seats.len() >= MAX_SEATS {
                return Err("seat_limit");
            }
            if absolute_existing(text(v, "cwd")?, true)? != s.cwd {
                return Err("cwd_mismatch");
            }
            if text(v, "report_to")? != s.endpoint.seat_id {
                return Err("coordinator_mismatch");
            }
            let model = text(v, "model")?;
            if !safe_id(model) {
                return Err("invalid_model");
            }
            let effort = text(v, "effort")?;
            if !["low", "medium", "high", "xhigh", "max"].contains(&effort) {
                return Err("invalid_effort");
            }
            let key = format!("executor-{}", random_hex(8)?);
            let binding = ExecutionBinding::new(&s.endpoint.room_id, &key, &uuid()?, rid)
                .map_err(|_| "invalid_binding")?;
            let seat = Seat {
                binding,
                start_signature: v["args"].clone(),
                model: model.into(),
                effort: effort.into(),
                send_request: None,
                prompt: None,
                prompt_digest: None,
                session_id: None,
                receipt: json!({"state":"reserved","completion_verified":false,"runtime_mode":MODE}),
            };
            let launch = seat.launch();
            create_json(
                &s.state_dir.join(format!("{key}.launch.json")),
                &json!({"binding":seat.binding,"launch":launch,"model":model,"effort":effort,"cwd":s.cwd}),
            )?;
            s.starts.insert(rid.into(), key.clone());
            s.seats.insert(key, seat);
            Ok(launch)
        }
        "launch-status" => {
            let key = s
                .starts
                .get(text(v, "request_id")?)
                .ok_or("launch_unknown")?;
            Ok(s.seats[key].launch())
        }
        "send" => {
            let key = text(v, "seat")?;
            let rid = text(v, "request_id")?;
            let prompt = text(v, "prompt")?;
            if !safe_id(rid)
                || prompt.is_empty()
                || prompt.len() > MAX_PROMPT
                || prompt.contains('\0')
            {
                return Err("invalid_prompt_or_request");
            }
            let seat = s.seats.get(key).ok_or("seat_unknown")?;
            if let Some(old) = &seat.send_request {
                return if old == rid && seat.prompt.as_deref() == Some(prompt) {
                    Ok(seat.send_ack())
                } else {
                    Err("seat_already_sent")
                };
            }
            if s.starts.contains_key(rid) || s.sends.contains_key(rid) {
                return Err("request_id_conflict");
            }
            if s.active >= MAX_ACTIVE {
                return Err("execution_limit");
            }
            let session_id = uuid()?;
            let dg = digest(prompt);
            let binding = seat.binding.clone();
            let model = seat.model.clone();
            let effort = seat.effort.clone();
            create_json(
                &s.state_dir.join(format!("{key}.send.json")),
                &json!({"binding":binding,"request_id":rid,"turn":"1","prompt_sha256":dg,"session_id":session_id,"runtime_mode":MODE}),
            )?;
            let seat = s.seats.get_mut(key).unwrap();
            seat.send_request = Some(rid.into());
            seat.prompt = Some(prompt.into());
            seat.prompt_digest = Some(dg);
            seat.session_id = Some(session_id.clone());
            seat.receipt = json!({"seat_key":key,"generation":binding.generation,"turn":"1","state":"running","completion_verified":false,"session_id":session_id,"request_id":rid,"runtime_mode":MODE});
            let ack = seat.send_ack();
            let cwd = s.cwd.clone();
            let exe = s.claude.clone();
            s.sends.insert(rid.into(), key.into());
            s.active += 1;
            let owned = shared.clone();
            let key = key.to_owned();
            let prompt = prompt.to_owned();
            let rid = rid.to_owned();
            // Intent is durably claimed before spawning. This daemon never restores
            // a previous state root or retries an uncertain process delivery.
            thread::spawn(move || {
                let result = std::panic::catch_unwind(|| {
                    run_provider(
                        &exe,
                        &cwd,
                        &binding,
                        &rid,
                        &session_id,
                        (&model, &effort),
                        &prompt,
                    )
                });
                let receipt = match result {
                    Ok(r) => r,
                    Err(_) => failed_receipt(&binding, &rid, &session_id, "runner_panicked"),
                };
                if let Ok(mut s) = owned.lock() {
                    let persisted = create_json(
                        &s.state_dir.join(format!("{key}.completion.json")),
                        &receipt,
                    )
                    .is_ok();
                    if let Some(seat) = s.seats.get_mut(&key) {
                        seat.receipt = if persisted {
                            receipt
                        } else {
                            failed_receipt(&binding, &rid, &session_id, "completion_persist_failed")
                        };
                    }
                    s.active = s.active.saturating_sub(1);
                }
            });
            Ok(ack)
        }
        "wait" => {
            let seat = s.seats.get(text(v, "seat")?).ok_or("seat_unknown")?;
            if text(v, "generation")? != seat.binding.generation || text(v, "turn")? != "1" {
                return Err("receipt_mismatch");
            }
            if seat.send_request.is_none() {
                return Err("turn_unknown");
            }
            Ok(seat.receipt.clone())
        }
        _ => Err("command_unsupported"),
    }
}

/// Pure Claude validator: a successful result is evidence only after exact init,
/// exact result, successful owned exit and both pipe EOFs have all been observed.
#[derive(Default)]
struct Evidence {
    init: bool,
    resolved_model: Option<String>,
    result: bool,
    error: Option<&'static str>,
    events: u64,
    stdout_eof: bool,
    stderr_eof: bool,
    owned_exit: bool,
}
impl Evidence {
    fn fail(&mut self, code: &'static str) {
        if self.error.is_none() {
            self.error = Some(code);
        }
    }
    fn frame(&mut self, frame: Frame, expected: &str) {
        match frame {
            Frame::Line(line) => match serde_json::from_str::<Value>(&line) {
                Ok(v) => self.event(&v, expected),
                Err(_) => self.fail("invalid_provider_json"),
            },
            _ => self.fail("provider_framing_error"),
        }
    }
    fn event(&mut self, v: &Value, expected: &str) {
        self.events += 1;
        if let Some(id) = v.get("session_id") {
            if id.as_str() != Some(expected) {
                self.fail("foreign_session");
            }
        }
        match v.get("type").and_then(Value::as_str) {
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
                if self.init
                    || self.result
                    || v.get("session_id").and_then(Value::as_str) != Some(expected)
                {
                    self.fail("invalid_init");
                }
                self.init = true;
                self.resolved_model = v
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|m| safe_id(m))
                    .map(str::to_owned);
            }
            Some("result") => {
                if self.result {
                    self.fail("duplicate_result");
                }
                if !self.init {
                    self.fail("missing_init");
                }
                if v.get("session_id").and_then(Value::as_str) != Some(expected) {
                    self.fail("foreign_session");
                }
                if v.get("subtype").and_then(Value::as_str) != Some("success")
                    || v.get("is_error") != Some(&json!(false))
                {
                    self.fail("provider_result_error");
                }
                if !v
                    .get("permission_denials")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
                {
                    self.fail("permission_denied_or_unverified");
                }
                self.result = true;
            }
            Some("error") => self.fail("provider_error_event"),
            Some("assistant" | "user") => {
                if self.result {
                    self.fail("events_after_result");
                }
                if v.get("error").is_some_and(|e| !e.is_null()) {
                    self.fail("provider_error_event");
                }
                if v.get("message")
                    .and_then(|m| m.get("stop_reason"))
                    .and_then(Value::as_str)
                    == Some("refusal")
                {
                    self.fail("provider_refusal");
                }
                if v.get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(Value::as_array)
                    .is_some_and(|c| {
                        c.iter().any(|p| {
                            p.get("is_error") == Some(&json!(true))
                                || p.get("type").and_then(Value::as_str) == Some("refusal")
                        })
                    })
                {
                    self.fail("provider_tool_or_refusal_error");
                }
            }
            Some(_) => {}
            None => self.fail("provider_event_type_missing"),
        }
    }
    fn verified(&self) -> bool {
        self.init
            && self.result
            && self.error.is_none()
            && self.stdout_eof
            && self.stderr_eof
            && self.owned_exit
    }
    fn finish(&mut self) {
        if !self.init {
            self.fail("missing_init");
        }
        if !self.result {
            self.fail("missing_result");
        }
        if !self.owned_exit {
            self.fail("provider_exit_failed");
        }
        if !self.stdout_eof || !self.stderr_eof {
            self.fail("provider_eof_missing");
        }
    }
}
fn failed_receipt(b: &ExecutionBinding, rid: &str, session: &str, code: &str) -> Value {
    json!({"seat_key":b.seat_key,"generation":b.generation,"turn":"1","request_id":rid,"session_id":session,"state":"failed","success":false,"completion_verified":false,"reason":code,"runtime_mode":MODE})
}
fn provider_command(
    exe: &Path,
    cwd: &Path,
    b: &ExecutionBinding,
    rid: &str,
    session: &str,
    profile: (&str, &str),
    prompt: &str,
) -> Command {
    let (model, effort) = profile;
    let mut c = Command::new(exe);
    c.current_dir(cwd).args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--model",
        model,
        "--effort",
        effort,
        "--session-id",
        session,
        "--restricted",
        "--safe-mode",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--permission-mode",
        "acceptEdits",
        "--permission-prompts",
        "none",
        "--allowedTools",
        "Read",
        "Write",
        "Edit",
        "--tools",
        "Read,Write,Edit",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--settings",
        "{\"disableAllHooks\":true}",
    ]);
    let context = format!(
        "Stackbite는 현재 로컬에서 실행 중인 자체 멀티 에이전트 작업 앱이다. This invocation belongs only to a private isolated clone in structured_one_shot mode. Owned Room={}, Seat={}, generation={}, request={}, turn=1, cwd={}. There is no Stackbite/WorkBite CLI or authenticated execution API available to this restricted file-only worker. Read, Write and Edit are the only allowed tools. Perform exactly the supplied task within this cwd; never access installed app state or existing sessions.",
        b.room_id,
        b.seat_key,
        b.generation,
        rid,
        cwd.display()
    );
    c.args(["--append-system-prompt", &context, "--", prompt]);
    for (k, _) in env::vars_os() {
        let upper = k.to_string_lossy().to_ascii_uppercase();
        if [
            "STACKBITE_",
            "WORKBITE_",
            "WORKSTACK_",
            "SWITCHBITE_",
            "HELM_",
            "CLAUDECODE",
            "CLAUDE_CODE_",
        ]
        .iter()
        .any(|p| upper.starts_with(p))
        {
            c.env_remove(k);
        }
    }
    // Restricted tools provide no command execution. Also remove helper-rich
    // inherited PATH, retaining just the explicit provider and OS directory.
    let mut paths = vec![exe.parent().unwrap_or(cwd).to_path_buf()];
    if let Some(root) = env::var_os("SystemRoot") {
        paths.push(PathBuf::from(root).join("System32"));
    }
    if let Ok(path) = env::join_paths(paths) {
        c.env("PATH", path);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c
}
fn run_provider(
    exe: &Path,
    cwd: &Path,
    b: &ExecutionBinding,
    rid: &str,
    session: &str,
    profile: (&str, &str),
    prompt: &str,
) -> Value {
    let (model, effort) = profile;
    let mut t =
        match ChildTransport::spawn(provider_command(exe, cwd, b, rid, session, profile, prompt)) {
            Ok(t) => t,
            Err(_) => return failed_receipt(b, rid, session, "provider_spawn_failed"),
        };
    t.close_input();
    let mut e = Evidence::default();
    let mut framer = NdjsonFramer::new(FrameLimits {
        max_line_bytes: 1024 * 1024,
        max_total_bytes: MAX_OUTPUT,
    });
    let began = Instant::now();
    let mut bytes = 0u64;
    let mut exit = None;
    let mut saw_exit = false;
    loop {
        if began.elapsed() > Duration::from_secs(600) {
            e.fail("provider_timeout");
            let _ = t.cancel();
            break;
        }
        if let Some(chunk) = t.recv(Duration::from_millis(50)) {
            match chunk {
                Chunk::Data(stream, data) => {
                    bytes = bytes.saturating_add(data.len() as u64);
                    if bytes > MAX_OUTPUT {
                        e.fail("provider_output_limit");
                        let _ = t.cancel();
                        break;
                    }
                    if stream == Stream::Stdout {
                        let mut frames = Vec::new();
                        framer.feed(&data, &mut frames);
                        for frame in frames {
                            e.frame(frame, session);
                        }
                    }
                }
                Chunk::Eof(Stream::Stdout) => {
                    e.stdout_eof = true;
                    let mut frames = Vec::new();
                    framer.finish(&mut frames);
                    for frame in frames {
                        e.frame(frame, session);
                    }
                }
                Chunk::Eof(Stream::Stderr) => e.stderr_eof = true,
                Chunk::ReadError(..) => {
                    e.fail("provider_pipe_error");
                    let _ = t.cancel();
                    break;
                }
            }
        }
        match t.try_exit() {
            Ok(Some(x)) => {
                e.owned_exit = x.success;
                exit = x.code;
                saw_exit = true;
            }
            Err(_) => {
                e.fail("provider_exit_unknown");
                break;
            }
            _ => {}
        }
        if e.stdout_eof && e.stderr_eof && saw_exit {
            break;
        }
        if e.error.is_some() {
            let _ = t.cancel();
            break;
        }
    }
    e.finish();
    let ok = e.verified();
    json!({"seat_key":b.seat_key,"generation":b.generation,"turn":"1","request_id":rid,"session_id":session,"state":if ok{"completed"}else{"failed"},"success":ok,"completion_verified":ok,"reason":e.error,"runtime_mode":MODE,"requested_model":model,"resolved_model":e.resolved_model,"requested_effort":effort,"evidence":{"exact_init":e.init,"single_success_result":e.result && e.error.is_none(),"owned_exit_success":e.owned_exit,"exit_code":exit,"stdout_eof":e.stdout_eof,"stderr_eof":e.stderr_eof,"events":e.events,"output_bytes":bytes},"prompt_sha256":digest(prompt)})
}

fn arg<'a>(args: &'a [String], name: &str) -> Result<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .ok_or("missing_argument")
}
fn serve(args: &[String]) -> Result<()> {
    let raw = PathBuf::from(arg(args, "--state")?);
    if !raw.is_absolute() || raw.exists() {
        return Err("fresh_absolute_state_required");
    }
    let parent = raw.parent().ok_or("invalid_state_path")?;
    let parent = parent.canonicalize().map_err(|_| "state_parent_missing")?;
    let state_dir = parent.join(raw.file_name().ok_or("invalid_state_path")?);
    let cwd = absolute_existing(arg(args, "--cwd")?, true)?;
    let claude = absolute_existing(arg(args, "--claude-exe")?, false)?;
    let port = arg(args, "--port")
        .unwrap_or("0")
        .parse::<u16>()
        .map_err(|_| "invalid_port")?;
    fs::create_dir(&state_dir).map_err(|_| "state_claim_failed")?;
    create_json(
        &state_dir.join("claim.json"),
        &json!({"owner_pid":std::process::id(),"runtime_mode":MODE,"isolated_clone":true,"cwd":cwd}),
    )?;
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|_| "listen_failed")?;
    let endpoint = Endpoint {
        port: listener.local_addr().map_err(|_| "listen_failed")?.port(),
        token: random_hex(32)?,
        room_id: format!("isolated-{}", uuid()?),
        seat_id: format!("coordinator-{}", random_hex(8)?),
        runtime_mode: MODE.into(),
    };
    create_json(
        &state_dir.join("endpoint.json"),
        &serde_json::to_value(&endpoint).map_err(|_| "endpoint_encode_failed")?,
    )?;
    println!(
        "{}",
        json!({"ready":true,"room_id":endpoint.room_id,"seat_id":endpoint.seat_id,"port":endpoint.port,"runtime_mode":MODE,"state":state_dir})
    );
    let shared = Arc::new(Mutex::new(State {
        endpoint,
        state_dir,
        cwd,
        claude,
        seats: BTreeMap::new(),
        starts: BTreeMap::new(),
        sends: BTreeMap::new(),
        active: 0,
    }));
    let connections = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else {
            continue;
        };
        if connections.fetch_add(1, Ordering::AcqRel) >= 16 {
            connections.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let owned = shared.clone();
        let count = connections.clone();
        thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            let response = (|| -> Result<Value> {
                let v = read_line_capped(&mut stream)?;
                let token = owned
                    .lock()
                    .map_err(|_| "state_unavailable")?
                    .endpoint
                    .token
                    .clone();
                let supplied = v
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or("unauthorized")?;
                if supplied.len() > 128 || !constant_eq(&token, supplied) {
                    return Err("unauthorized");
                }
                dispatch(&owned, &v)
            })()
            .unwrap_or_else(error);
            let _ = serde_json::to_writer(&mut stream, &response);
            let _ = stream.write_all(b"\n");
            count.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Err("listener_stopped")
}
fn client(args: &[String]) -> Result<Value> {
    let state =
        env::var("SWITCHBITE_BACKEND_STATE").map_err(|_| "explicit_backend_state_required")?;
    let state = absolute_existing(&state, true)?;
    let ep: Endpoint = serde_json::from_slice(&read_capped(&state.join("endpoint.json"), 4096)?)
        .map_err(|_| "endpoint_invalid")?;
    if ep.runtime_mode != MODE || ep.token.len() != 64 || ep.port == 0 {
        return Err("endpoint_invalid");
    }
    let args = if args.first().map(String::as_str) == Some("agent") {
        &args[1..]
    } else {
        args
    };
    let command = args.first().ok_or("command_required")?;
    let mut v = json!({"token":ep.token,"command":command});
    match command.as_str() {
        "context" | "seats" => {}
        "start" => {
            v["request_id"] = json!(arg(args, "--request-id")?);
            v["provider"] = json!(arg(args, "--provider")?);
            v["cwd"] = json!(arg(args, "--cwd")?);
            v["model"] = json!(arg(args, "--model")?);
            v["effort"] = json!(arg(args, "--reasoning-effort")?);
            v["report_to"] = json!(arg(args, "--report-to")?);
            // Task/scope are only reservation metadata, never model onboarding.
            v["args"] = json!({"provider":v["provider"],"cwd":v["cwd"],"model":v["model"],"effort":v["effort"],"report_to":v["report_to"],"task":arg(args,"--task").unwrap_or(""),"scope":arg(args,"--scope").unwrap_or("")});
        }
        "launch-status" => {
            v["request_id"] = json!(args.get(1).ok_or("request_required")?);
        }
        "send" => {
            v["seat"] = json!(args.get(1).ok_or("seat_required")?);
            v["request_id"] = json!(arg(args, "--request-id")?);
            let prompt = read_capped(Path::new(arg(args, "--prompt-file")?), MAX_PROMPT)?;
            v["prompt"] = json!(String::from_utf8(prompt).map_err(|_| "prompt_utf8_required")?);
        }
        "wait" => {
            v["seat"] = json!(args.get(1).ok_or("seat_required")?);
            v["generation"] = json!(arg(args, "--generation")?);
            v["turn"] = json!(arg(args, "--turn")?);
        }
        _ => return Err("command_unsupported"),
    }
    let addr: SocketAddr = ([127, 0, 0, 1], ep.port).into();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|_| "backend_unavailable")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| "socket_failed")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "socket_failed")?;
    serde_json::to_writer(&mut stream, &v).map_err(|_| "request_write_failed")?;
    stream
        .write_all(b"\n")
        .map_err(|_| "request_write_failed")?;
    read_line_capped(&mut stream)
}
const BRIDGE_PROVIDERS: &[&str] = &["codex", "claude", "cursor", "opencode", "antigravity"];

fn bridge_provider_supported(provider: Option<&str>) -> bool {
    provider.is_some_and(|name| BRIDGE_PROVIDERS.contains(&name))
}

// The host owns the process tree and verifies provider receipts.
fn bridge_exec(args: &[String]) -> Result<i32> {
    let request: Value = serde_json::from_slice(&read_capped(
        Path::new(arg(args, "--request")?),
        1024 * 1024,
    )?)
    .map_err(|_| "invalid_request")?;
    if request["protocol"] != 1 || !bridge_provider_supported(request["provider"].as_str()) {
        return Err("invalid_request");
    }
    let mut command = Command::new(text(&request, "program")?);
    for item in request["args"].as_array().ok_or("invalid_request")? {
        command.arg(item.as_str().ok_or("invalid_request")?);
    }
    let mut transport = ChildTransport::spawn(command).map_err(|_| "provider_spawn_failed")?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(128 * 1024 + 1)
        .read_to_end(&mut input)
        .map_err(|_| "input_error")?;
    if input.len() > 128 * 1024 {
        return Err("input_too_long");
    }
    if !input.is_empty() && !transport.write_line(&input).is_written() {
        return Err("input_error");
    }
    transport.close_input();
    let (mut out_eof, mut err_eof, mut exit) = (false, false, None);
    loop {
        if let Some(chunk) = transport.recv(Duration::from_millis(50)) {
            match chunk {
                Chunk::Data(Stream::Stdout, bytes) => {
                    let mut out = std::io::stdout().lock();
                    out.write_all(&bytes)
                        .and_then(|_| out.flush())
                        .map_err(|_| "output_error")?;
                }
                Chunk::Data(Stream::Stderr, bytes) => {
                    let mut out = std::io::stderr().lock();
                    out.write_all(&bytes)
                        .and_then(|_| out.flush())
                        .map_err(|_| "output_error")?;
                }
                Chunk::Eof(Stream::Stdout) => out_eof = true,
                Chunk::Eof(Stream::Stderr) => err_eof = true,
                Chunk::ReadError(..) => return Err("output_error"),
            }
        }
        if exit.is_none() {
            exit = transport.try_exit().map_err(|_| "exit_unknown")?;
        }
        if out_eof && err_eof && exit.is_some() {
            break;
        }
    }
    Ok(exit.and_then(|e| e.code).unwrap_or(1))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--transport-fixture") {
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap();
        std::io::stdout().write_all(&input).unwrap();
        std::io::stderr().write_all(b"switchbite-stderr\n").unwrap();
        std::process::exit(7);
    }
    if args.first().map(String::as_str) == Some("self-test") {
        let outcome = (|| -> Result<()> {
            let mut command = Command::new(env::current_exe().map_err(|_| "self_test_failed")?);
            command.arg("--transport-fixture");
            let mut transport = ChildTransport::spawn(command).map_err(|_| "self_test_failed")?;
            if !transport.write_line(b"switchbite-stdout\n").is_written() {
                return Err("self_test_failed");
            }
            transport.close_input();
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let (mut out_eof, mut err_eof, mut exit) = (false, false, None);
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(5) {
                match transport.recv(Duration::from_millis(20)) {
                    Some(Chunk::Data(Stream::Stdout, bytes)) => stdout.extend(bytes),
                    Some(Chunk::Data(Stream::Stderr, bytes)) => stderr.extend(bytes),
                    Some(Chunk::Eof(Stream::Stdout)) => out_eof = true,
                    Some(Chunk::Eof(Stream::Stderr)) => err_eof = true,
                    Some(Chunk::ReadError(..)) => return Err("self_test_failed"),
                    None => (),
                }
                if exit.is_none() {
                    exit = transport.try_exit().map_err(|_| "self_test_failed")?;
                }
                if out_eof && err_eof && exit.is_some() {
                    break;
                }
            }
            if !out_eof
                || !err_eof
                || exit.and_then(|e| e.code) != Some(7)
                || stdout != b"switchbite-stdout\n"
                || stderr != b"switchbite-stderr\n"
            {
                return Err("self_test_failed");
            }
            println!(
                "{}",
                json!({"status":"passed","checks":["stdin","stdout","stderr","eof","exit_code"]})
            );
            Ok(())
        })();
        if outcome.is_err() {
            std::process::exit(1);
        }
        return;
    }
    if args.first().map(String::as_str) == Some("capabilities") {
        println!(
            "{}",
            json!({"kind":"stackbite_upstream_adapter","version":env!("CARGO_PKG_VERSION"),"stackbite_version":option_env!("SWITCHBITE_STACKBITE_VERSION").unwrap_or("unknown"),"stackbite_commit":option_env!("SWITCHBITE_STACKBITE_COMMIT").unwrap_or("unknown"),"protocol":1,"protocol_version":1,"providers":BRIDGE_PROVIDERS,"raw_streams":true,"session_isolation":"fresh_per_task"})
        );
        return;
    }
    if args.first().map(String::as_str) == Some("exec") {
        match bridge_exec(&args) {
            Ok(code) => std::process::exit(code),
            Err(code) => {
                eprintln!("{code}");
                std::process::exit(1);
            }
        }
    }
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        println!(
            "switchbite-stackbite-engine serve --state NEW_ABSOLUTE_DIR --claude-exe ABSOLUTE_EXE --cwd ABSOLUTE_DIR --port 0\nSWITCHBITE_BACKEND_STATE=ABSOLUTE_DIR switchbite-stackbite-engine [agent] context|seats|start|launch-status|send|wait\nPrivate isolated clone structured_one_shot runner. Start acknowledges backend readiness, never model onboarding. Prompt UTF-8 limit 16 KiB; owned execution limit 10 minutes, output 16 MiB. No original Stackbite state fallback."
        );
        return;
    }
    let result = if args.first().map(String::as_str) == Some("serve") {
        serve(&args).map(|_| Value::Null)
    } else {
        client(&args)
    };
    match result {
        Ok(v) if !v.is_null() => println!("{v}"),
        Ok(_) => {}
        Err(code) => {
            println!("{}", error(code));
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_advertises_exact_native_providers() {
        for provider in ["codex", "claude", "cursor", "opencode", "antigravity"] {
            assert!(bridge_provider_supported(Some(provider)));
        }
        for provider in [None, Some("grok"), Some("unknown"), Some("")] {
            assert!(!bridge_provider_supported(provider));
        }
        assert_eq!(BRIDGE_PROVIDERS.len(), 5);
    }
    fn init() -> Value {
        json!({"type":"system","subtype":"init","session_id":"session"})
    }
    fn result() -> Value {
        json!({"type":"result","subtype":"success","is_error":false,"permission_denials":[],"session_id":"session"})
    }
    fn complete(e: &mut Evidence) {
        e.stdout_eof = true;
        e.stderr_eof = true;
        e.owned_exit = true;
        e.finish();
    }
    #[test]
    fn success_requires_both_eofs_and_owned_exit() {
        let mut e = Evidence::default();
        e.event(&init(), "session");
        e.event(&result(), "session");
        assert!(!e.verified());
        e.stdout_eof = true;
        e.owned_exit = true;
        assert!(!e.verified());
        e.stderr_eof = true;
        e.finish();
        assert!(e.verified());
    }
    #[test]
    fn duplicate_result_fails_closed() {
        let mut e = Evidence::default();
        e.event(&init(), "session");
        e.event(&result(), "session");
        e.event(&result(), "session");
        complete(&mut e);
        assert!(!e.verified());
        assert_eq!(e.error, Some("duplicate_result"));
    }
    #[test]
    fn foreign_session_fails_closed() {
        let mut e = Evidence::default();
        e.event(&init(), "session");
        let mut v = result();
        v["session_id"] = json!("foreign");
        e.event(&v, "session");
        complete(&mut e);
        assert!(!e.verified());
    }
    #[test]
    fn error_and_permission_denial_fail_closed() {
        for v in [
            json!({"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"session","permission_denials":[]}),
            json!({"type":"result","subtype":"success","is_error":false,"session_id":"session","permission_denials":[{"tool_name":"Write"}]}),
        ] {
            let mut e = Evidence::default();
            e.event(&init(), "session");
            e.event(&v, "session");
            complete(&mut e);
            assert!(!e.verified());
        }
    }
    #[test]
    fn missing_result_and_init_never_complete() {
        let mut e = Evidence::default();
        e.event(&init(), "session");
        complete(&mut e);
        assert!(!e.verified());
        assert_eq!(e.error, Some("missing_result"));
        let mut e = Evidence::default();
        e.event(&result(), "session");
        complete(&mut e);
        assert!(!e.verified());
    }
    #[test]
    fn unverified_permission_field_and_tool_errors_fail() {
        let mut e = Evidence::default();
        e.event(&init(), "session");
        let mut r = result();
        r.as_object_mut().unwrap().remove("permission_denials");
        e.event(&r, "session");
        complete(&mut e);
        assert!(!e.verified());
        let mut e = Evidence::default();
        e.event(&init(), "session");
        e.event(
            &json!({"type":"user","message":{"content":[{"type":"tool_result","is_error":true}]}}),
            "session",
        );
        e.event(&result(), "session");
        complete(&mut e);
        assert!(!e.verified());
    }
    #[test]
    fn auth_and_digest() {
        assert!(constant_eq("secret", "secret"));
        assert!(!constant_eq("secret", "secreT"));
        assert!(!constant_eq("a", "a\0"));
        assert_eq!(
            digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn uuid_has_v4_shape() {
        let id = uuid().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(safe_id(&id));
    }
}
