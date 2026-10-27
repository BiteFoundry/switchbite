use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
use switchbite::adapters::LocalHost;

fn git(repo: &Path, args: &[&str]) {
    let result = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn action(id: &str, kind: &str, payload: Value) -> Value {
    json!({"id":id,"kind":kind,"run_id":"run-1","payload":payload})
}

#[test]
fn recovered_read_denial_keeps_output_verification_and_scope_enforcement() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("claude.rs");
    fs::write(&source, r##"fn main() {
        let args: Vec<String> = std::env::args().collect();
        let session = &args[args.iter().position(|s| s == "--session-id").unwrap() + 1];
        let prompt = args.last().unwrap();
        if !prompt.contains("missing-output") {
            std::fs::write("report.md", "x".repeat(40000)).unwrap();
        }
        if prompt.contains("outside-output") {
            std::fs::write("outside.txt", "outside").unwrap();
        }
        println!("{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"{}\",\"model\":\"fake\"}}", session);
        println!("{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"permission_denials\":[{{\"tool_name\":\"Read\",\"tool_use_id\":\"denied-1\",\"tool_input\":{{\"file_path\":\"private.txt\"}}}}],\"session_id\":\"{}\"}}", session);
    }"##).unwrap();
    let exe = tmp.path().join(if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    });
    assert!(
        Command::new("rustc")
            .arg(source)
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap()
            .success()
    );

    for mode in ["valid-output", "missing-output", "outside-output"] {
        let case = tmp.path().join(mode);
        let repo = case.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@localhost",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        let host =
            LocalHost::new(&repo, &case.join("state"), &json!({"timeout_seconds":20})).unwrap();
        let task = json!({"id":"report", "objective":mode, "outputs":["report.md"], "write_paths":["report.md"]});
        let prepared = host
            .perform(&action("prepare", "prepare", json!({"task":task})))
            .unwrap();
        let payload = json!({"task":task, "prepared":prepared, "candidate":{"provider":"claude","executable":exe,"model":"fake","effort":"high"}, "checks":[]});
        let launch = host
            .perform(&action("launch", "launch", payload.clone()))
            .unwrap();
        assert_eq!(launch["status"], "completed", "{mode}: {launch}");
        assert_eq!(launch["completion_verified"], true);
        assert_eq!(launch["permission_denial_count"], 1);
        let verify = host
            .perform(&action("verify", "verify", payload.clone()))
            .unwrap();
        if mode == "missing-output" {
            assert_eq!(verify["status"], "failed");
            assert_eq!(verify["evidence"][0]["reason"], "output_missing");
            continue;
        }
        assert_eq!(verify["status"], "passed", "{verify}");
        let integration = host
            .perform(&action("integrate", "integrate", payload))
            .unwrap();
        if mode == "outside-output" {
            assert_eq!(integration["status"], "failed");
            assert_eq!(integration["evidence"][0], "write outside declared outputs");
        } else {
            assert_eq!(integration["status"], "integrated", "{integration}");
            let target = Path::new(prepared["integration_workspace"].as_str().unwrap());
            assert_eq!(fs::read(target.join("report.md")).unwrap().len(), 40000);
            assert!(!repo.join("report.md").exists());
        }
    }
}
