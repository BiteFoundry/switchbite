use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_switchbite"));
    command
        .env("SWITCHBITE_RUNTIME_HOME", home)
        .env_remove("SWITCHBITE_UPDATE_MANIFEST")
        .env_remove("SWITCHBITE_STACKBITE_ENGINE")
        .env_remove("SWITCHBITE_INITIAL_SUBMISSION")
        .env_remove("SWITCHBITE_WEB_READY");
    command
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn sha(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}
fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}
fn fixture(dir: &Path) -> PathBuf {
    let source = dir.join("fixture.rs");
    let binary = dir.join(if cfg!(windows) {
        "fixture.exe"
    } else {
        "fixture"
    });
    fs::write(&source, r##"fn main() {
        match std::env::args().nth(1).as_deref() {
            Some("capabilities") => println!("{{\"protocol\":1,\"protocol_version\":1,\"raw_streams\":true,\"version\":\"fixture\",\"providers\":[\"codex\"]}}"),
            Some("hold") => std::thread::sleep(std::time::Duration::from_secs(30)),
            Some("workflow") => {
                let submission = std::env::var_os("SWITCHBITE_INITIAL_SUBMISSION").unwrap();
                let ready = std::env::var_os("SWITCHBITE_WEB_READY").unwrap();
                let recorded = std::env::var_os("SWITCHBITE_TEST_CAPTURE").unwrap();
                std::fs::copy(submission, recorded).unwrap();
                std::fs::write(ready, r#"{"url":"http://127.0.0.1:2468/"}"#).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(500));
            },
            _ => std::process::exit(2),
        }
    }"##).unwrap();
    let output = Command::new("rustc")
        .args([source.as_os_str(), "-o".as_ref(), binary.as_os_str()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}
fn manifest(id: &str, app: &Path, engine: &Path) -> Value {
    json!({"schema_version":1,"protocol_version":1,"bundle_id":id,
        "switchbite":{"version":"fixture","source":app,"sha256":sha(app)},
        "stackbite":{"version":"fixture","source":engine,"sha256":sha(engine)}})
}

#[test]
fn installation_is_paired_immutable_and_failure_preserves_current() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("runtime");
    let app = fixture(temp.path());
    let engine = temp.path().join(if cfg!(windows) {
        "engine.exe"
    } else {
        "engine"
    });
    fs::copy(&app, &engine).unwrap();
    let original_app = fs::read(&app).unwrap();
    let original_engine = fs::read(&engine).unwrap();
    let path = temp.path().join("manifest.json");
    let valid = manifest("one", &app, &engine);
    write_json(&path, &valid);
    let installed = success(
        command(&home)
            .args(["runtime", "configure", "--manifest"])
            .arg(&path)
            .output()
            .unwrap(),
    );
    assert_eq!(installed["bundle_id"], "one");
    let installed_app = PathBuf::from(installed["switchbite"]["executable"].as_str().unwrap());
    let installed_engine = PathBuf::from(installed["stackbite"]["executable"].as_str().unwrap());
    assert_eq!(sha(&installed_app), sha(&app));
    assert_eq!(sha(&installed_engine), sha(&engine));
    let current = fs::read(home.join("current.json")).unwrap();

    let mut running = Command::new(&installed_app)
        .arg("hold")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(running.try_wait().unwrap().is_none());
    let again = command(&home).args(["runtime", "sync"]).output().unwrap();
    let still_running = running.try_wait().unwrap().is_none();
    let _ = running.kill();
    let _ = running.wait();
    assert!(still_running);
    assert_eq!(success(again)["bundle_id"], "one");
    assert_eq!(fs::read(&installed_app).unwrap(), original_app);
    assert_eq!(fs::read(&app).unwrap(), original_app);
    assert_eq!(fs::read(&engine).unwrap(), original_engine);

    let mut invalid = valid.clone();
    invalid["protocol_version"] = json!(2);
    write_json(&path, &invalid);
    assert!(
        !command(&home)
            .args(["runtime", "sync"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(home.join("current.json")).unwrap(), current);
    let mut corrupt = valid.clone();
    corrupt["bundle_id"] = json!("two");
    corrupt["stackbite"]["sha256"] = json!("0".repeat(64));
    write_json(&path, &corrupt);
    assert!(
        !command(&home)
            .args(["runtime", "sync"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(home.join("current.json")).unwrap(), current);
    let mut conflict = valid;
    conflict["switchbite"]["sha256"] = json!("0".repeat(64));
    write_json(&path, &conflict);
    assert!(
        !command(&home)
            .args(["runtime", "sync"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(home.join("current.json")).unwrap(), current);
    assert_eq!(fs::read(installed_app).unwrap(), original_app);
}

#[test]
fn workflow_start_records_the_git_base_at_submission() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["commit", "--allow-empty", "-m", "base"],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let base = Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let base = String::from_utf8(base.stdout).unwrap().trim().to_owned();
    let request = temp.path().join("request.json");
    write_json(
        &request,
        &json!({"workspace":repo,"pool":[{"id":"worker","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"one","objective":"one","outputs":["one.txt"]}]}}),
    );
    let state = success(
        command(&temp.path().join("runtime"))
            .current_dir(&repo)
            .args(["workflow", "start", "--request-file"])
            .arg(&request)
            .output()
            .unwrap(),
    );
    assert_eq!(state["runtime"]["project"]["base_commit"], base);
    assert!(
        Command::new("git")
            .current_dir(&repo)
            .args(["commit", "--allow-empty", "-m", "later"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let status = success(
        command(&temp.path().join("runtime"))
            .current_dir(&repo)
            .args(["workflow", "status", state["run_id"].as_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(status["runtime"]["project"]["base_commit"], base);
}

#[test]
fn followed_engine_changes_only_new_runs_and_never_writes_the_source_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("runtime");
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["commit", "--allow-empty", "-m", "base"],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let binary = fixture(temp.path());
    fs::create_dir_all(&home).unwrap();
    write_json(
        &home.join("stackbite-follow.json"),
        &json!({"enabled":true}),
    );
    let request = temp.path().join("request.json");
    write_json(
        &request,
        &json!({"workspace":repo,"pool":[{"id":"worker","description":"worker","capabilities":[],"available":true}],"plan":{"nodes":[{"id":"one","objective":"one","outputs":["one.txt"]}]}}),
    );
    let mut versions = Vec::new();
    for version in ["one", "two"] {
        let folder = home.join("stackbite-engines").join(version);
        fs::create_dir_all(&folder).unwrap();
        let executable = folder.join(binary.file_name().unwrap());
        fs::copy(&binary, &executable).unwrap();
        write_json(
            &home.join("stackbite-current.json"),
            &json!({"version":version,"executable":executable,"sha256":sha(&executable)}),
        );
        let state = success(
            command(&home)
                .current_dir(&repo)
                .args(["workflow", "start", "--request-file"])
                .arg(&request)
                .output()
                .unwrap(),
        );
        assert_eq!(state["runtime"]["backend"]["executable"], json!(executable));
        versions.push(state);
    }
    let old = success(
        command(&home)
            .current_dir(&repo)
            .args([
                "workflow",
                "status",
                versions[0]["run_id"].as_str().unwrap(),
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(old["runtime"]["backend"], versions[0]["runtime"]["backend"]);
    assert_ne!(
        old["runtime"]["backend"]["executable"],
        versions[1]["runtime"]["backend"]["executable"]
    );

    // Even an uncreated nested cache must be rejected before creating follow configuration.
    let forbidden = repo.join("updater-cache");
    let rejected = command(&forbidden)
        .args(["runtime", "follow-stackbite", "--source"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("runtime_home_must_be_outside_stackbite_source")
    );
    assert!(!forbidden.exists());
}

// Invoked as a subprocess so environment configuration is never shared with other tests.
#[test]
fn handoff_helper() {
    if std::env::var_os("SWITCHBITE_TEST_HANDOFF_HELPER").is_none() {
        return;
    }
    let workspace = PathBuf::from(std::env::var_os("SWITCHBITE_TEST_WORKSPACE").unwrap());
    let body =
        json!({"workspace":workspace,"goal":"handoff test","tools":["codex"],"max_parallel":3});
    let result = switchbite::runtime_bundle::handoff_new_run(&body, &workspace)
        .unwrap()
        .unwrap();
    assert_eq!(result["handoff"]["url"], "http://127.0.0.1:2468/");
    assert_eq!(result["handoff"]["bundle_id"], "handoff");
}

#[test]
fn new_submission_is_handed_to_the_installed_app() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("runtime");
    let binary = fixture(temp.path());
    let path = temp.path().join("manifest.json");
    write_json(&path, &manifest("handoff", &binary, &binary));
    success(
        command(&home)
            .args(["runtime", "configure", "--manifest"])
            .arg(&path)
            .output()
            .unwrap(),
    );
    let capture = temp.path().join("captured.json");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "handoff_helper", "--nocapture"])
        .env("SWITCHBITE_RUNTIME_HOME", &home)
        .env("SWITCHBITE_TEST_HANDOFF_HELPER", "1")
        .env("SWITCHBITE_TEST_WORKSPACE", temp.path())
        .env("SWITCHBITE_TEST_CAPTURE", &capture)
        .env_remove("SWITCHBITE_UPDATE_MANIFEST")
        .env_remove("SWITCHBITE_STACKBITE_ENGINE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Let the short-lived fixture exit before its temporary directory is removed.
    thread::sleep(Duration::from_millis(600));
    let captured: Value = serde_json::from_slice(&fs::read(capture).unwrap()).unwrap();
    assert_eq!(
        captured,
        json!({"workspace":temp.path(),"goal":"handoff test","tools":["codex"],"max_parallel":3})
    );
}
