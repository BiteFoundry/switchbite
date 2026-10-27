use super::{HostError, write_atomic};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// Probe an explicitly selected isolated engine. Unsupported or missing engines fail closed.
pub fn capabilities(executable: &Path) -> Result<Value, HostError> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err(HostError("stackbite_backend_unavailable"));
    }
    let mut command = Command::new(executable);
    command.arg("capabilities");
    let stdout = bounded_output(&mut command, Duration::from_secs(3))?;
    let value: Value =
        serde_json::from_slice(&stdout).map_err(|_| HostError("stackbite_backend_incompatible"))?;
    validate(&value)?;
    Ok(value)
}

// Drain both pipes concurrently, with one deadline for process exit and pipe EOF.
fn bounded_output(command: &mut Command, timeout: Duration) -> Result<Vec<u8>, HostError> {
    const LIMIT: u64 = 65536;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| HostError("stackbite_backend_unavailable"))?;
    let (tx, rx) = mpsc::channel();
    for (index, pipe) in [
        (
            0,
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
        (
            1,
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
    ] {
        let Some(pipe) = pipe else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(HostError("stackbite_backend_incompatible"));
        };
        let tx = tx.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.take(LIMIT + 1).read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send((index, result));
        });
    }
    drop(tx);
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut streams = [None, None];
    let outcome = loop {
        let mut invalid = false;
        while let Ok((index, result)) = rx.try_recv() {
            match result {
                Ok(bytes) if bytes.len() <= LIMIT as usize => streams[index] = Some(bytes),
                _ => invalid = true,
            }
        }
        if invalid {
            break Err(());
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(_) => break Err(()),
            }
        }
        if let Some(exit) = status
            && streams.iter().all(Option::is_some)
        {
            break if exit.success() {
                Ok(streams[0].take().unwrap())
            } else {
                Err(())
            };
        }
        if Instant::now() >= deadline {
            break Err(());
        }
        thread::sleep(Duration::from_millis(10));
    };
    if outcome.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    outcome.map_err(|_| HostError("stackbite_backend_incompatible"))
}

fn validate(value: &Value) -> Result<(), HostError> {
    if value["protocol"] != 1
        || value["protocol_version"] != 1
        || value["raw_streams"] != true
        || value["version"].as_str().is_none_or(str::is_empty)
        || !value["providers"].is_array()
    {
        return Err(HostError("stackbite_backend_incompatible"));
    }
    Ok(())
}

pub(super) fn configured(config: &Value) -> Result<Option<PathBuf>, HostError> {
    let Some(value) = config.get("backend") else {
        return Ok(None);
    };
    if value["type"] != "stackbite" {
        return Err(HostError("invalid_backend"));
    }
    let path = PathBuf::from(
        value["executable"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(HostError("invalid_backend"))?,
    );
    capabilities(&path)?;
    Ok(Some(path))
}

pub(super) fn wrap(
    executable: &Path,
    provider: &str,
    command: Command,
    request_path: &Path,
) -> Result<Command, HostError> {
    let caps = capabilities(executable)?;
    if !caps["providers"]
        .as_array()
        .is_some_and(|providers| providers.iter().any(|p| p.as_str() == Some(provider)))
    {
        return Err(HostError("stackbite_provider_unsupported"));
    }
    let program = command
        .get_program()
        .to_str()
        .ok_or(HostError("invalid_action"))?;
    let args: Vec<&str> = command
        .get_args()
        .map(|s| s.to_str().ok_or(HostError("invalid_action")))
        .collect::<Result<_, _>>()?;
    write_atomic(
        request_path,
        &json!({"protocol":1,"provider":provider,"program":program,"args":args}),
    )?;
    let mut engine = Command::new(executable);
    engine.arg("exec").arg("--request").arg(request_path);
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                engine.env(key, value);
            }
            None => {
                engine.env_remove(key);
            }
        }
    }
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn probe_bounds_time_and_both_streams() {
        fn shell(script: &str) -> Command {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", script]);
            command
        }
        let started = Instant::now();
        assert!(
            bounded_output(
                &mut shell("Start-Sleep -Seconds 10"),
                Duration::from_millis(300)
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        for script in [
            "[Console]::Out.Write(('x' * 70000))",
            "[Console]::Error.Write(('x' * 70000))",
        ] {
            assert!(bounded_output(&mut shell(script), Duration::from_secs(3)).is_err());
        }
        let output = bounded_output(
            &mut shell("[Console]::Out.Write('ok')"),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(output, b"ok");
        assert!(bounded_output(&mut shell("exit 7"), Duration::from_secs(3)).is_err());
    }
    #[test]
    fn protocol_requires_exact_version_and_raw_streams() {
        let mut v = json!({"protocol":1,"protocol_version":1,"version":"1.0","raw_streams":true,"providers":["codex","claude"]});
        assert!(validate(&v).is_ok());
        for (key, bad) in [
            ("protocol", json!(2)),
            ("protocol_version", json!(2)),
            ("raw_streams", json!(false)),
            ("version", json!("")),
            ("providers", json!(null)),
        ] {
            let original = v[key].clone();
            v[key] = bad;
            assert!(validate(&v).is_err());
            v[key] = original;
        }
    }
    #[test]
    fn selected_missing_engine_never_defaults_to_direct() {
        assert!(configured(&json!({})).unwrap().is_none());
        assert!(
            configured(&json!({"backend":{"type":"stackbite","executable":"missing"}})).is_err()
        );
        assert!(configured(&json!({"backend":{"type":"other"}})).is_err());
    }
}
