//! Follow official StackBite releases from SwitchBite. Upstream repositories are read-only.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
static UPDATE: Mutex<()> = Mutex::new(());
const ADAPTER: &str = include_str!("../integrations/stackbite-engine/src/main.rs");
const REPOSITORY: &str = "Shinick-Han/stackbite";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn write(path: &Path, value: &Value) -> Result<()> {
    fs::create_dir_all(path.parent().ok_or("invalid_update_path")?)?;
    crate::catalog::write_cache(path, &serde_json::to_vec(value)?)?;
    Ok(())
}
fn read(path: &Path) -> Result<Value> {
    let mut data = Vec::new();
    fs::File::open(path)?.take(65537).read_to_end(&mut data)?;
    if data.len() > 65536 {
        return Err("update_record_too_large".into());
    }
    Ok(serde_json::from_slice(&data)?)
}
fn config(root: &Path) -> Result<Option<Value>> {
    let path = root.join("stackbite-follow.json");
    if path.exists() {
        Ok(Some(read(&path)?))
    } else {
        Ok(None)
    }
}
fn sha(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn valid_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() < 64
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
}
fn resolved(path: &Path) -> Result<PathBuf> {
    // Resolve existing ancestors before any writes, including a not-yet-created runtime home.
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("runtime_path_must_not_contain_parent_components".into());
    }
    let mut ancestor = path;
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(ancestor.file_name().ok_or("invalid_runtime_path")?);
        ancestor = ancestor.parent().ok_or("invalid_runtime_path")?;
    }
    let mut result = ancestor.canonicalize()?;
    for part in tail.iter().rev() {
        result.push(part);
    }
    #[cfg(windows)]
    {
        let text = result.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{unc}")));
        }
        if let Some(drive) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(drive));
        }
    }
    Ok(result)
}
fn separate_cache(root: &Path, source: &Path) -> Result<()> {
    if resolved(root)?.starts_with(resolved(source)?) {
        return Err("runtime_home_must_be_outside_stackbite_source".into());
    }
    Ok(())
}
fn checked_manifest(document: Value, tag: &str, asset_hash: &str) -> Result<Value> {
    let value = &document["payload"];
    let version = value["version"]
        .as_str()
        .ok_or("missing_stackbite_version")?;
    let commit = value["commit"].as_str().ok_or("missing_stackbite_commit")?;
    if value["schema"] != "stackbite-update-v1"
        || value["sha256"].as_str() != Some(asset_hash)
        || !valid_version(version)
        || tag != format!("v{version}")
        || !valid_commit(commit)
    {
        return Err("invalid_official_stackbite_manifest".into());
    }
    Ok(json!({"version":version,"commit":commit.to_ascii_lowercase(),"tag":tag}))
}
fn official_latest(root: &Path) -> Result<Value> {
    // Poll the release redirect; use the rate-limited API only when the tag changes.
    let response = reqwest::blocking::Client::builder()
        .user_agent("SwitchBite")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()?
        .head(format!("https://github.com/{REPOSITORY}/releases/latest"))
        .send()?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|h| h.to_str().ok())
        .ok_or("latest_release_redirect_missing")?;
    let tag = location
        .strip_prefix(&format!("https://github.com/{REPOSITORY}/releases/tag/"))
        .ok_or("invalid_release_redirect")?;
    if !tag.starts_with('v') || !valid_version(&tag[1..]) {
        return Err("invalid_release_tag".into());
    }
    let cache = root.join("stackbite-official-release.json");
    if let Ok(cached) = read(&cache)
        && cached["tag"] == tag
        && cached["commit"].as_str().is_some_and(valid_commit)
        && cached["version"].as_str() == Some(&tag[1..])
    {
        return Ok(cached);
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("SwitchBite/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;
    fn get(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        client
            .get(url)
            .send()?
            .error_for_status()?
            .take(512 * 1024 + 1)
            .read_to_end(&mut data)?;
        if data.len() > 512 * 1024 {
            return Err("release_metadata_too_large".into());
        }
        Ok(data)
    }
    let release: Value = serde_json::from_slice(&get(
        &client,
        &format!("https://api.github.com/repos/{REPOSITORY}/releases/tags/{tag}"),
    )?)?;
    let tag = release["tag_name"].as_str().ok_or("missing_release_tag")?;
    if release["draft"] != false
        || release["prerelease"] != false
        || !tag.starts_with('v')
        || !valid_version(&tag[1..])
    {
        return Err("invalid_official_release".into());
    }
    // The public tag's source manifest can lag a release. Use that release's actual asset metadata.
    let assets = release["assets"]
        .as_array()
        .ok_or("release_assets_missing")?;
    let prefix = format!("stackbite-{}-windows-x64-", &tag[1..]);
    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".zip"))
        })
        .ok_or("official_native_asset_missing")?;
    let name = asset["name"].as_str().ok_or("invalid_release_asset")?;
    let signature_name = format!("{name}.signature.json");
    let metadata = assets
        .iter()
        .find(|a| a["name"] == signature_name)
        .ok_or("official_release_metadata_missing")?;
    let expected = metadata["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .ok_or("release_metadata_digest_missing")?;
    let bytes = get(
        &client,
        &format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{signature_name}"),
    )?;
    if format!("{:x}", Sha256::digest(&bytes)) != expected {
        return Err("release_metadata_digest_mismatch".into());
    }
    let hash = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .ok_or("release_asset_digest_missing")?;
    let latest = checked_manifest(serde_json::from_slice(&bytes)?, tag, hash)?;
    write(&cache, &latest)?;
    Ok(latest)
}

/// All writes, source snapshots, build logs and executable pointers stay under this root.
pub fn configure(source: &Path, interval: u64) -> Result<Value> {
    if !source.is_absolute() || !(15..=86400).contains(&interval) {
        return Err("absolute_source_and_interval_15_to_86400_required".into());
    }
    let source = resolved(source)?;
    if source.join("ARCHIVED.json").exists() {
        return Err("restore_stackbite_source_first".into());
    }
    let git = Command::new("git")
        .arg("-C")
        .arg(&source)
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !git.status.success() {
        return Err("stackbite_source_repository_required".into());
    }
    let root = crate::runtime_bundle::home()?;
    // A cache inside the upstream checkout would violate the read-only source contract.
    separate_cache(&root, &source)?;
    let value = json!({"schema_version":1,"enabled":true,"source_repository":source,
        "interval_seconds":interval,"official_repository":REPOSITORY});
    write(&root.join("stackbite-follow.json"), &value)?;
    check_at(&root, true)?;
    status_at(&root)
}
pub fn status() -> Result<Value> {
    let mut status = status_at(&crate::runtime_bundle::home()?)?;
    status["offline"] = json!(std::env::args().any(|arg| arg == "--offline"));
    Ok(status)
}
fn status_at(root: &Path) -> Result<Value> {
    let Some(cfg) = config(root)? else {
        return Ok(json!({"enabled":false,"status":"disabled"}));
    };
    let mut value = if root.join("stackbite-update.json").exists() {
        read(&root.join("stackbite-update.json"))?
    } else {
        json!({"status":"waiting"})
    };
    value["enabled"] = cfg["enabled"].clone();
    value["interval_seconds"] = cfg["interval_seconds"].clone();
    value["official_repository"] = json!(REPOSITORY);
    if root.join("stackbite-current.json").exists() {
        let active = read(&root.join("stackbite-current.json"))?;
        value["ready_version"] = active["version"].clone();
        value["ready_commit"] = active["commit"].clone();
        value["activated_at"] = active["activated_at"].clone();
    }
    Ok(value)
}
pub fn selected() -> Result<Option<Value>> {
    let root = crate::runtime_bundle::home()?;
    if !config(&root)?.is_some_and(|v| v["enabled"] == true) {
        return Ok(None);
    }
    let path = root.join("stackbite-current.json");
    if !path.exists() {
        return Ok(None);
    }
    let value = read(&path)?;
    let executable = Path::new(
        value["executable"]
            .as_str()
            .ok_or("invalid_engine_pointer")?,
    );
    if !executable.starts_with(root.join("stackbite-engines"))
        || sha(executable)? != value["sha256"]
    {
        return Err("stackbite_engine_integrity_failed".into());
    }
    Ok(Some(value))
}
pub fn check() -> Result<Value> {
    let root = crate::runtime_bundle::home()?;
    check_at(&root, true)?;
    status_at(&root)
}
fn stage(root: &Path, status: &str, latest: &Value) -> Result<()> {
    write(
        &root.join("stackbite-update.json"),
        &json!({"status":status,
        "checked_at":now(),"latest_version":latest["version"],"latest_commit":latest["commit"]}),
    )
}
fn check_at(root: &Path, force: bool) -> Result<()> {
    let Some(cfg) = config(root)? else {
        return Ok(());
    };
    if cfg["enabled"] != true {
        return Ok(());
    }
    let source = Path::new(cfg["source_repository"].as_str().ok_or("invalid_source")?);
    separate_cache(root, source)?;
    let Ok(_guard) = UPDATE.try_lock() else {
        return Ok(());
    };
    // File lock also excludes other SwitchBite processes sharing the runtime home.
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("stackbite-update.lock"))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    if !force
        && let Ok(last) = read(&root.join("stackbite-update.json"))
        && now().saturating_sub(last["checked_at"].as_u64().unwrap_or(0))
            < cfg["interval_seconds"].as_u64().unwrap_or(60)
    {
        return Ok(());
    }
    let result: Result<()> = (|| {
        stage(root, "checking", &Value::Null)?;
        let latest = official_latest(root)?;
        let current = root.join("stackbite-current.json");
        let adapter_hash = format!("{:x}", Sha256::digest(ADAPTER.as_bytes()));
        if current.exists() {
            let value = read(&current)?;
            if value["commit"] == latest["commit"]
                && value["adapter_sha256"] == adapter_hash
                && value["version"] == latest["version"]
                && let Some(executable) = value["executable"].as_str()
                && sha(Path::new(executable)).ok().as_deref() == value["sha256"].as_str()
            {
                stage(root, "current", &latest)?;
                return Ok(());
            }
        }
        let engine = build(root, &cfg, &latest, &adapter_hash)?;
        write(&current, &engine)?;
        stage(root, "current", &latest)
    })();
    if let Err(error) = &result {
        let mut value = read(&root.join("stackbite-update.json")).unwrap_or_else(|_| json!({}));
        value["status"] = json!("error");
        // Errors are codes from this module or IO errors, never child stderr/authentication data.
        value["error"] = json!(error.to_string().chars().take(240).collect::<String>());
        value["checked_at"] = json!(now());
        write(&root.join("stackbite-update.json"), &value)?;
    }
    result
}

fn run(mut command: Command, log: &Path, code: &'static str, timeout: Duration) -> Result<()> {
    let output = fs::OpenOptions::new().create(true).append(true).open(log)?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .spawn()?;
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(code.into())
            };
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(code.into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}
fn git(directory: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(directory)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never");
    command
}
fn build(root: &Path, cfg: &Value, latest: &Value, adapter_hash: &str) -> Result<Value> {
    let version = latest["version"].as_str().ok_or("invalid_release")?;
    let commit = latest["commit"].as_str().ok_or("invalid_release")?;
    let source = Path::new(cfg["source_repository"].as_str().ok_or("invalid_source")?);
    separate_cache(root, source)?;
    if source.join("ARCHIVED.json").exists() {
        return Err("restore_stackbite_source_first".into());
    }
    let id = format!("{version}-{commit}-{}", &adapter_hash[..16]);
    let work = root.join("stackbite-builds").join(&id);
    fs::create_dir_all(&work)?;
    let log = work.join("build.log");
    stage(root, "fetching_source", latest)?;
    // Fetch into a SwitchBite-owned bare repository. Never fetch, checkout or build upstream.
    let mirror = root.join("stackbite-source.git");
    if !mirror.exists() {
        fs::create_dir_all(&mirror)?;
        run(
            git(&mirror, &["init", "--bare"]),
            &log,
            "source_cache_init_failed",
            Duration::from_secs(30),
        )?;
    }
    let local = run(
        git(
            &mirror,
            &[
                "fetch",
                "--no-tags",
                "--depth=1",
                source.to_str().ok_or("invalid_source")?,
                commit,
            ],
        ),
        &log,
        "source_commit_unavailable",
        Duration::from_secs(120),
    );
    if local.is_err() {
        // The local source checkout may lag the release. Read its remote, then fetch only in our cache.
        let remote = git(source, &["remote", "get-url", "origin"]).output()?;
        if !remote.status.success() {
            return Err("source_remote_unavailable".into());
        }
        let remote = String::from_utf8(remote.stdout)?;
        let remote = remote.trim();
        if remote.starts_with('-') || remote.is_empty() {
            return Err("invalid_source_remote".into());
        }
        if let Ok(url) = reqwest::Url::parse(remote)
            && (!url.username().is_empty() || url.password().is_some())
        {
            return Err("use_git_credential_manager_for_source_remote".into());
        }
        run(
            git(
                &mirror,
                &["fetch", "--no-tags", "--depth=1", remote, commit],
            ),
            &log,
            "source_commit_unavailable",
            Duration::from_secs(180),
        )?;
    }
    let archive = work.join("source.zip");
    let source_prefix = if git(
        &mirror,
        &[
            "cat-file",
            "-e",
            &format!("{commit}:apps/stackbite/rust/crates/provider-stream/Cargo.toml"),
        ],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()?
    .success()
    {
        "apps/stackbite/rust"
    } else {
        "rust"
    };
    let mut export = git(&mirror, &["archive", "--format=zip"]);
    export
        .arg("--output")
        .arg(&archive)
        .arg(commit)
        .arg("--")
        .arg(source_prefix);
    run(
        export,
        &log,
        "source_export_failed",
        Duration::from_secs(60),
    )?;
    let snapshot = work.join("upstream");
    if !snapshot.exists() {
        let staging = work.join(format!("upstream-{}-{}", std::process::id(), now()));
        extract(&archive, &staging)?;
        fs::rename(staging, &snapshot)?;
    }
    let crate_dir = [
        snapshot.join("apps/stackbite/rust/crates/provider-stream"),
        snapshot.join("rust/crates/provider-stream"),
    ]
    .into_iter()
    .find(|path| path.join("Cargo.toml").is_file())
    .ok_or("provider_stream_not_in_release_source")?;
    let adapter = work.join("adapter");
    fs::create_dir_all(adapter.join("src"))?;
    fs::write(adapter.join("src/main.rs"), ADAPTER)?;
    let dependency = serde_json::to_string(&crate_dir.to_string_lossy().replace('\\', "/"))?;
    fs::write(
        adapter.join("Cargo.toml"),
        format!(
            r#"[package]
name = "switchbite-stackbite-engine"
version = "0.1.0"
edition = "2024"
rust-version = "1.89"
license = "LicenseRef-Proprietary"
[workspace]
[dependencies]
serde = {{ version = "=1.0.228", features = ["derive"] }}
serde_json = "=1.0.150"
sha2 = "=0.10.9"
getrandom = "=0.3.4"
stackbite-provider-stream = {{ path = {dependency} }}
[profile.release]
strip = true
"#
        ),
    )?;
    let target = root.join("stackbite-build-target");
    let cargo = |action: &str| {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(&adapter)
            .arg(action)
            .arg("--manifest-path")
            .arg(adapter.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &target)
            .env("SWITCHBITE_STACKBITE_VERSION", version)
            .env("SWITCHBITE_STACKBITE_COMMIT", commit);
        #[cfg(windows)]
        cmd.env("RUSTFLAGS", "-C target-feature=+crt-static");
        cmd
    };
    stage(root, "building", latest)?;
    if !adapter.join("Cargo.lock").exists() {
        run(
            cargo("generate-lockfile"),
            &log,
            "adapter_dependencies_failed",
            Duration::from_secs(180),
        )?;
    }
    let mut command = cargo("test");
    command.args(["--locked", "--release"]);
    run(
        command,
        &log,
        "adapter_tests_failed",
        Duration::from_secs(600),
    )?;
    let mut command = cargo("build");
    command.args(["--locked", "--release"]);
    run(
        command,
        &log,
        "adapter_build_failed",
        Duration::from_secs(600),
    )?;
    let name = if cfg!(windows) {
        "switchbite-stackbite-engine.exe"
    } else {
        "switchbite-stackbite-engine"
    };
    let built = target.join("release").join(name);
    stage(root, "testing", latest)?;
    let caps = crate::adapters::stackbite_backend::capabilities(&built)?;
    if caps["stackbite_version"] != version || caps["stackbite_commit"] != commit {
        return Err("engine_release_identity_mismatch".into());
    }
    let mut smoke = Command::new(&built);
    smoke.arg("self-test");
    run(
        smoke,
        &log,
        "engine_transport_test_failed",
        Duration::from_secs(15),
    )?;
    let directory = root.join("stackbite-engines").join(id);
    fs::create_dir_all(&directory)?;
    let executable = directory.join(name);
    if executable.exists() {
        if sha(&executable)? != sha(&built)? {
            return Err("immutable_engine_conflict".into());
        }
    } else {
        fs::copy(&built, &executable)?;
    }
    Ok(
        json!({"kind":"stackbite_upstream_adapter","version":version,"commit":commit,
        "adapter_sha256":adapter_hash,"executable":executable,"sha256":sha(&executable)?,
        "capabilities":caps,"activated_at":now(),"release_url":format!("https://github.com/{REPOSITORY}/releases/tag/v{version}")}),
    )
}
fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
    if zip.len() > 20000 {
        return Err("source_archive_too_large".into());
    }
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let relative = entry.enclosed_name().ok_or("invalid_source_archive_path")?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("source_archive_symlink".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("source_archive_too_large")?;
        if total > 512 * 1024 * 1024 {
            return Err("source_archive_too_large".into());
        }
        let path = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
        } else {
            fs::create_dir_all(path.parent().ok_or("invalid_source_archive_path")?)?;
            let mut output = fs::File::create(path)?;
            std::io::copy(&mut entry, &mut output)?;
        }
    }
    Ok(())
}

pub struct Watcher(mpsc::Sender<()>);
impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}
pub fn watch() -> Result<Option<Watcher>> {
    if std::env::args().any(|a| a == "--offline") {
        return Ok(None);
    }
    let root = crate::runtime_bundle::home()?;
    let Some(cfg) = config(&root)? else {
        return Ok(None);
    };
    if cfg["enabled"] != true {
        return Ok(None);
    }
    let interval = cfg["interval_seconds"]
        .as_u64()
        .unwrap_or(60)
        .clamp(15, 86400);
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        loop {
            let _ = check_at(&root, false);
            if receiver.recv_timeout(Duration::from_secs(interval))
                != Err(mpsc::RecvTimeoutError::Timeout)
            {
                break;
            }
        }
    });
    Ok(Some(Watcher(sender)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_metadata_pins_the_release_source_commit() {
        let hash = "b".repeat(64);
        let manifest = json!({"payload":{"schema":"stackbite-update-v1","sha256":hash,"version":"0.5.75","commit":"a".repeat(40)}});
        assert!(checked_manifest(manifest.clone(), "v0.5.75", &hash).is_ok());
        assert!(checked_manifest(manifest.clone(), "v0.5.74", &hash).is_err());
        let mut wrong = manifest.clone();
        wrong["payload"]["commit"] = json!("HEAD");
        assert!(checked_manifest(wrong, "v0.5.75", &hash).is_err());
        assert!(checked_manifest(manifest, "v0.5.75", &"c".repeat(64)).is_err());
    }
    #[test]
    fn failed_update_keeps_last_ready_engine_visible() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("stackbite-follow.json"),
            &json!({"enabled":true,"interval_seconds":60}),
        )
        .unwrap();
        write(
            &dir.path().join("stackbite-current.json"),
            &json!({"version":"0.5.74","commit":"a".repeat(40)}),
        )
        .unwrap();
        write(
            &dir.path().join("stackbite-update.json"),
            &json!({"status":"error","latest_version":"0.5.75"}),
        )
        .unwrap();
        let status = status_at(dir.path()).unwrap();
        assert_eq!(status["ready_version"], "0.5.74");
        assert_eq!(status["latest_version"], "0.5.75");
        assert_eq!(status["status"], "error");
        assert!(status.get("source_repository").is_none());
    }
    #[test]
    fn source_checkout_cannot_be_used_as_a_build_or_runtime_cache() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        let proposed = source.join("not-created").join("runtime");
        assert!(separate_cache(&proposed, &source).is_err());
        assert!(!proposed.exists());
        assert!(separate_cache(&dir.path().join("switchbite-runtime"), &source).is_ok());
        assert!(separate_cache(&source.join("..").join("runtime"), &source).is_err());
    }
}
