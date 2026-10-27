//! Immutable, paired engine releases and a small web hand-off at new-run boundaries.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_BINARY: u64 = 256 * 1024 * 1024;
static INSTALL: Mutex<()> = Mutex::new(());

fn home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SWITCHBITE_RUNTIME_HOME") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("runtime_home_must_be_absolute".into());
        }
        return Ok(path);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or("runtime_home_unavailable")?;
    Ok(PathBuf::from(base).join("Switchbite").join("runtime"))
}
fn read_json(path: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err("manifest_too_large".into());
    }
    Ok(serde_json::from_slice(
        bytes.strip_prefix(&[239, 187, 191]).unwrap_or(&bytes),
    )?)
}
fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    fs::create_dir_all(path.parent().ok_or("invalid_runtime_path")?)?;
    crate::catalog::write_cache(path, &serde_json::to_vec(value)?)?;
    Ok(())
}
fn digest(path: &Path) -> Result<String> {
    let mut input = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn bytes(source: &str, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    if source.starts_with("https://") {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?
            .get(source)
            .send()?
            .error_for_status()?
            .take(limit + 1)
            .read_to_end(&mut data)?;
    } else {
        let path = Path::new(source);
        if !path.is_absolute() {
            return Err("source_must_be_https_or_absolute_path".into());
        }
        fs::File::open(path)?
            .take(limit + 1)
            .read_to_end(&mut data)?;
    }
    if data.len() as u64 > limit {
        return Err("release_asset_too_large".into());
    }
    Ok(data)
}
fn channel() -> Result<Option<String>> {
    if let Ok(value) = std::env::var("SWITCHBITE_UPDATE_MANIFEST") {
        return Ok(Some(value));
    }
    let path = home()?.join("channel.json");
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(
        read_json(&path)?["manifest"]
            .as_str()
            .ok_or("invalid_update_channel")?
            .to_owned(),
    ))
}
pub fn configure(source: &str) -> Result<Value> {
    let bundle = sync_from(source)?;
    atomic_json(&home()?.join("channel.json"), &json!({"manifest":source}))?;
    Ok(bundle)
}
pub fn status() -> Result<Value> {
    let path = home()?.join("current.json");
    Ok(if path.exists() {
        read_json(&path)?
    } else {
        json!({"status":"unconfigured"})
    })
}
pub fn sync() -> Result<Option<Value>> {
    channel()?.map(|source| sync_from(&source)).transpose()
}
pub fn sync_from(source: &str) -> Result<Value> {
    let _guard = INSTALL.lock().map_err(|_| "runtime_install_busy")?;
    let raw = bytes(source, MAX_MANIFEST)?;
    let manifest: Value =
        serde_json::from_slice(raw.strip_prefix(&[239, 187, 191]).unwrap_or(&raw))?;
    if manifest["schema_version"] != 1 || manifest["protocol_version"] != 1 {
        return Err("incompatible_runtime_manifest".into());
    }
    let id = manifest["bundle_id"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && ![".", ".."].contains(s)
                && s.len() <= 96
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
        .ok_or("invalid_bundle_id")?;
    let dir = home()?.join("bundles").join(id);
    fs::create_dir_all(&dir)?;
    let mut installed = manifest.clone();
    for name in ["switchbite", "stackbite"] {
        let asset = &manifest[name];
        let hash = asset["sha256"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("invalid_asset_hash")?
            .to_ascii_lowercase();
        if asset["version"].as_str().is_none_or(str::is_empty) {
            return Err("missing_asset_version".into());
        }
        let filename = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let destination = dir.join(filename);
        if destination.exists() {
            if digest(&destination)? != hash {
                return Err("immutable_bundle_conflict".into());
            }
        } else {
            let source = asset["source"].as_str().ok_or("missing_asset_source")?;
            let data = bytes(source, MAX_BINARY)?;
            if format!("{:x}", Sha256::digest(&data)) != hash {
                return Err("asset_hash_mismatch".into());
            }
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let staging = dir.join(format!("{name}.download-{}-{nonce}", std::process::id()));
            {
                use std::io::Write;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&staging)?;
                file.write_all(&data)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
            }
            fs::rename(&staging, &destination)?;
        }
        installed[name]["executable"] = json!(destination);
    }
    let engine = Path::new(
        installed["stackbite"]["executable"]
            .as_str()
            .ok_or("invalid_bundle")?,
    );
    let caps = crate::adapters::stackbite_backend::capabilities(engine)
        .map_err(|_| "incompatible_stackbite_engine")?;
    installed["stackbite"]["capabilities"] = caps;
    installed["status"] = json!("ready");
    atomic_json(&home()?.join("current.json"), &installed)?;
    Ok(installed)
}
pub fn default_backend() -> Result<Option<Value>> {
    if let Some(value) = std::env::var_os("SWITCHBITE_STACKBITE_ENGINE") {
        return Ok(Some(
            json!({"type":"stackbite","executable":PathBuf::from(value)}),
        ));
    }
    let value = status()?;
    Ok(value["stackbite"]["executable"]
        .as_str()
        .map(|path| json!({"type":"stackbite","executable":path})))
}
pub fn pin_request(request: &mut Value) -> Result<()> {
    if request["host"].is_null() {
        request["host"] = json!({});
    }
    if !request["host"].is_object() {
        return Err("invalid_host".into());
    }
    if request["host"]["backend"].is_null()
        && let Some(backend) = default_backend()?
    {
        request["host"]["backend"] = backend;
    }
    let metadata = run_runtime_metadata(request)?;
    if let Some(base) = metadata["project"]["base_commit"].as_str() {
        request["host"]["base"] = json!(base);
    }
    request["runtime"] = metadata;
    Ok(())
}
pub fn run_runtime_metadata(request: &Value) -> Result<Value> {
    let workspace = request["workspace"].as_str().ok_or("invalid_workspace")?;
    let reference = request["host"]["base"].as_str().unwrap_or("HEAD");
    let base = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned());
    let backend = if request["host"]["backend"]["type"] == "stackbite" {
        let path = Path::new(
            request["host"]["backend"]["executable"]
                .as_str()
                .ok_or("invalid_backend")?,
        );
        let caps = crate::adapters::stackbite_backend::capabilities(path)
            .map_err(|_| "stackbite_backend_unavailable")?;
        json!({"kind":"stackbite_private_clone","version":caps["stackbite_version"].as_str().unwrap_or_else(|| caps["version"].as_str().unwrap_or("unknown")),"bridge_version":caps["version"],"protocol_version":caps["protocol_version"],"executable":path,"sha256":digest(path)?,"providers":caps["providers"]})
    } else {
        json!({"kind":"native_cli","version":env!("CARGO_PKG_VERSION"),"protocol_version":1})
    };
    Ok(
        json!({"switchbite":{"version":env!("CARGO_PKG_VERSION"),"commit":option_env!("SWITCHBITE_BUILD_COMMIT").unwrap_or("unknown")},"backend":backend,"project":{"workspace":workspace,"base_commit":base}}),
    )
}
/// New app submissions can move to a newer immutable app without disturbing old runs.
pub fn handoff_new_run(body: &Value, workspace: &Path) -> Result<Option<Value>> {
    if std::env::args().any(|a| a == "--offline") {
        return Ok(None);
    }
    let Some(bundle) = sync()? else {
        return Ok(None);
    };
    let executable = PathBuf::from(
        bundle["switchbite"]["executable"]
            .as_str()
            .ok_or("invalid_bundle")?,
    );
    if digest(&std::env::current_exe()?)? == bundle["switchbite"]["sha256"].as_str().unwrap_or("") {
        return Ok(None);
    }
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dir = home()?
        .join("submissions")
        .join(format!("{}-{nonce}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let submission = dir.join("request.json");
    let readiness = dir.join("ready.json");
    atomic_json(&submission, body)?;
    let mut command = Command::new(executable);
    command.args(["workflow", "app", "--port", "0"]);
    let inherited: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < inherited.len() {
        if ["--judge", "--endpoint", "--model"].contains(&inherited[i].as_str())
            && i + 1 < inherited.len()
        {
            command.args([&inherited[i], &inherited[i + 1]]);
            i += 2;
        } else {
            if inherited[i] == "--offline" {
                command.arg("--offline");
            }
            i += 1;
        }
    }
    let mut child = command
        .current_dir(workspace)
        .env("SWITCHBITE_INITIAL_SUBMISSION", &submission)
        .env("SWITCHBITE_WEB_READY", &readiness)
        .env(
            "SWITCHBITE_STACKBITE_ENGINE",
            bundle["stackbite"]["executable"]
                .as_str()
                .ok_or("invalid_bundle")?,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(dir.join("app.log"))?))
        .spawn()?;
    for _ in 0..100 {
        if readiness.exists() {
            let ready = read_json(&readiness)?;
            return Ok(Some(
                json!({"status":"handoff","handoff":{"url":ready["url"],"bundle_id":bundle["bundle_id"]},"tasks":[],"actions":[],"progress":{"done":0,"total":0}}),
            ));
        }
        if child.try_wait()?.is_some() {
            return Err("updated_app_start_failed".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("updated_app_start_timeout".into())
}
pub fn initial_submission() -> Result<Option<Value>> {
    let Some(path) = std::env::var_os("SWITCHBITE_INITIAL_SUBMISSION") else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let value = read_json(&path)?;
    fs::remove_file(path)?;
    Ok(Some(value))
}
pub fn publish_web_ready(host: &str) -> Result<()> {
    if let Some(path) = std::env::var_os("SWITCHBITE_WEB_READY") {
        atomic_json(Path::new(&path), &json!({"url":format!("http://{host}/")}))?;
    }
    Ok(())
}
pub fn launch_current(args: &[String]) -> Result<Option<std::process::ExitStatus>> {
    if args.first().map(String::as_str) != Some("workflow")
        || !matches!(
            args.get(1).map(String::as_str),
            Some("app" | "run" | "start")
        )
        || args.iter().any(|a| a == "--resume" || a == "--offline")
    {
        return Ok(None);
    }
    let Some(bundle) = sync()? else {
        return Ok(None);
    };
    if digest(&std::env::current_exe()?)? == bundle["switchbite"]["sha256"].as_str().unwrap_or("") {
        return Ok(None);
    }
    let executable = bundle["switchbite"]["executable"]
        .as_str()
        .ok_or("invalid_bundle")?;
    let status = Command::new(executable)
        .args(args)
        .env(
            "SWITCHBITE_STACKBITE_ENGINE",
            bundle["stackbite"]["executable"]
                .as_str()
                .ok_or("invalid_bundle")?,
        )
        .status()?;
    Ok(Some(status))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_requires_protocol_and_valid_id_before_install() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        fs::write(&path, br#"{"schema_version":1,"protocol_version":2}"#).unwrap();
        assert!(
            sync_from(path.to_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("incompatible_runtime_manifest")
        );
        fs::write(
            &path,
            br#"{"schema_version":1,"protocol_version":1,"bundle_id":"../escape"}"#,
        )
        .unwrap();
        assert!(sync_from(path.to_str().unwrap()).is_err());
    }
}
