use serde_json::{Value, json};
mod process;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_OUTPUT: u64 = 1024 * 1024;

#[derive(Debug)]
pub struct HostError(&'static str);
impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for HostError {}
impl HostError {
    pub fn to_value(&self) -> Value {
        json!({"code":self.0})
    }
}

pub struct LocalHost {
    workspace: PathBuf,
    state: PathBuf,
    base: String,
    timeout: u64,
}
fn git_lock(root: &Path) -> Result<fs::File, HostError> {
    lock_at(&root.join("git.lock"))
}
fn lock_at(path: &Path) -> Result<fs::File, HostError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| HostError("io_error"))?;
    lock_file(&file)?;
    Ok(file)
}
#[cfg(unix)]
fn lock_file(file: &fs::File) -> Result<(), HostError> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    if unsafe { flock(file.as_raw_fd(), 2) } == 0 {
        Ok(())
    } else {
        Err(HostError("git_locked"))
    }
}
#[cfg(windows)]
fn lock_file(file: &fs::File) -> Result<(), HostError> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut std::ffi::c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFileEx(
            file: *mut std::ffi::c_void,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    let mut o = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: std::ptr::null_mut(),
    };
    if unsafe { LockFileEx(file.as_raw_handle(), 2, 0, 1, 0, &mut o) } != 0 {
        Ok(())
    } else {
        Err(HostError("git_locked"))
    }
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str, HostError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or(HostError("invalid_action"))
}
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))[..24].to_string()
}
fn path_allowed(path: &str, pattern: &str) -> bool {
    if pattern == "." {
        return true;
    }
    let mut expression = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '*' {
            if chars.peek() == Some(&'*') {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    expression.push_str("(?:.*/)?");
                } else {
                    expression.push_str(".*");
                }
            } else {
                expression.push_str("[^/]*");
            }
        } else {
            expression.push_str(&regex::escape(&c.to_string()));
        }
    }
    expression.push_str("(?:/.*)?$");
    regex::Regex::new(&expression).is_ok_and(|r| r.is_match(path))
}
fn session_id(id: &str) -> String {
    let h = format!("{:x}", Sha256::digest(id.as_bytes()));
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}
fn verified_receipt(provider: &str, output: &[u8], session: &str) -> (bool, Option<String>) {
    let mut init = false;
    let mut turn = false;
    let mut terminal = false;
    let mut model = None;
    let mut valid = true;
    for line in output.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            return (false, None);
        };
        let Some(kind) = v.get("type").and_then(Value::as_str) else {
            return (false, None);
        };
        if terminal {
            valid = false;
        }
        if provider == "claude" {
            if v.get("session_id")
                .is_some_and(|x| x.as_str() != Some(session))
            {
                valid = false;
            }
            match kind {
                "system" if v["subtype"] == "init" => {
                    if init || v["session_id"] != session {
                        valid = false;
                    }
                    init = true;
                    model = v
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty() && s.len() <= 256)
                        .map(str::to_owned);
                }
                "result" => {
                    if !init
                        || v["session_id"] != session
                        || v["subtype"] != "success"
                        || v["is_error"] != false
                        || !v
                            .get("permission_denials")
                            .and_then(Value::as_array)
                            .is_some_and(Vec::is_empty)
                    {
                        valid = false;
                    }
                    terminal = true;
                }
                "error" => valid = false,
                "assistant" | "user" => {
                    if !init
                        || v.get("error").is_some_and(|x| !x.is_null())
                        || v["message"]["stop_reason"] == "refusal"
                        || v["message"]["content"].as_array().is_some_and(|a| {
                            a.iter()
                                .any(|x| x["is_error"] == true || x["type"] == "refusal")
                        })
                    {
                        valid = false;
                    }
                }
                _ => {
                    if !init {
                        valid = false;
                    }
                }
            }
        } else {
            match kind {
                "thread.started" => {
                    if init || v["thread_id"].as_str().is_none_or(str::is_empty) {
                        valid = false;
                    }
                    init = true;
                }
                "turn.started" => {
                    if !init || turn {
                        valid = false;
                    }
                    turn = true;
                }
                "turn.completed" => {
                    if !init || !turn {
                        valid = false;
                    }
                    terminal = true;
                }
                "error" | "turn.failed" | "item.failed" => valid = false,
                _ => {
                    if !turn
                        || v["item"]["status"] == "failed"
                        || v["item"]["exit_code"].as_i64().is_some_and(|n| n != 0)
                    {
                        valid = false;
                    }
                }
            }
        }
    }
    (valid && init && terminal, model)
}
fn run_git(dir: &Path, args: &[&str]) -> Result<String, HostError> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|_| HostError("git_failed"))?;
    if !output.status.success() || output.stdout.len() > MAX_OUTPUT as usize {
        return Err(HostError("git_failed"));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|_| HostError("git_failed"))
}
fn read_bounded(path: &Path) -> Result<Vec<u8>, HostError> {
    let mut f = fs::File::open(path).map_err(|_| HostError("io_error"))?;
    let mut b = Vec::new();
    Read::by_ref(&mut f)
        .take(MAX_OUTPUT + 1)
        .read_to_end(&mut b)
        .map_err(|_| HostError("io_error"))?;
    if b.len() as u64 > MAX_OUTPUT {
        return Err(HostError("output_limit"));
    }
    Ok(b)
}
fn value_file(path: &Path) -> Result<Value, HostError> {
    serde_json::from_slice(&read_bounded(path)?).map_err(|_| HostError("invalid_state"))
}
fn write_atomic(path: &Path, v: &Value) -> Result<(), HostError> {
    let tmp = path.with_extension("tmp");
    fs::write(
        &tmp,
        serde_json::to_vec(v).map_err(|_| HostError("io_error"))?,
    )
    .map_err(|_| HostError("io_error"))?;
    if path.exists() {
        fs::remove_file(path).map_err(|_| HostError("io_error"))?;
    }
    fs::rename(tmp, path).map_err(|_| HostError("io_error"))
}
fn path_string(p: &Path) -> Result<&str, HostError> {
    p.to_str()
        .map(|s| {
            if cfg!(windows) {
                s.strip_prefix(r"\\?\").unwrap_or(s)
            } else {
                s
            }
        })
        .ok_or(HostError("invalid_path"))
}
impl LocalHost {
    pub fn new(workspace: &Path, state_dir: &Path, config: &Value) -> Result<Self, HostError> {
        let workspace = fs::canonicalize(workspace).map_err(|_| HostError("invalid_workspace"))?;
        fs::create_dir_all(state_dir).map_err(|_| HostError("io_error"))?;
        let state = fs::canonicalize(state_dir).map_err(|_| HostError("io_error"))?;
        let reference = config.get("base").and_then(Value::as_str).unwrap_or("HEAD");
        let base = run_git(
            &workspace,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", reference),
            ],
        )?;
        if base.len() != 40 && base.len() != 64 {
            return Err(HostError("invalid_base"));
        }
        Ok(Self {
            workspace,
            state,
            base,
            timeout: config
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(900)
                .clamp(1, 3600),
        })
    }
    pub fn describe(&self) -> Value {
        json!({"kind":"local","base":self.base,"providers":["claude","codex"],"process_tree":if cfg!(windows) {"windows_job"} else {"unsupported"},"timeout_seconds":self.timeout,"requires":["git","candidate.provider","candidate.executable","candidate.model","candidate.effort"]})
    }
    pub fn perform(&self, action: &Value) -> Result<Value, HostError> {
        let id = string(action, "id")?;
        let kind = string(action, "kind")?;
        let payload = action
            .get("payload")
            .filter(|v| v.is_object())
            .ok_or(HostError("invalid_action"))?;
        let dir = self.state.join("actions");
        fs::create_dir_all(&dir).map_err(|_| HostError("io_error"))?;
        let key = hash(id);
        let _action_lock = lock_at(&dir.join(format!("{key}.lock")))?;
        let record = dir.join(format!("{key}.json"));
        let claim = dir.join(format!("{key}.claim"));
        let fingerprint =
            hash(&serde_json::to_string(action).map_err(|_| HostError("invalid_action"))?);
        if record.exists() {
            let saved = value_file(&record)?;
            if saved["fingerprint"] != fingerprint {
                return Err(HostError("action_conflict"));
            }
            return Ok(saved["result"].clone());
        }
        match OpenOptions::new().write(true).create_new(true).open(&claim) {
            Ok(mut f) => {
                f.write_all(fingerprint.as_bytes())
                    .map_err(|_| HostError("io_error"))?;
                f.sync_all().map_err(|_| HostError("io_error"))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read_to_string(&claim).ok().as_deref() != Some(&fingerprint) {
                    return Err(HostError("action_conflict"));
                }
                if kind == "launch" {
                    return Ok(
                        json!({"status":"unknown","execution_id":id,"completion_verified":false}),
                    );
                }
                if !["prepare", "inspect", "gather_context"].contains(&kind) {
                    return Err(HostError("recovery_required"));
                }
            }
            Err(_) => return Err(HostError("io_error")),
        }
        let result = match kind {
            "prepare" => self.prepare(action, payload),
            "launch" => self.launch(action, payload),
            "inspect" => self.inspect(payload),
            "verify" => self.verify(payload),
            "integrate" => self.integrate(payload),
            "gather_context" => self.gather(payload),
            _ => Err(HostError("unsupported_action")),
        };
        match result {
            Ok(v) => {
                write_atomic(&record, &json!({"fingerprint":fingerprint,"result":v}))?;
                Ok(v)
            }
            Err(e) => {
                if ["prepare", "inspect", "gather_context"].contains(&kind) {
                    let _ = fs::remove_file(&claim);
                }
                Err(e)
            }
        }
    }
    fn prepare(&self, action: &Value, p: &Value) -> Result<Value, HostError> {
        let run = hash(string(action, "run_id")?);
        let task = p.get("task").ok_or(HostError("invalid_action"))?;
        let taskid = hash(string(task, "id")?);
        let root = self.state.join("worktrees").join(&run);
        fs::create_dir_all(&root).map_err(|_| HostError("io_error"))?;
        let _lock = git_lock(&root)?;
        let base_file = root.join("base");
        if !base_file.exists() {
            fs::write(&base_file, &self.base).map_err(|_| HostError("io_error"))?;
        }
        let base = fs::read_to_string(&base_file).map_err(|_| HostError("io_error"))?;
        let integration = root.join("integration");
        let ib = format!("switchbite/{run}/integration");
        if !integration.exists() {
            run_git(
                &self.workspace,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &ib,
                    path_string(&integration)?,
                    &base,
                ],
            )?;
        }
        if run_git(&integration, &["branch", "--show-current"])? != ib {
            return Err(HostError("recovery_required"));
        }
        let taskpath = root.join(&taskid);
        let branch = format!("switchbite/{run}/{taskid}");
        if !taskpath.exists() {
            let head = run_git(&integration, &["rev-parse", "HEAD"])?;
            run_git(
                &self.workspace,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    path_string(&taskpath)?,
                    &head,
                ],
            )?;
        }
        if run_git(&taskpath, &["branch", "--show-current"])? != branch {
            return Err(HostError("recovery_required"));
        }
        Ok(
            json!({"status":"prepared","workspace":taskpath,"branch":branch,"integration_workspace":integration,"integration_branch":ib}),
        )
    }
    fn owned(&self, p: &Value, key: &str) -> Result<PathBuf, HostError> {
        let s = string(p.get("prepared").ok_or(HostError("invalid_action"))?, key)?;
        let path = fs::canonicalize(s).map_err(|_| HostError("invalid_workspace"))?;
        let root = fs::canonicalize(self.state.join("worktrees"))
            .map_err(|_| HostError("invalid_workspace"))?;
        if !path.starts_with(root) {
            return Err(HostError("invalid_workspace"));
        }
        Ok(path)
    }
    fn execute(
        &self,
        mut cmd: Command,
        dir: &Path,
        tag: &str,
        launch: bool,
    ) -> Result<(bool, Vec<u8>), HostError> {
        let logs = self.state.join("output");
        fs::create_dir_all(&logs).map_err(|_| HostError("io_error"))?;
        let execution_dir = self.state.join("executions");
        let cancel = execution_dir.join(format!("{}.cancel", hash(tag)));
        if launch && cancel.exists() {
            return Err(HostError("cancelled"));
        }
        let out = logs.join(format!("{}.out", hash(tag)));
        let err = logs.join(format!("{}.err", hash(tag)));
        let stdout = fs::File::create(&out).map_err(|_| HostError("io_error"))?;
        let stderr = fs::File::create(&err).map_err(|_| HostError("io_error"))?;
        cmd.current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let mut tree = process::Tree::spawn(&mut cmd)
            .map_err(|_| HostError("process_ownership_unavailable"))?;
        if launch {
            fs::create_dir_all(&execution_dir).map_err(|_| HostError("io_error"))?;
            if write_atomic(&execution_dir.join(format!("{}.json", hash(tag))), &json!({"execution_id":tag,"token":hash(tag),"pid":tree.child.id(),"owner_pid":std::process::id(),"started_unix_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()})).is_err() {
                tree.terminate().map_err(|_| HostError("process_ownership_unavailable"))?;
                return Err(HostError("io_error"));
            }
        }
        let started = Instant::now();
        loop {
            if let Some(status) = tree
                .child
                .try_wait()
                .map_err(|_| HostError("process_failed"))?
                .filter(|_| tree.active().is_ok_and(|n| n == 0))
            {
                let bytes = read_bounded(&out)?;
                if fs::metadata(&err)
                    .map(|m| m.len() > MAX_OUTPUT)
                    .unwrap_or(false)
                {
                    let _ = fs::remove_file(&out);
                    let _ = fs::remove_file(&err);
                    return Err(HostError("output_limit"));
                }
                let _ = fs::remove_file(&out);
                let _ = fs::remove_file(&err);
                return Ok((status.success(), bytes));
            }
            if launch && cancel.exists() {
                tree.terminate()
                    .map_err(|_| HostError("process_ownership_unavailable"))?;
                let _ = fs::remove_file(&out);
                let _ = fs::remove_file(&err);
                return Err(HostError("cancelled"));
            }
            if started.elapsed() >= Duration::from_secs(self.timeout) {
                tree.terminate()
                    .map_err(|_| HostError("process_ownership_unavailable"))?;
                let _ = fs::remove_file(&out);
                let _ = fs::remove_file(&err);
                return Err(HostError("timeout"));
            }
            if fs::metadata(&out)
                .map(|m| m.len() > MAX_OUTPUT)
                .unwrap_or(false)
                || fs::metadata(&err)
                    .map(|m| m.len() > MAX_OUTPUT)
                    .unwrap_or(false)
            {
                tree.terminate()
                    .map_err(|_| HostError("process_ownership_unavailable"))?;
                let _ = fs::remove_file(&out);
                let _ = fs::remove_file(&err);
                return Err(HostError("output_limit"));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    fn launch(&self, a: &Value, p: &Value) -> Result<Value, HostError> {
        let dir = self.owned(p, "workspace")?;
        let c = p.get("candidate").ok_or(HostError("invalid_action"))?;
        let provider = string(c, "provider")?;
        let executable = string(c, "executable")?;
        let model = string(c, "model")?;
        let effort = string(c, "effort")?;
        if !effort
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(HostError("invalid_action"));
        }
        let task = p.get("task").ok_or(HostError("invalid_action"))?;
        string(task, "objective")?;
        let prompt = format!(
            "Complete the following task only within this working directory. Treat outputs beginning with artifact: as references, not writable paths. Task: {}",
            serde_json::to_string(task).map_err(|_| HostError("invalid_action"))?
        );
        if prompt.len() > 128 * 1024 {
            return Err(HostError("invalid_action"));
        }
        let mut cmd = Command::new(executable);
        if c.get("args")
            .and_then(Value::as_array)
            .is_some_and(|args| !args.is_empty())
        {
            return Err(HostError("unsupported_candidate_args"));
        }
        let id = string(a, "id")?;
        let session = session_id(id);
        match provider {
            "codex" => {
                cmd.args([
                    "exec",
                    "--json",
                    "--model",
                    model,
                    "--sandbox",
                    "workspace-write",
                    "--ignore-user-config",
                    "-c",
                    &format!("model_reasoning_effort=\"{effort}\""),
                    &prompt,
                ]);
            }
            "claude" => {
                cmd.args([
                    "--print",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--model",
                    model,
                    "--effort",
                    effort,
                    "--session-id",
                    &session,
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
                    "--",
                    &prompt,
                ]);
            }
            _ => return Err(HostError("unsupported_provider")),
        }
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy().to_ascii_uppercase();
            if [
                "STACKBITE_",
                "WORKBITE_",
                "WORKSTACK_",
                "SWITCHBITE_",
                "HELM_",
                "CLAUDECODE",
                "CLAUDE_CODE_",
                "TYPESAFE_API_KEY",
            ]
            .iter()
            .any(|p| name.starts_with(p))
            {
                cmd.env_remove(key);
            }
        }
        let (ok, output) = match self.execute(cmd, &dir, id, true) {
            Ok(result) => result,
            Err(HostError("cancelled")) => {
                return Ok(
                    json!({"status":"failed","execution_id":id,"completion_verified":false,"cancelled":true}),
                );
            }
            Err(error) => return Err(error),
        };
        let (verified, resolved_model) = verified_receipt(provider, &output, &session);
        Ok(
            json!({"status":if ok && verified {"completed"} else {"failed"},"execution_id":id,"completion_verified":ok && verified,"requested_model":model,"resolved_model":resolved_model}),
        )
    }
    fn inspect(&self, p: &Value) -> Result<Value, HostError> {
        let id = string(p, "execution_id")?;
        let record = self
            .state
            .join("actions")
            .join(format!("{}.json", hash(id)));
        if record.exists() {
            return Ok(value_file(&record)?["result"].clone());
        }
        if p.get("operation").and_then(Value::as_str) == Some("cancel") {
            let executions = self.state.join("executions");
            fs::create_dir_all(&executions).map_err(|_| HostError("io_error"))?;
            fs::write(executions.join(format!("{}.cancel", hash(id))), b"cancel")
                .map_err(|_| HostError("io_error"))?;
        }
        Ok(json!({"status":"unknown","execution_id":id,"completion_verified":false}))
    }
    fn verify(&self, p: &Value) -> Result<Value, HostError> {
        let dir = self.owned(p, "workspace")?;
        let checks = p
            .get("checks")
            .and_then(Value::as_array)
            .ok_or(HostError("invalid_action"))?;
        if checks.len() > 32 {
            return Err(HostError("invalid_action"));
        }
        let mut evidence = Vec::new();
        let mut passed = true;
        for (i, c) in checks.iter().enumerate() {
            let program = string(c, "program")?;
            let args = c
                .get("args")
                .and_then(Value::as_array)
                .ok_or(HostError("invalid_action"))?;
            if args.len() > 64 {
                return Err(HostError("invalid_action"));
            }
            let mut cmd = Command::new(program);
            for arg in args {
                cmd.arg(
                    arg.as_str()
                        .filter(|s| s.len() <= 4096)
                        .ok_or(HostError("invalid_action"))?,
                );
            }
            let ok = self
                .execute(cmd, &dir, &format!("check-{}-{i}", dir.display()), false)
                .map(|(ok, _)| ok)
                .unwrap_or(false);
            passed &= ok;
            evidence.push(json!({"check":i,"status":if ok {"passed"} else {"failed"}}));
        }
        Ok(json!({"status":if passed {"passed"} else {"failed"},"evidence":evidence}))
    }
    fn integrate(&self, p: &Value) -> Result<Value, HostError> {
        let task = self.owned(p, "workspace")?;
        let target = self.owned(p, "integration_workspace")?;
        if task.parent() != target.parent() || task == target {
            return Err(HostError("invalid_workspace"));
        }
        let _lock = git_lock(target.parent().ok_or(HostError("invalid_workspace"))?)?;
        if !run_git(&target, &["status", "--porcelain"])?.is_empty() {
            return Ok(json!({"status":"failed","evidence":["integration workspace dirty"]}));
        }
        run_git(&task, &["add", "-A"])?;
        let outputs = p["task"]
            .get("write_paths")
            .or_else(|| p["task"].get("outputs"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut owned = Vec::new();
        for output in outputs {
            let s = output.as_str().ok_or(HostError("invalid_action"))?;
            if s.starts_with("artifact:") || s.contains("://") {
                continue;
            }
            let path = Path::new(s);
            if s != "."
                && (path.is_absolute()
                    || path
                        .components()
                        .any(|c| !matches!(c, std::path::Component::Normal(_))))
            {
                return Err(HostError("invalid_output_path"));
            }
            owned.push(s.replace('\\', "/").trim_end_matches('/').to_owned());
        }
        {
            let ancestor = run_git(
                &task,
                &[
                    "merge-base",
                    "HEAD",
                    &run_git(&target, &["rev-parse", "HEAD"])?,
                ],
            )?;
            let changed = run_git(&task, &["diff", "--cached", "--name-only", "-z", &ancestor])?;
            if changed
                .split('\0')
                .filter(|s| !s.is_empty())
                .any(|path| !owned.iter().any(|o| path_allowed(path, o)))
            {
                return Ok(
                    json!({"status":"failed","evidence":["write outside declared outputs"]}),
                );
            }
        }
        let status = run_git(&task, &["status", "--porcelain"])?;
        if !status.is_empty() {
            run_git(
                &task,
                &[
                    "-c",
                    "user.name=Switchbite",
                    "-c",
                    "user.email=switchbite@localhost",
                    "commit",
                    "-m",
                    "Switchbite task",
                ],
            )?;
        }
        let head = run_git(&task, &["rev-parse", "HEAD"])?;
        let target_head = run_git(&target, &["rev-parse", "HEAD"])?;
        if head != target_head
            && run_git(
                &target,
                &[
                    "-c",
                    "user.name=Switchbite",
                    "-c",
                    "user.email=switchbite@localhost",
                    "merge",
                    "--no-edit",
                    &head,
                ],
            )
            .is_err()
        {
            let _ = run_git(&target, &["merge", "--abort"]);
            return Ok(json!({"status":"failed","evidence":["integration conflict"]}));
        }
        let commit = run_git(&target, &["rev-parse", "HEAD"])?;
        let mut integration_payload = p.clone();
        integration_payload["prepared"]["workspace"] = json!(target);
        let checked = self
            .verify(&integration_payload)
            .unwrap_or_else(|_| json!({"status":"failed","evidence":[]}));
        if checked["status"] != "passed"
            || !run_git(&target, &["status", "--porcelain"])?.is_empty()
        {
            run_git(&target, &["reset", "--hard", &target_head])?;
            return Ok(
                json!({"status":"failed","commit":commit,"evidence":checked["evidence"],"reason":"integration_checks_failed"}),
            );
        }
        Ok(json!({"status":"integrated","commit":commit,"evidence":checked["evidence"]}))
    }
    fn gather(&self, p: &Value) -> Result<Value, HostError> {
        let task = p.get("task").ok_or(HostError("invalid_action"))?;
        let refs = task
            .get("context_refs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if refs.len() > 64 || refs.iter().any(|r| !r.is_string()) {
            return Err(HostError("invalid_action"));
        }
        for r in &refs {
            let reference = r.as_str().ok_or(HostError("invalid_action"))?;
            if reference.starts_with("artifact:") {
                continue;
            }
            let rel = Path::new(reference);
            let resolved = fs::canonicalize(self.workspace.join(rel))
                .map_err(|_| HostError("context_missing"))?;
            if !resolved.starts_with(&self.workspace) || !resolved.is_file() {
                return Err(HostError("context_missing"));
            }
        }
        Ok(json!({"status":"gathered","context_refs":refs}))
    }
}
#[cfg(test)]
mod tests {
    use super::verified_receipt;
    #[test]
    fn receipts_fail_closed_on_contradictions() {
        let valid = b"{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n{\"type\":\"turn.started\"}\n{\"type\":\"turn.completed\"}\n";
        assert!(verified_receipt("codex", valid, "unused").0);
        assert!(!verified_receipt("codex", b"{\"type\":\"turn.completed\"}\n", "unused").0);
        let mut duplicate = valid.to_vec();
        duplicate.extend_from_slice(b"{\"type\":\"turn.completed\"}\n");
        assert!(!verified_receipt("codex", &duplicate, "unused").0);
        let claude = b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\",\"model\":\"m\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"permission_denials\":[],\"session_id\":\"s\"}\n";
        assert_eq!(
            verified_receipt("claude", claude, "s"),
            (true, Some("m".into()))
        );
        assert!(!verified_receipt("claude", claude, "other").0);
        assert!(!verified_receipt("claude", b"{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"session_id\":\"s\"}\n", "s").0);
    }
}
