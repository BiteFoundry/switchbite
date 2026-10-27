use super::{HostError, write_atomic};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Probe an explicitly selected isolated engine. Unsupported or missing engines fail closed.
pub fn capabilities(executable: &Path) -> Result<Value, HostError> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err(HostError("stackbite_backend_unavailable"));
    }
    let output = Command::new(executable)
        .arg("capabilities")
        .output()
        .map_err(|_| HostError("stackbite_backend_unavailable"))?;
    if !output.status.success() || output.stdout.len() > 65536 {
        return Err(HostError("stackbite_backend_incompatible"));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| HostError("stackbite_backend_incompatible"))?;
    validate(&value)?;
    Ok(value)
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
