use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MODEL_TTL: u64 = 86_400_000;
const PROVIDERS: [&str; 4] = ["claude", "codex", "cursor", "opencode"];
#[derive(Debug)]
pub struct CatalogError(&'static str);
impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for CatalogError {}
impl CatalogError {
    pub fn to_value(&self) -> Value {
        json!({"code":self.0,"message":self.0})
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn valid_model(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.:/".contains(&c))
}
fn host_scope() -> String {
    hash(&format!(
        "{}:{}",
        std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_default(),
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default()
    ))[..16]
        .to_owned()
}
fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}
fn executable(provider: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let app = std::env::var_os("APPDATA").map(PathBuf::from);
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let special = match provider {
            "codex" => app.map(|p| p.join("npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe")),
            "opencode" => app.map(|p| p.join("npm/node_modules/@opencode/cli/bin/opencode.exe")),
            "cursor" => local.map(|p| p.join("cursor-agent/cursor-agent.cmd")),
            _ => None,
        };
        if let Some(p) = special.filter(|p| p.is_file()) {
            return fs::canonicalize(p).ok();
        }
    }
    let name = if provider == "cursor" {
        "cursor-agent"
    } else {
        provider
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path).filter(|p| p.is_absolute()) {
        #[cfg(windows)]
        let names = [format!("{name}.exe"), format!("{name}.cmd")];
        #[cfg(not(windows))]
        let names = [name.to_string()];
        for name in names {
            let p = dir.join(name);
            if p.is_file() {
                return fs::canonicalize(p).ok();
            }
        }
    }
    None
}
// Drain both pipes even after the capture limit, and kill a slow probe.
fn probe(exe: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buf = [0; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            let take = n.min(262_144usize.saturating_sub(captured.len()));
            captured.extend_from_slice(&buf[..take]);
        }
        let _ = sender.send(captured);
    });
    thread::spawn(move || {
        let mut buf = [0; 4096];
        while matches!(stderr.read(&mut buf), Ok(n) if n > 0) {}
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let bytes = receiver
                    .recv_timeout(Duration::from_secs(5).saturating_sub(started.elapsed()))
                    .ok()?;
                return String::from_utf8(bytes).ok();
            }
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                thread::sleep(Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
fn configured_effort() -> Option<String> {
    let base = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|p| p.join(".codex")))?;
    let text = fs::read_to_string(base.join("config.toml")).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let effort = doc.get("model_reasoning_effort")?.as_str()?;
    ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"]
        .contains(&effort)
        .then(|| effort.to_owned())
}
fn configured_model(provider: &str) -> Option<String> {
    let h = home()?;
    let value = match provider {
        "claude" => {
            let v: Value =
                serde_json::from_slice(&fs::read(h.join(".claude/settings.json")).ok()?).ok()?;
            v.get("model")?.as_str()?.to_owned()
        }
        "codex" => {
            let p = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| h.join(".codex"));
            let text = fs::read_to_string(p.join("config.toml")).ok()?;
            let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
            doc.get("model")?.as_str()?.to_owned()
        }
        _ => return None,
    };
    valid_model(&value).then_some(value)
}
fn candidate(
    provider: &str,
    exe: &Path,
    model: &str,
    effort: &str,
    source: &str,
    observed: u64,
    scope: &str,
) -> Value {
    let identity = format!("{provider}|{model}|{effort}|{scope}");
    let id = format!("{provider}:{}", &hash(&identity)[..32]);
    let role = match provider {
        "claude" => "Implementation, refactoring and bug fixes",
        "codex" => "Orchestration, planning and independent review",
        "cursor" => "General coding, exploration and debugging",
        _ => "Explicit model and custom agent work",
    };
    json!({"id":id,"description":format!("{role}; {provider} model {model}, {effort} effort."),"capabilities":["file.read","file.write","code.change","code.review","html.write","data.write","writing","code.explore"],"available":true,"provider":provider,"executable":exe.to_string_lossy(),"model":model,"effort":effort,"resource_key":format!("{provider}:{scope}"),"catalog_info":{"source":source,"readiness":"unverified","observed_at_ms":observed}})
}
fn models_from_output(provider: &str, output: &str) -> Vec<String> {
    let mut models = BTreeSet::new();
    for line in output.lines().take(4096) {
        let first = line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| c == '*' || c == '-');
        let valid = valid_model(first)
            && match provider {
                "opencode" => first.contains('/'),
                "cursor" => line.contains(" - ") && !first.contains('/'),
                _ => false,
            };
        if valid {
            models.insert(first.to_string());
        }
    }
    models.into_iter().collect()
}
fn codex_models(value: &Value) -> Vec<(String, String, u64)> {
    let mut result = Vec::new();
    if let Some(models) = value.get("models").and_then(Value::as_array) {
        for model in models {
            if model.get("visibility").and_then(Value::as_str) != Some("list") {
                continue;
            }
            let Some(name) = model
                .get("slug")
                .and_then(Value::as_str)
                .filter(|s| valid_model(s))
            else {
                continue;
            };
            let priority = model
                .get("priority")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            if let Some(levels) = model
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
            {
                for level in levels {
                    if let Some(effort) = level.get("effort").and_then(Value::as_str).filter(|e| {
                        ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"].contains(e)
                    }) {
                        result.push((name.to_owned(), effort.to_owned(), priority));
                    }
                }
            }
        }
    }
    result
}
fn codex_cache() -> Vec<(String, String, u64)> {
    let Some(base) = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|p| p.join(".codex")))
    else {
        return Vec::new();
    };
    fs::read(base.join("models_cache.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .map(|v| codex_models(&v))
        .unwrap_or_default()
}
// Replace the complete cache file in one operation. Concurrent writers use distinct names.
fn write_cache(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        drop(f);
        replace_file(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}
#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
/// Discover installed local CLIs; cached data never supplies executable paths.
pub fn snapshot(workspace: &Path, selection: &Value) -> Result<Value, CatalogError> {
    if !selection.is_object() {
        return Err(CatalogError("invalid_selection"));
    }
    if workspace.join("ARCHIVED.json").exists() {
        return Err(CatalogError("workspace_archived"));
    }
    let requested: Vec<&str> = match selection.get("providers") {
        None => PROVIDERS.to_vec(),
        Some(Value::Array(a)) => {
            let mut v = Vec::new();
            for x in a {
                let p = x
                    .as_str()
                    .filter(|s| PROVIDERS.contains(s))
                    .ok_or(CatalogError("invalid_provider"))?;
                if !v.contains(&p) {
                    v.push(p);
                }
            }
            v
        }
        _ => return Err(CatalogError("invalid_selection")),
    };
    let refresh = match selection.get("refresh") {
        None => false,
        Some(Value::Bool(b)) => *b,
        _ => return Err(CatalogError("invalid_selection")),
    };
    let timestamp = now();
    let scope = host_scope();
    let dir = workspace.join(".switchbite/catalog");
    let mut candidates = Vec::new();
    let mut diagnostics = Vec::new();
    for provider in requested {
        let Some(exe) = executable(provider) else {
            diagnostics.push(json!({"provider":provider,"code":"not_installed"}));
            continue;
        };
        let version = probe(&exe, &["--version"]);
        if version.is_none() {
            diagnostics.push(json!({"provider":provider,"code":"version_unverified"}));
        }
        let version = version
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(128)
            .collect::<String>();
        let cache_path = dir.join(format!("{provider}.json"));
        let cached: Option<Value> = fs::read(&cache_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let fresh = !refresh
            && cached.as_ref().is_some_and(|v| {
                v["version"] == version
                    && v["scope"] == scope
                    && v["observed_at_ms"]
                        .as_u64()
                        .is_some_and(|t| timestamp >= t && timestamp - t < MODEL_TTL)
            });
        let mut models: Vec<(String, String)> = Vec::new();
        if let Some(m) = configured_model(provider) {
            models.push((m, "user_config".into()));
        }
        if provider == "claude" {
            models.push(("sonnet".into(), "official_alias".into()));
            models.push(("opus".into(), "official_alias".into()));
        }
        if provider == "cursor" || provider == "opencode" {
            let discovered = if fresh {
                cached
                    .as_ref()
                    .and_then(|v| v["models"].as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .filter(|s| valid_model(s))
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                let args: &[&str] = if provider == "cursor" {
                    &["--list-models"]
                } else {
                    &["models", "--standalone"]
                };
                match probe(&exe, args) {
                    Some(out) => models_from_output(provider, &out),
                    None => {
                        diagnostics.push(json!({"provider":provider,"code":"model_probe_failed"}));
                        Vec::new()
                    }
                }
            };
            if !fresh && !discovered.is_empty() {
                let _ = fs::create_dir_all(&dir);
                let bytes = serde_json::to_vec(&json!({"version":version,"scope":scope,"observed_at_ms":timestamp,"models":discovered})).unwrap_or_default();
                if write_cache(&cache_path, &bytes).is_err() {
                    diagnostics.push(json!({"provider":provider,"code":"cache_write_failed"}));
                }
            }
            for m in discovered {
                models.push((m, if fresh { "cached_cli" } else { "cli" }.into()));
            }
        }
        models.sort();
        models.dedup_by(|a, b| a.0 == b.0);
        if models.is_empty() {
            diagnostics.push(json!({"provider":provider,"code":"no_models_discovered"}));
        }
        let configured = if provider == "codex" {
            configured_effort()
        } else {
            None
        };
        for (model, source) in models {
            candidates.push(candidate(
                provider, &exe, &model, "default", &source, timestamp, &scope,
            ));
            if source == "user_config" {
                if let Some(effort) = &configured {
                    candidates.push(candidate(
                        provider, &exe, &model, effort, &source, timestamp, &scope,
                    ));
                }
            }
        }
        if provider == "codex" {
            for (model, effort, priority) in codex_cache() {
                let mut entry = candidate(
                    provider,
                    &exe,
                    &model,
                    &effort,
                    "codex_models_cache",
                    timestamp,
                    &scope,
                );
                entry["catalog_info"]["priority"] = json!(priority);
                candidates.push(entry);
            }
        }
        diagnostics.push(json!({"provider":provider,"code":"readiness_unverified"}));
    }
    candidates.sort_by(|a, b| candidate_order(a, b));
    let mut seen = BTreeSet::new();
    candidates.retain(|c| {
        c["id"]
            .as_str()
            .is_some_and(|id| seen.insert(id.to_owned()))
    });
    if candidates.len() > 1024 {
        diagnostics.push(json!({"provider":"all","code":"catalog_limit","total":candidates.len(),"limit":1024,"message":"Catalog is limited to 1024 candidates; narrow selection.providers to inspect individual providers."}));
    }
    candidates.truncate(1024);
    let revision = hash(
        &serde_json::to_string(
            &candidates
                .iter()
                .map(|c| (&c["id"], &c["model"], &c["executable"]))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default(),
    );
    Ok(
        json!({"schema_version":1,"revision":revision,"observed_at_ms":timestamp,"candidates":candidates,"diagnostics":diagnostics}),
    )
}
fn candidate_order(a: &Value, b: &Value) -> std::cmp::Ordering {
    let key = |v: &Value| {
        let info = &v["catalog_info"];
        let configured = info["source"] == "user_config";
        let configured_default = configured && v["effort"] == "default";
        let cli_default = v["provider"] == "cursor" && v["model"] == "auto";
        (
            !configured,
            configured_default,
            !cli_default,
            info["priority"].as_u64().unwrap_or(u64::MAX),
            v["model"].as_str().unwrap_or("").to_owned(),
            v["effort"].as_str().unwrap_or("").to_owned(),
            v["id"].as_str().unwrap_or("").to_owned(),
        )
    };
    key(a).cmp(&key(b))
}
/// Return deterministic, diverse candidates that meet known requirements and exact pins.
pub fn shortlist(
    snapshot: &Value,
    task: &Value,
    preferences: &str,
    limit: usize,
) -> Result<Vec<Value>, CatalogError> {
    let candidates = snapshot
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or(CatalogError("invalid_snapshot"))?;
    let requirements = task
        .get("requirements")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let routing = task.get("routing").and_then(Value::as_object);
    let fields = [
        ("candidate_id", "id"),
        ("provider", "provider"),
        ("model", "model"),
        ("effort", "effort"),
    ];
    let pins: Vec<_> = fields
        .iter()
        .filter_map(|(k, v)| routing.and_then(|r| r.get(*k)).map(|p| (*v, p)))
        .collect();
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for c in candidates {
        if c["available"] != true
            || !requirements
                .iter()
                .all(|r| c["capabilities"].as_array().is_some_and(|a| a.contains(r)))
            || !pins.iter().all(|(key, value)| c.get(*key) == Some(*value))
        {
            continue;
        }
        let Some(p) = c["provider"].as_str() else {
            continue;
        };
        groups.entry(p.to_owned()).or_default().push(c.clone());
    }
    for list in groups.values_mut() {
        list.sort_by(candidate_order);
    }
    let pref = preferences.to_lowercase();
    let objective = task["objective"].as_str().unwrap_or("").to_lowercase();
    let mut order: Vec<String> = groups.keys().cloned().collect();
    order.sort_by_key(|p| {
        let mentioned = pref.contains(p);
        let role = (p == "claude"
            && (objective.contains("implement") || objective.contains("fix")))
            || (p == "codex" && (objective.contains("review") || objective.contains("plan")));
        (!mentioned, !role, p.clone())
    });
    let mut result = Vec::new();
    let max = limit.min(16);
    while result.len() < max {
        let mut progress = false;
        for p in &order {
            if let Some(list) = groups.get_mut(p) {
                if !list.is_empty() {
                    result.push(list.remove(0));
                    progress = true;
                    if result.len() == max {
                        break;
                    }
                }
            }
        }
        if !progress {
            break;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovered_workers_support_review_and_html_tasks() {
        let entry = candidate(
            "codex",
            Path::new("worker"),
            "model",
            "high",
            "user_config",
            0,
            "host",
        );
        for capability in ["code.review", "html.write"] {
            let matches = shortlist(
                &json!({"candidates":[entry]}),
                &json!({"requirements":[capability]}),
                "",
                16,
            )
            .unwrap();
            assert_eq!(matches.len(), 1);
        }
    }
    #[test]
    fn codex_cache_only_admits_listed_models_and_reported_efforts() {
        let input = json!({"models":[
            {"slug":"z-model","visibility":"list","priority":4,"supported_reasoning_levels":[{"effort":"high"},{"effort":"ultra"}]},
            {"slug":"hidden","visibility":"hide","supported_reasoning_levels":[{"effort":"low"}]},
            {"slug":"bad name","visibility":"list","supported_reasoning_levels":[{"effort":"high"}]}
        ]});
        assert_eq!(
            codex_models(&input),
            vec![
                ("z-model".into(), "high".into(), 4),
                ("z-model".into(), "ultra".into(), 4)
            ]
        );
    }
    #[test]
    fn cache_replacement_never_leaves_partial_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        write_cache(&path, br#"{"version":1}"#).unwrap();
        write_cache(&path, br#"{"version":2}"#).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap()["version"],
            2
        );
    }
}
