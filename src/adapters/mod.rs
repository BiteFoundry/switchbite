use serde_json::{Value, json};
mod process;
mod workers;
use workers::verified_receipt;

pub fn supports_provider(provider: &str) -> bool {
    matches!(
        provider,
        "claude" | "codex" | "cursor" | "opencode" | "antigravity"
    )
}
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_OUTPUT: u64 = 1024 * 1024;
// Codex discovers PowerShell from its child PATH. Store aliases cannot be
// started by the restricted Windows token; prefer a real executable instead.
#[cfg(windows)]
fn codex_shell_path(path: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    fn packaged(path: &Path) -> bool {
        let blocked = |p: &Path| {
            p.components().any(|part| {
                part.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("WindowsApps")
            })
        };
        blocked(path) || fs::canonicalize(path).ok().is_some_and(|p| blocked(&p))
    }
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    let find = |name: &str| {
        dirs.iter()
            .find(|dir| {
                let exe = dir.join(name);
                dir.is_absolute() && exe.is_file() && !packaged(&exe)
            })
            .cloned()
    };
    let preferred = find("pwsh.exe");
    let selected = preferred
        .clone()
        .or_else(|| find("powershell.exe"))
        .or_else(|| {
            let dir = PathBuf::from(std::env::var_os("SystemRoot")?)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0");
            let exe = dir.join("powershell.exe");
            (exe.is_file() && !packaged(&exe)).then_some(dir)
        })?;
    let mut adjusted = vec![selected];
    adjusted.extend(dirs.into_iter().filter(|dir| {
        // With only Windows PowerShell available, remove inaccessible pwsh
        // aliases so Codex can fall back instead of rediscovering the alias.
        preferred.is_some() || !(packaged(&dir.join("pwsh.exe")) && dir.join("pwsh.exe").exists())
    }));
    std::env::join_paths(adjusted).ok()
}
#[cfg(windows)]
fn configure_codex_shell(cmd: &mut Command) {
    if let Some(path) = std::env::var_os("PATH").and_then(|p| codex_shell_path(&p)) {
        cmd.env("PATH", path);
    }
}
#[cfg(not(windows))]
fn configure_codex_shell(_: &mut Command) {}

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
    worktrees: PathBuf,
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
fn run_git(dir: &Path, args: &[&str]) -> Result<String, HostError> {
    let output = Command::new("git")
        .args(["-c", "core.longpaths=true"])
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
fn cursor_weekly_limit(bytes: &[u8]) -> bool {
    bytes
        .windows(b"ActionRequiredError: Weekly usage limit reached.".len())
        .any(|window| window == b"ActionRequiredError: Weekly usage limit reached.")
}
fn failure_hint(bytes: &[u8]) -> Option<&'static str> {
    let message = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    if (message.contains("effort") || message.contains("variant"))
        && (message.contains("unsupported")
            || message.contains("invalid")
            || message.contains("not found"))
    {
        Some("unsupported_effort")
    } else if message.contains("authentication")
        || message.contains("unauthorized")
        || message.contains("login required")
    {
        Some("authentication_failed")
    } else if message.contains("quota") || message.contains("rate limit") {
        Some("quota_or_rate_limit")
    } else if message.contains("model")
        && (message.contains("not found") || message.contains("unsupported"))
    {
        Some("model_unavailable")
    } else if message.contains("permission denied") {
        Some("permission_denied")
    } else {
        None
    }
}
fn attach_rate_limit_error(result: &mut Value) {
    if let Some(code @ ("rate_limited" | "quota_or_rate_limit")) = result["reason"].as_str() {
        result["error"] = json!({"code":code});
    }
}
fn value_file(path: &Path) -> Result<Value, HostError> {
    serde_json::from_slice(&read_bounded(path)?).map_err(|_| HostError("invalid_state"))
}
fn write_atomic(path: &Path, v: &Value) -> Result<(), HostError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(
        &tmp,
        serde_json::to_vec(v).map_err(|_| HostError("io_error"))?,
    )
    .map_err(|_| HostError("io_error"))?;
    if replace_atomic(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
        return Err(HostError("io_error"));
    }
    Ok(())
}
#[cfg(not(windows))]
fn replace_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}
#[cfg(windows)]
fn replace_atomic(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // MOVEFILE_REPLACE_EXISTING keeps readers from seeing a missing record.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Query only the named job and the recorded process identity; never signal it.
pub(crate) fn execution_alive(record: &Value) -> Option<bool> {
    let name = record["job_name"].as_str()?;
    // A surviving owned job can contain tools after its leader exits.
    process::job_drained(name).map(|drained| !drained)
}

fn output_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn retain_tail(path: &Path) -> Result<(), HostError> {
    const TAIL: u64 = 64 * 1024;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| HostError("io_error"))?;
    let len = file.metadata().map_err(|_| HostError("io_error"))?.len();
    if len > TAIL {
        file.seek(SeekFrom::End(-(TAIL as i64)))
            .map_err(|_| HostError("io_error"))?;
        let mut tail = Vec::with_capacity(TAIL as usize);
        file.read_to_end(&mut tail)
            .map_err(|_| HostError("io_error"))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| HostError("io_error"))?;
        file.write_all(&tail).map_err(|_| HostError("io_error"))?;
        file.set_len(tail.len() as u64)
            .map_err(|_| HostError("io_error"))?;
    }
    Ok(())
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
fn validate_worktree(path: &Path, branch: &str, original: &Path) -> Result<(), HostError> {
    if run_git(path, &["branch", "--show-current"])? != branch {
        return Err(HostError("recovery_required"));
    }
    let top = run_git(path, &["rev-parse", "--show-toplevel"])?;
    if fs::canonicalize(top).map_err(|_| HostError("recovery_required"))?
        != fs::canonicalize(path).map_err(|_| HostError("recovery_required"))?
    {
        return Err(HostError("recovery_required"));
    }
    let common = |directory: &Path| -> Result<PathBuf, HostError> {
        let s = run_git(directory, &["rev-parse", "--git-common-dir"])?;
        fs::canonicalize(directory.join(s)).map_err(|_| HostError("recovery_required"))
    };
    if common(path)? != common(original)? {
        return Err(HostError("recovery_required"));
    }
    Ok(())
}
impl LocalHost {
    fn add_worktree(&self, path: &Path, branch: &str, base: &str) -> Result<(), HostError> {
        if let Ok(existing) = run_git(
            &self.workspace,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{branch}^{{commit}}"),
            ],
        ) {
            if existing != base {
                return Err(HostError("worktree_branch_conflict"));
            }
            run_git(
                &self.workspace,
                &["worktree", "add", path_string(path)?, branch],
            )?;
        } else {
            run_git(
                &self.workspace,
                &["worktree", "add", "-b", branch, path_string(path)?, base],
            )?;
        }
        Ok(())
    }
    pub fn new(workspace: &Path, state_dir: &Path, config: &Value) -> Result<Self, HostError> {
        let workspace = fs::canonicalize(workspace).map_err(|_| HostError("invalid_workspace"))?;
        fs::create_dir_all(state_dir).map_err(|_| HostError("io_error"))?;
        let state = fs::canonicalize(state_dir).map_err(|_| HostError("io_error"))?;
        // Git for Windows has fixed-size internal GIT_DIR buffers even with
        // core.longpaths. Keep owned worktrees short while receipts stay with
        // the project. The state-path digest isolates different runs/stores.
        let worktrees = if cfg!(windows) && path_string(&state)?.len() > 120 {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Switchbite")
                .join("workspaces")
                .join(hash(path_string(&state)?))
        } else {
            state.join("worktrees")
        };
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
            worktrees,
            base,
            timeout: config
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(900)
                .clamp(1, 3600),
        })
    }
    pub fn describe(&self) -> Value {
        json!({"kind":"local","base":self.base,"providers":["claude","codex","cursor","opencode","antigravity"],"process_tree":if cfg!(windows) {"windows_job"} else {"unsupported"},"timeout_seconds":self.timeout,"requires":["git","candidate.provider","candidate.executable","candidate.model","candidate.effort"]})
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
        let _integration_lock = if kind == "integrate" {
            let target = self.owned(payload, "integration_workspace")?;
            Some(git_lock(
                target.parent().ok_or(HostError("invalid_workspace"))?,
            )?)
        } else {
            None
        };
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
                if ![
                    "prepare",
                    "inspect",
                    "gather_context",
                    "verify",
                    "integrate",
                ]
                .contains(&kind)
                {
                    return Err(HostError("recovery_required"));
                }
            }
            Err(_) => return Err(HostError("io_error")),
        }
        let result = match kind {
            "prepare" => self.prepare(action, payload),
            "launch" => self.launch(action, payload),
            "inspect" => self.inspect(payload),
            "verify" => self.verify(payload, id),
            "integrate" => self.integrate(action, payload),
            "gather_context" => self.gather(action, payload),
            _ => Err(HostError("unsupported_action")),
        };
        match result {
            Ok(v) => {
                write_atomic(&record, &json!({"fingerprint":fingerprint,"result":v}))?;
                if kind == "integrate" {
                    let target = self.owned(payload, "integration_workspace")?;
                    let journal = target
                        .parent()
                        .ok_or(HostError("invalid_workspace"))?
                        .join("integration-transaction.json");
                    if journal.exists() && value_file(&journal)?["action_id"] == id {
                        let _ = fs::remove_file(journal);
                    }
                }
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
        let task_name = string(task, "id")?;
        let attempt = p.get("attempt").and_then(Value::as_u64).unwrap_or(0);
        let taskid = if attempt == 0 {
            hash(task_name)
        } else {
            hash(&format!("{task_name}:attempt:{attempt}"))
        };
        let root = self.worktrees.join(&run);
        fs::create_dir_all(&root).map_err(|_| HostError("io_error"))?;
        let _lock = git_lock(&root)?;
        let pending = root.join("integration-transaction.json");
        if pending.exists() {
            let transaction = value_file(&pending)?;
            let id = string(&transaction, "action_id")?;
            if self
                .state
                .join("actions")
                .join(format!("{}.json", hash(id)))
                .exists()
            {
                fs::remove_file(pending).map_err(|_| HostError("io_error"))?;
            } else {
                return Err(HostError("recovery_required"));
            }
        }
        let base_file = root.join("base");
        if !base_file.exists() {
            fs::write(&base_file, &self.base).map_err(|_| HostError("io_error"))?;
        }
        let base = fs::read_to_string(&base_file).map_err(|_| HostError("io_error"))?;
        let integration = root.join("integration");
        let ib = format!("switchbite/{run}/integration");
        if !integration.exists() {
            self.add_worktree(&integration, &ib, &base)?;
        }
        validate_worktree(&integration, &ib, &self.workspace)?;
        let taskpath = root.join(&taskid);
        let branch = format!("switchbite/{run}/{taskid}");
        if !taskpath.exists() {
            let head = run_git(&integration, &["rev-parse", "HEAD"])?;
            self.add_worktree(&taskpath, &branch, &head)?;
        }
        validate_worktree(&taskpath, &branch, &self.workspace)?;
        Ok(
            json!({"status":"prepared","workspace":taskpath,"branch":branch,"integration_workspace":integration,"integration_branch":ib}),
        )
    }
    fn owned(&self, p: &Value, key: &str) -> Result<PathBuf, HostError> {
        let s = string(p.get("prepared").ok_or(HostError("invalid_action"))?, key)?;
        let path = fs::canonicalize(s).map_err(|_| HostError("invalid_workspace"))?;
        let root = fs::canonicalize(&self.worktrees).map_err(|_| HostError("invalid_workspace"))?;
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
        cursor: bool,
    ) -> Result<(bool, Vec<u8>, Option<&'static str>), HostError> {
        let logs = self.state.join("output");
        let execution_dir = self.state.join("executions");
        fs::create_dir_all(&logs).map_err(|_| HostError("io_error"))?;
        fs::create_dir_all(&execution_dir).map_err(|_| HostError("io_error"))?;
        let key = hash(tag);
        let cancel = execution_dir.join(format!("{key}.cancel"));
        let record_path = execution_dir.join(format!("{key}.json"));
        let out = logs.join(format!("{key}.out"));
        let err = logs.join(format!("{key}.err"));
        let now = unix_ms();
        let mut record = json!({"execution_id":tag,"token":key,"status":"running",
            "started_unix_ms":now,"updated_unix_ms":now,"finished_unix_ms":null,
            "stdout_bytes":0,"stderr_bytes":0,"cwd":dir,"kind":if launch {"worker"} else {"check"}});
        let publish = |record: &mut Value, status: &str, exit_code: Option<i32>| {
            let now = unix_ms();
            record["status"] = json!(status);
            record["updated_unix_ms"] = json!(now);
            record["stdout_bytes"] = json!(output_len(&out));
            record["stderr_bytes"] = json!(output_len(&err));
            if status != "running" {
                record["finished_unix_ms"] = json!(now);
                if let Some(code) = exit_code {
                    record["exit_code"] = json!(code);
                }
            }
            write_atomic(&record_path, record)
        };
        // Settle the record even when spawning or a later operation fails.
        let mut terminal = "failed";
        let mut exit_code = None;
        let result = (|| {
            if launch && cancel.exists() {
                terminal = "cancelled";
                return Err(HostError("cancelled"));
            }
            let stdout = fs::File::create(&out).map_err(|_| HostError("io_error"))?;
            let stderr = fs::File::create(&err).map_err(|_| HostError("io_error"))?;
            publish(&mut record, "running", None)?;
            cmd.current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr));
            let job_name = format!(
                "Local\\Switchbite-{}",
                hash(&format!("{}:{tag}", self.state.display()))
            );
            let mut tree = process::Tree::spawn(&mut cmd, &job_name)
                .map_err(|_| HostError("process_ownership_unavailable"))?;
            let identity = match (process::owner_creation_time(), tree.creation_time()) {
                (Ok(owner), Ok(worker)) => (owner, worker),
                _ => {
                    tree.terminate()
                        .map_err(|_| HostError("process_ownership_unavailable"))?;
                    return Err(HostError("process_ownership_unavailable"));
                }
            };
            record["pid"] = json!(tree.child.id());
            record["owner_pid"] = json!(std::process::id());
            record["owner_creation"] = json!(identity.0);
            record["worker_creation"] = json!(identity.1);
            record["job_name"] = json!(job_name);
            if publish(&mut record, "running", None).is_err() {
                tree.terminate()
                    .map_err(|_| HostError("process_ownership_unavailable"))?;
                return Err(HostError("io_error"));
            }
            let started = Instant::now();
            let mut heartbeat = started;
            loop {
                let status = tree
                    .child
                    .try_wait()
                    .map_err(|_| HostError("process_failed"))?
                    .filter(|_| tree.active().is_ok_and(|n| n == 0));
                if output_len(&out) > MAX_OUTPUT || output_len(&err) > MAX_OUTPUT {
                    tree.terminate()
                        .map_err(|_| HostError("process_ownership_unavailable"))?;
                    terminal = "output_limit";
                    retain_tail(&out)?;
                    retain_tail(&err)?;
                    return Err(HostError("output_limit"));
                }
                if let Some(status) = status {
                    exit_code = status.code();
                    let bytes = read_bounded(&out)?;
                    let stderr = read_bounded(&err)?;
                    if launch && cursor && cursor_weekly_limit(&stderr) {
                        return Ok((false, bytes, Some("rate_limited")));
                    }
                    terminal = if status.success() {
                        "completed"
                    } else {
                        "failed"
                    };
                    let hint = if status.success() {
                        None
                    } else {
                        failure_hint(&stderr)
                    };
                    return Ok((status.success(), bytes, hint));
                }
                if launch && cancel.exists() {
                    tree.terminate()
                        .map_err(|_| HostError("process_ownership_unavailable"))?;
                    terminal = "cancelled";
                    return Err(HostError("cancelled"));
                }
                if started.elapsed() >= Duration::from_secs(self.timeout) {
                    tree.terminate()
                        .map_err(|_| HostError("process_ownership_unavailable"))?;
                    terminal = "timeout";
                    return Err(HostError("timeout"));
                }
                if heartbeat.elapsed() >= Duration::from_millis(500) {
                    heartbeat = Instant::now();
                    publish(&mut record, "running", None)?;
                    if launch && cursor && cursor_weekly_limit(&read_bounded(&err)?) {
                        tree.terminate()
                            .map_err(|_| HostError("process_ownership_unavailable"))?;
                        return Ok((false, read_bounded(&out)?, Some("rate_limited")));
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
        })();
        if output_len(&out) > MAX_OUTPUT || output_len(&err) > MAX_OUTPUT {
            retain_tail(&out)?;
            retain_tail(&err)?;
            terminal = "output_limit";
            publish(&mut record, terminal, exit_code)?;
            return Err(HostError("output_limit"));
        }
        publish(&mut record, terminal, exit_code)?;
        result
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
            "Complete the following task only within this working directory. Treat outputs beginning with artifact: as references, not writable paths. On Windows use non-login shell commands (login=false) and PowerShell -NoProfile so profiles do not change the working directory or write outside it. Task: {}",
            serde_json::to_string(task).map_err(|_| HostError("invalid_action"))?
        );
        if prompt.len() > 128 * 1024 {
            return Err(HostError("invalid_action"));
        }
        if c.get("args")
            .and_then(Value::as_array)
            .is_some_and(|args| !args.is_empty())
        {
            return Err(HostError("unsupported_candidate_args"));
        }
        let id = string(a, "id")?;
        let session = session_id(id);
        let messages = self.state.join("messages");
        fs::create_dir_all(&messages).map_err(|_| HostError("io_error"))?;
        let message_path = messages.join(format!("{}.txt", hash(id)));
        let mut cmd = workers::command(
            provider,
            executable,
            model,
            effort,
            &session,
            &message_path,
            &dir,
            &prompt,
        )?;
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
                || [
                    "CODEX_THREAD_ID",
                    "CODEX_SESSION_ID",
                    "CODEX_TOOL_CALL_ID",
                    "CODEX_PERMISSION_PROFILE",
                    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
                ]
                .contains(&name.as_str())
            {
                cmd.env_remove(key);
            }
        }
        let (ok, output, stderr_hint) = match self.execute(
            cmd,
            &dir,
            id,
            true,
            provider == "cursor",
        ) {
            Ok(result) => result,
            Err(HostError("cancelled")) => {
                return Ok(
                    json!({"status":"failed","execution_id":id,"completion_verified":false,"cancelled":true}),
                );
            }
            Err(error) => return Err(error),
        };
        let (verified, resolved_model) = verified_receipt(provider, &output, &session);
        if !message_path.exists()
            && let Some(message) = workers::message(provider, &output)
        {
            fs::write(&message_path, message).map_err(|_| HostError("io_error"))?;
        }
        // Private context pointers let the coordinator inspect a failed attempt
        // and prepare a repair. Browser summaries never expose these raw logs.
        let stream_path = messages.join(format!("{}.jsonl", hash(id)));
        fs::write(&stream_path, &output).map_err(|_| HostError("io_error"))?;
        let mut result = json!({"status":if ok && verified {"completed"} else {"failed"},"execution_id":id,"completion_verified":ok && verified,"requested_model":model,"resolved_model":resolved_model,"message_ref":if message_path.exists(){Some(message_path)}else{None},"stream_ref":stream_path,"reason":if !ok {Some(stderr_hint.or_else(|| failure_hint(&output)).unwrap_or("provider_exit_failed"))} else if !verified {Some("provider_receipt_invalid")} else {None}});
        if ok && verified && provider == "antigravity" {
            result["session_id"] = json!(workers::antigravity_session(&output));
        }
        attach_rate_limit_error(&mut result);
        if stderr_hint == Some("rate_limited") {
            result["stderr_ref"] =
                json!(self.state.join("output").join(format!("{}.err", hash(id))));
        }
        Ok(result)
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
        let execution = self
            .state
            .join("executions")
            .join(format!("{}.json", hash(id)));
        if execution.exists() {
            let saved = value_file(&execution)?;
            let expected_job = format!(
                "Local\\Switchbite-{}",
                hash(&format!("{}:{id}", self.state.display()))
            );
            if saved["execution_id"] == id
                && saved["token"] == hash(id)
                && saved["job_name"] == expected_job
            {
                let pid = saved["pid"].as_u64().and_then(|v| u32::try_from(v).ok());
                let owner = saved["owner_pid"]
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok());
                let worker_time = saved["worker_creation"].as_u64();
                let owner_time = saved["owner_creation"].as_u64();
                if let (Some(pid), Some(owner), Some(worker_time), Some(owner_time)) =
                    (pid, owner, worker_time, owner_time)
                    && process::stopped(owner, owner_time) == Some(true)
                    && process::stopped(pid, worker_time) == Some(true)
                    && process::job_drained(&expected_job) == Some(true)
                {
                    return Ok(
                        json!({"status":"failed","execution_id":id,"completion_verified":false,"reason":"owner_stopped"}),
                    );
                }
            }
        }
        if p.get("operation").and_then(Value::as_str) == Some("cancel") {
            let executions = self.state.join("executions");
            fs::create_dir_all(&executions).map_err(|_| HostError("io_error"))?;
            fs::write(executions.join(format!("{}.cancel", hash(id))), b"cancel")
                .map_err(|_| HostError("io_error"))?;
        }
        Ok(json!({"status":"unknown","execution_id":id,"completion_verified":false}))
    }
    fn verify(&self, p: &Value, action_id: &str) -> Result<Value, HostError> {
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
        if let Some(outputs) = p["task"]["outputs"].as_array() {
            for output in outputs {
                let name = output.as_str().ok_or(HostError("invalid_action"))?;
                if name.starts_with("artifact:")
                    || name.contains("://")
                    || name.contains('*')
                    || name.contains('?')
                {
                    continue;
                }
                let relative = Path::new(name);
                if name != "."
                    && (relative.is_absolute()
                        || relative
                            .components()
                            .any(|c| !matches!(c, std::path::Component::Normal(_))))
                {
                    return Err(HostError("invalid_output_path"));
                }
                let exists =
                    fs::canonicalize(dir.join(relative)).is_ok_and(|p| p.starts_with(&dir));
                passed &= exists;
                evidence.push(json!({"output":name,"status":if exists {"passed"} else {"failed"},"reason":if exists {None} else {Some("output_missing")}}));
            }
        }
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
            let outcome = self.execute(cmd, &dir, &format!("check-{action_id}-{i}"), false, false);
            let ok = matches!(&outcome, Ok((true, _, _)));
            passed &= ok;
            let reason = match outcome {
                Ok((true, _, _)) => None,
                Ok((false, _, hint)) => Some(hint.unwrap_or("check_exit_failed")),
                Err(error) => Some(error.0),
            };
            evidence
                .push(json!({"check":i,"status":if ok {"passed"} else {"failed"},"reason":reason}));
        }
        Ok(json!({"status":if passed {"passed"} else {"failed"},"evidence":evidence}))
    }
    fn integrate(&self, action: &Value, p: &Value) -> Result<Value, HostError> {
        let task = self.owned(p, "workspace")?;
        let target = self.owned(p, "integration_workspace")?;
        if task.parent() != target.parent() || task == target {
            return Err(HostError("invalid_workspace"));
        }
        let action_id = string(action, "id")?;
        let journal = target
            .parent()
            .ok_or(HostError("invalid_workspace"))?
            .join("integration-transaction.json");
        if journal.exists() {
            let saved = value_file(&journal)?;
            if saved["action_id"] != action_id {
                let old = saved["action_id"]
                    .as_str()
                    .ok_or(HostError("invalid_state"))?;
                let receipt = self
                    .state
                    .join("actions")
                    .join(format!("{}.json", hash(old)));
                if receipt.exists() {
                    fs::remove_file(&journal).map_err(|_| HostError("io_error"))?;
                } else {
                    return Err(HostError("recovery_required"));
                }
            }
        }
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
        let current = run_git(&target, &["rev-parse", "HEAD"])?;
        let target_head = if journal.exists() {
            let saved = value_file(&journal)?;
            if saved["action_id"] != action_id || saved["task_commit"] != head {
                return Err(HostError("recovery_required"));
            }
            string(&saved, "base_commit")?.to_owned()
        } else {
            write_atomic(
                &journal,
                &json!({"action_id":action_id,"base_commit":current,"task_commit":head}),
            )?;
            current.clone()
        };
        if current != target_head
            && (run_git(
                &target,
                &["merge-base", "--is-ancestor", &target_head, &current],
            )
            .is_err()
                || run_git(&target, &["merge-base", "--is-ancestor", &head, &current]).is_err())
        {
            return Err(HostError("recovery_required"));
        }
        if current == target_head
            && head != target_head
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
            .verify(&integration_payload, string(action, "id")?)
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
    fn gather(&self, action: &Value, p: &Value) -> Result<Value, HostError> {
        let task = p.get("task").ok_or(HostError("invalid_action"))?;
        let refs = task
            .get("context_refs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if refs.len() > 64 || refs.iter().any(|r| !r.is_string()) {
            return Err(HostError("invalid_action"));
        }
        let run = hash(string(action, "run_id")?);
        let integrated = self.worktrees.join(&run).join("integration");
        let _lock = if integrated.exists() {
            let root = integrated.parent().ok_or(HostError("invalid_workspace"))?;
            let lock = git_lock(root)?;
            let pending = root.join("integration-transaction.json");
            if pending.exists() {
                let transaction = value_file(&pending)?;
                let id = string(&transaction, "action_id")?;
                if !self
                    .state
                    .join("actions")
                    .join(format!("{}.json", hash(id)))
                    .exists()
                {
                    return Err(HostError("recovery_required"));
                }
            }
            Some(lock)
        } else {
            None
        };
        let integration_root = if integrated.exists() {
            validate_worktree(
                &integrated,
                &format!("switchbite/{run}/integration"),
                &self.workspace,
            )?;
            Some(fs::canonicalize(&integrated).map_err(|_| HostError("context_missing"))?)
        } else {
            None
        };
        let source_commit = run_git(
            integration_root.as_deref().unwrap_or(&self.workspace),
            &["rev-parse", "HEAD"],
        )?;
        let mut sources = Vec::new();
        for r in &refs {
            let reference = r.as_str().ok_or(HostError("invalid_action"))?;
            if reference.starts_with("artifact:") {
                sources.push(json!({"reference":reference,"kind":"artifact"}));
                continue;
            }
            let rel = Path::new(reference);
            let in_integration = integration_root.as_ref().and_then(|root| {
                fs::canonicalize(root.join(rel))
                    .ok()
                    .filter(|p| p.starts_with(root) && p.is_file())
            });
            let in_original = || {
                fs::canonicalize(self.workspace.join(rel))
                    .ok()
                    .filter(|p| p.starts_with(&self.workspace) && p.is_file())
            };
            if in_integration.is_some() {
                sources.push(
                    json!({"reference":reference,"kind":"integration","commit":source_commit}),
                );
            } else if in_original().is_some() {
                sources.push(json!({"reference":reference,"kind":"workspace","commit":run_git(&self.workspace, &["rev-parse", "HEAD"])?,"working_tree":true}));
            } else {
                return Err(HostError("context_missing"));
            }
        }
        Ok(
            json!({"status":"gathered","context_refs":refs,"source_commit":source_commit,"sources":sources}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::{cursor_weekly_limit, verified_receipt};
    #[test]
    fn cursor_weekly_limit_requires_the_definite_error() {
        assert!(cursor_weekly_limit(b"ActionRequiredError: Weekly usage limit reached. It resets in 1 day. Buy credits to keep going now."));
        assert!(!cursor_weekly_limit(b"Weekly usage limit reached."));
        assert!(!cursor_weekly_limit(
            b"ActionRequiredError: temporary rate limit"
        ));
    }
    #[cfg(windows)]
    #[test]
    fn cursor_quota_terminates_owned_job_and_preserves_output() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let host = LocalHost {
            workspace: temp.path().to_owned(),
            state: temp.path().to_owned(),
            worktrees: temp.path().to_owned(),
            base: String::new(),
            timeout: 20,
        };
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-Command", "[Console]::Out.WriteLine('partial'); [Console]::Error.WriteLine('ActionRequiredError: Weekly usage limit reached. It resets in 1 day.'); Start-Sleep -Seconds 15"]);
        let start = Instant::now();
        let (ok, output, hint) = host
            .execute(cmd, temp.path(), "quota-test", true, true)
            .unwrap();
        assert!(!ok);
        assert_eq!(hint, Some("rate_limited"));
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(String::from_utf8_lossy(&output).contains("partial"));
        assert!(cursor_weekly_limit(
            &read_bounded(
                &temp
                    .path()
                    .join("output")
                    .join(format!("{}.err", hash("quota-test")))
            )
            .unwrap()
        ));
    }
    #[cfg(windows)]
    #[test]
    fn normal_provider_quota_failure_exposes_workflow_error_code() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let host = LocalHost {
            workspace: temp.path().to_owned(),
            state: temp.path().to_owned(),
            worktrees: temp.path().to_owned(),
            base: String::new(),
            timeout: 20,
        };
        let mut cmd = Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-Command",
            "[Console]::Error.WriteLine('rate limit exceeded'); exit 1",
        ]);
        let (ok, output, hint) = host
            .execute(cmd, temp.path(), "normal-quota-test", true, false)
            .unwrap();
        assert!(!ok);
        let reason = hint
            .or_else(|| failure_hint(&output))
            .unwrap_or("provider_exit_failed");
        let mut result = json!({"status":"failed", "reason":reason});
        attach_rate_limit_error(&mut result);
        assert_eq!(result["error"]["code"], "quota_or_rate_limit");
    }
    #[cfg(windows)]
    #[test]
    fn live_check_progress_and_terminal_output_are_visible() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let worker_root = root.clone();
        let handle = thread::spawn(move || {
            let host = LocalHost {
                workspace: worker_root.clone(),
                state: worker_root.clone(),
                worktrees: worker_root.clone(),
                base: String::new(),
                timeout: 10,
            };
            let mut cmd = Command::new("powershell.exe");
            cmd.args(["-NoProfile", "-Command", "[Console]::Out.WriteLine('live'); [Console]::Error.WriteLine('error'); Start-Sleep -Seconds 2"]);
            host.execute(cmd, &worker_root, "live-check", false, false)
                .unwrap()
        });
        let record_path = root
            .join("executions")
            .join(format!("{}.json", hash("live-check")));
        let out = root
            .join("output")
            .join(format!("{}.out", hash("live-check")));
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut observed = false;
        while Instant::now() < deadline {
            if let Ok(record) = value_file(&record_path)
                && record["status"] == "running"
                && record["kind"] == "check"
                && record["pid"].is_number()
                && output_len(&out) > 0
            {
                assert_eq!(execution_alive(&record), Some(true));
                observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(observed);
        assert!(handle.join().unwrap().0);
        let record = value_file(&record_path).unwrap();
        assert_eq!(record["status"], "completed");
        assert!(record["finished_unix_ms"].is_number());
        assert!(String::from_utf8_lossy(&fs::read(out).unwrap()).contains("live"));
        assert!(record["stderr_bytes"].as_u64().unwrap() > 0);
    }
    #[cfg(windows)]
    #[test]
    fn timeout_and_cancel_keep_output_and_terminal_records() {
        use super::*;
        for cancel in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().to_owned();
            let worker_root = root.clone();
            let handle = thread::spawn(move || {
                let host = LocalHost {
                    workspace: worker_root.clone(),
                    state: worker_root.clone(),
                    worktrees: worker_root.clone(),
                    base: String::new(),
                    timeout: 2,
                };
                let mut cmd = Command::new("powershell.exe");
                cmd.args(["-NoProfile", "-Command", "[Console]::Out.WriteLine('partial'); [Console]::Error.WriteLine('detail'); Start-Sleep -Seconds 15"]);
                host.execute(cmd, &worker_root, "stopped", true, false)
                    .unwrap_err()
                    .0
            });
            let out = root.join("output").join(format!("{}.out", hash("stopped")));
            let deadline = Instant::now() + Duration::from_secs(8);
            while output_len(&out) == 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            assert!(output_len(&out) > 0);
            if cancel {
                fs::write(
                    root.join("executions")
                        .join(format!("{}.cancel", hash("stopped"))),
                    b"cancel",
                )
                .unwrap();
            }
            let expected = if cancel { "cancelled" } else { "timeout" };
            assert_eq!(handle.join().unwrap(), expected);
            let record = value_file(
                &root
                    .join("executions")
                    .join(format!("{}.json", hash("stopped"))),
            )
            .unwrap();
            assert_eq!(record["status"], expected);
            assert!(String::from_utf8_lossy(&fs::read(out).unwrap()).contains("partial"));
            assert!(record["stderr_bytes"].as_u64().unwrap() > 0);
        }
    }
    #[cfg(windows)]
    #[test]
    fn rapid_output_limit_retains_bounded_tails() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let host = LocalHost {
            workspace: root.to_owned(),
            state: root.to_owned(),
            worktrees: root.to_owned(),
            base: String::new(),
            timeout: 10,
        };
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-Command", "[Console]::Out.Write(('x' * 3000000)); [Console]::Error.Write(('e' * 3000000)); Start-Sleep -Seconds 5"]);
        assert_eq!(
            host.execute(cmd, root, "flood", false, false)
                .unwrap_err()
                .0,
            "output_limit"
        );
        let record = value_file(
            &root
                .join("executions")
                .join(format!("{}.json", hash("flood"))),
        )
        .unwrap();
        assert_eq!(record["status"], "output_limit");
        for ext in ["out", "err"] {
            assert!(
                output_len(
                    &root
                        .join("output")
                        .join(format!("{}.{}", hash("flood"), ext))
                ) <= 64 * 1024
            );
        }
    }
    #[test]
    fn oversized_output_keeps_only_tail() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("output");
        let mut bytes = vec![b'a'; 1024 * 1024];
        bytes.extend_from_slice(b"tail");
        fs::write(&path, bytes).unwrap();
        retain_tail(&path).unwrap();
        let retained = fs::read(path).unwrap();
        assert_eq!(retained.len(), 64 * 1024);
        assert!(retained.ends_with(b"tail"));
    }
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
    #[test]
    fn recovered_tool_failure_keeps_terminal_success_for_artifact_verification() {
        let stream=b"{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n{\"type\":\"turn.started\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"status\":\"failed\",\"exit_code\":1}}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Created the required artifact with another tool\"}}\n{\"type\":\"turn.completed\"}\n";
        assert!(verified_receipt("codex", stream, "unused").0);
        assert!(!verified_receipt("codex",b"{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n{\"type\":\"turn.started\"}\n{\"type\":\"turn.failed\"}\n","unused").0);
    }
}

#[cfg(all(test, windows))]
mod codex_shell_tests {
    use super::*;
    #[test]
    fn prefers_portable_pwsh_and_falls_back_without_store_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let alias = temp.path().join("WindowsApps");
        let portable = temp.path().join("portable");
        let legacy = temp.path().join("legacy");
        for dir in [&alias, &portable, &legacy] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(alias.join("pwsh.exe"), b"alias").unwrap();
        fs::write(portable.join("pwsh.exe"), b"portable").unwrap();
        fs::write(legacy.join("powershell.exe"), b"legacy").unwrap();
        let path = std::env::join_paths([&alias, &legacy, &portable]).unwrap();
        let adjusted = codex_shell_path(&path).unwrap();
        let dirs: Vec<_> = std::env::split_paths(&adjusted).collect();
        assert_eq!(dirs[0], portable);
        fs::remove_file(portable.join("pwsh.exe")).unwrap();
        let adjusted = codex_shell_path(&path).unwrap();
        let dirs: Vec<_> = std::env::split_paths(&adjusted).collect();
        assert_eq!(dirs[0], legacy);
        assert!(!dirs.contains(&alias));
    }
}
