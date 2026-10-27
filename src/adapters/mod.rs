use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
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
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str, HostError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or(HostError("invalid_action"))
}
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))[..24].to_string()
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
        json!({"kind":"local","base":self.base,"providers":["claude","codex"],"timeout_seconds":self.timeout,"requires":["git","candidate.provider","candidate.executable","candidate.model","candidate.effort"]})
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
                return Ok(
                    json!({"status":"unknown","execution_id":id,"completion_verified":false}),
                );
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
                if kind != "launch" {
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
                    &self.base,
                ],
            )?;
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
    ) -> Result<(bool, Vec<u8>), HostError> {
        let logs = self.state.join("output");
        fs::create_dir_all(&logs).map_err(|_| HostError("io_error"))?;
        let out = logs.join(format!("{}.out", hash(tag)));
        let err = logs.join(format!("{}.err", hash(tag)));
        let stdout = fs::File::create(&out).map_err(|_| HostError("io_error"))?;
        let stderr = fs::File::create(&err).map_err(|_| HostError("io_error"))?;
        let mut child = cmd
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|_| HostError("spawn_failed"))?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|_| HostError("process_failed"))? {
                let bytes = read_bounded(&out)?;
                let _ = fs::remove_file(&out);
                let _ = fs::remove_file(&err);
                return Ok((status.success(), bytes));
            }
            if started.elapsed() >= Duration::from_secs(self.timeout) {
                let _ = child.kill();
                let _ = child.wait();
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
                let _ = child.kill();
                let _ = child.wait();
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
        let prompt = string(task, "objective")?;
        let mut cmd = Command::new(executable);
        match provider {
            "codex" => {
                cmd.args([
                    "exec",
                    "--json",
                    "--model",
                    model,
                    "-c",
                    &format!("model_reasoning_effort=\"{effort}\""),
                    prompt,
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
                    prompt,
                ]);
            }
            _ => return Err(HostError("unsupported_provider")),
        }
        if let Some(args) = c.get("args").and_then(Value::as_array) {
            if args.len() > 32 {
                return Err(HostError("invalid_action"));
            }
            for arg in args {
                cmd.arg(
                    arg.as_str()
                        .filter(|s| s.len() <= 4096)
                        .ok_or(HostError("invalid_action"))?,
                );
            }
        }
        let id = string(a, "id")?;
        let (ok, output) = self.execute(cmd, &dir, id)?;
        let verified = output
            .split(|b| *b == b'\n')
            .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
            .any(|v| match provider {
                "codex" => v["type"] == "turn.completed",
                "claude" => v["type"] == "result" && v["is_error"] == false,
                _ => false,
            });
        Ok(
            json!({"status":if ok && verified {"completed"} else {"failed"},"execution_id":id,"completion_verified":ok && verified}),
        )
    }
    fn inspect(&self, p: &Value) -> Result<Value, HostError> {
        let id = string(p, "execution_id")?;
        if p.get("operation").and_then(Value::as_str) == Some("cancel") {
            return Ok(json!({"status":"unknown","execution_id":id,"completion_verified":false}));
        }
        let record = self
            .state
            .join("actions")
            .join(format!("{}.json", hash(id)));
        if record.exists() {
            return Ok(value_file(&record)?["result"].clone());
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
                .execute(cmd, &dir, &format!("check-{}-{i}", dir.display()))
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
        if !run_git(&target, &["status", "--porcelain"])?.is_empty() {
            return Ok(json!({"status":"failed","evidence":["integration workspace dirty"]}));
        }
        run_git(&task, &["add", "-A"])?;
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
        Ok(json!({"status":"integrated","commit":commit,"evidence":[]}))
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
            let rel = Path::new(r.as_str().ok_or(HostError("invalid_action"))?);
            let resolved = fs::canonicalize(self.workspace.join(rel))
                .map_err(|_| HostError("context_missing"))?;
            if !resolved.starts_with(&self.workspace) || !resolved.is_file() {
                return Err(HostError("context_missing"));
            }
        }
        Ok(json!({"status":"gathered","context_refs":refs}))
    }
}
