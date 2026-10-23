// Standalone inclusion also allows a small harness without compiling the server.
#[path = "../src/connection.rs"]
mod connection;

use connection::{ConfigError, render};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const CLIENTS: &[&str] = &[
    "codex",
    "claude",
    "cursor",
    "opencode",
    "grok",
    "antigravity",
    "generic",
];

fn executable() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\Program Files\\Jev 路由\\quoted\"plugin.exe")
    } else {
        PathBuf::from("/opt/Jev 路由/quoted\"plugin")
    }
}

fn json_server<'a>(value: &'a Value, client: &str) -> &'a Value {
    if client == "opencode" {
        &value["mcp"]["servers"]["switchbite"]
    } else {
        &value["mcpServers"]["switchbite"]
    }
}

#[test]
fn json_dialects_preserve_paths_and_launch_typed_profile() {
    for client in CLIENTS
        .iter()
        .copied()
        .filter(|c| !matches!(*c, "codex" | "grok"))
    {
        for offline in [false, true] {
            let path = executable();
            let value: Value =
                serde_json::from_str(&render(client, &path, offline).unwrap()).unwrap();
            let server = json_server(&value, client);
            let mut expected_args = vec!["serve", "--profile", "cli"];
            if offline {
                expected_args.push("--offline");
            }
            if client == "opencode" {
                let mut command = vec![path.to_str().unwrap()];
                command.extend(expected_args);
                assert_eq!(server["type"], "local");
                assert_eq!(server["command"], json!(command));
                assert!(server.get("enabled").is_none());
                assert!(value["mcp"].get("switchbite").is_none());
            } else {
                assert_eq!(server["type"], "stdio");
                assert_eq!(server["command"], path.to_str().unwrap());
                assert_eq!(server["args"], json!(expected_args));
            }
            if client == "claude" && !offline {
                assert_eq!(
                    server["env"],
                    json!({"TYPESAFE_API_KEY": "${TYPESAFE_API_KEY:-}"})
                );
            } else if client == "cursor" && !offline {
                assert_eq!(
                    server["env"],
                    json!({"TYPESAFE_API_KEY": "${env:TYPESAFE_API_KEY}"})
                );
            } else {
                assert!(server.get("env").is_none());
            }
        }
    }
}

#[test]
fn toml_dialects_preserve_paths_and_profile_arguments() {
    // These fields deliberately use the common JSON/TOML basic-string subset.
    // A real TOML parser/client acceptance check belongs in the small harness.
    for client in ["codex", "grok"] {
        for offline in [false, true] {
            let path = executable();
            let text = render(client, &path, offline).unwrap();
            assert!(text.starts_with("[mcp_servers.switchbite]\n"));
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(&format!("{name} = ")))
                    .unwrap()
            };
            assert_eq!(
                serde_json::from_str::<String>(field("command")).unwrap(),
                path.to_str().unwrap()
            );
            let mut args = vec!["serve", "--profile", "cli"];
            if offline {
                args.push("--offline");
            }
            assert_eq!(
                serde_json::from_str::<Value>(field("args")).unwrap(),
                json!(args)
            );
            if client == "codex" {
                assert_eq!(field("startup_timeout_sec"), "15");
                assert_eq!(field("tool_timeout_sec"), "45");
                assert_eq!(text.contains("env_vars = [\"TYPESAFE_API_KEY\"]"), !offline);
            } else {
                assert_eq!(field("enabled"), "true");
            }
        }
    }
}

#[test]
fn offline_has_no_key_reference_and_rendering_does_not_copy_environment() {
    for client in CLIENTS {
        let offline = render(client, &executable(), true).unwrap();
        assert!(!offline.contains("TYPESAFE_API_KEY"));
        assert!(!offline.contains("${"));
        // Only literal references or variable names are emitted.
        let online = render(client, &executable(), false).unwrap();
        if !matches!(*client, "claude" | "codex" | "cursor") {
            assert!(!online.contains("TYPESAFE_API_KEY"));
        }
        assert_eq!(online, render(client, &executable(), false).unwrap());
    }
}

#[test]
fn secret_environment_is_not_serialized() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "render_with_secret_environment", "--nocapture"])
        .env(
            "TYPESAFE_API_KEY",
            "connection-test-secret-do-not-serialize",
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn render_with_secret_environment() {
    for client in CLIENTS {
        for offline in [false, true] {
            assert!(
                !render(client, &executable(), offline)
                    .unwrap()
                    .contains("connection-test-secret-do-not-serialize")
            );
        }
    }
}

#[test]
fn invalid_inputs_have_sanitized_static_errors() {
    for client in CLIENTS {
        for path in ["", "plugin.exe", "./plugin", "../plugin", "C:plugin.exe"] {
            assert_eq!(
                render(client, Path::new(path), false),
                Err(ConfigError::InvalidExecutable)
            );
        }
        for control in ['\0', '\n', '\r', '\t', '\u{7f}', '\u{85}'] {
            let path = PathBuf::from(format!("{}{control}private-input", executable().display()));
            let error = render(client, &path, false).unwrap_err();
            assert_eq!(error, ConfigError::InvalidExecutable);
            assert!(!error.to_string().contains("private-input"));
        }
    }
    let error = render("untrusted-client-secret", &executable(), false).unwrap_err();
    assert_eq!(error, ConfigError::UnsupportedClient);
    assert_eq!(error.to_string(), "Unsupported MCP client.");
    assert_eq!(
        ConfigError::Encoding.to_string(),
        "Unable to encode MCP configuration."
    );
}

#[cfg(unix)]
#[test]
fn rejects_non_unicode_path_without_lossy_substitution() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let path = PathBuf::from(OsString::from_vec(b"/tmp/plugin-\xff".to_vec()));
    assert_eq!(
        render("generic", &path, false),
        Err(ConfigError::InvalidExecutable)
    );
}

#[cfg(windows)]
#[test]
fn rejects_non_unicode_path_without_lossy_substitution() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let mut units: Vec<u16> = "C:\\plugin-".encode_utf16().collect();
    units.push(0xd800);
    let path = PathBuf::from(OsString::from_wide(&units));
    assert_eq!(
        render("generic", &path, false),
        Err(ConfigError::InvalidExecutable)
    );
}
