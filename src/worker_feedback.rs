//! Local, explicit worker reports. No worker output is interpreted as a report.
use crate::{
    adapters::{hash, path_string},
    workflow::Workflow,
};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

pub(crate) struct ArchiveOnDrop {
    scratch: PathBuf,
    archive: PathBuf,
}
impl ArchiveOnDrop {
    pub(crate) fn new(scratch: PathBuf, archive: PathBuf) -> Self {
        Self { scratch, archive }
    }
}
impl Drop for ArchiveOnDrop {
    fn drop(&mut self) {
        // Copy into a newly created archive before removing the only live copy.
        if !fs::symlink_metadata(&self.scratch).is_ok_and(|m| m.file_type().is_file()) {
            return;
        }
        if let (Ok(mut input), Ok(mut output)) = (
            File::open(&self.scratch),
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.archive),
        ) && std::io::copy(&mut input, &mut output).is_ok()
            && output.sync_all().is_ok()
        {
            let _ = fs::remove_file(&self.scratch);
        }
    }
}

// Cap each line without allocating an unbounded buffer. Ignore incomplete and
// oversized lines, including any continuation which happens to look like JSON.
fn read_reports(path: &Path, execution: &str, task: &str) -> Vec<Value> {
    if !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file()) {
        return Vec::new();
    }
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut reports = Vec::new();
    let mut oversized = false;
    loop {
        let mut chunk = Vec::new();
        let Ok(n) = (&mut reader).take(65537).read_until(b'\n', &mut chunk) else {
            break;
        };
        if n == 0 {
            break;
        }
        let complete = chunk.last() == Some(&b'\n');
        if oversized || chunk.len() > 65536 {
            oversized = !complete;
            continue;
        }
        if !complete {
            break;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&chunk) else {
            continue;
        };
        let Some(obj) = value.as_object() else {
            continue;
        };
        if obj.len() != 3
            || !obj
                .keys()
                .all(|k| ["id", "kind", "message"].contains(&k.as_str()))
        {
            continue;
        }
        let (Some(id), Some(kind), Some(message)) = (
            value["id"].as_str(),
            value["kind"].as_str(),
            value["message"].as_str(),
        ) else {
            continue;
        };
        if id.is_empty()
            || id.len() > 128
            || !["blocked", "finding"].contains(&kind)
            || message.trim().is_empty()
            || message.len() > 8000
        {
            continue;
        }
        reports.push(json!({"id":format!("worker-{}", hash(&format!("{execution}\0{id}"))),"source":"worker","task_id":task,"kind":kind,"message":message}));
        if reports.len() >= 4096 {
            break;
        }
    }
    reports
}

pub(crate) fn contains_blocker(path: &Path, execution: &str) -> bool {
    read_reports(path, execution, "worker")
        .iter()
        .any(|report| report["kind"] == "blocked")
}

fn feedback_workspace(state: &Path, prepared: &Value) -> Option<PathBuf> {
    let state = fs::canonicalize(state).ok()?;
    let state_text = path_string(&state).ok()?;
    let root = if cfg!(windows) && state_text.len() > 120 {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Switchbite")
            .join("workspaces")
            .join(hash(state_text))
    } else {
        state.join("worktrees")
    };
    let root = fs::canonicalize(root).ok()?;
    let path = fs::canonicalize(prepared["workspace"].as_str()?).ok()?;
    path.starts_with(root).then_some(path)
}

/// Inspect only the exact paths derived from trusted workflow execution IDs.
pub(crate) fn collect(
    engine: &Workflow,
    run: &str,
    host_state: &Path,
) -> Result<(), crate::workflow::WorkflowError> {
    let state = engine.status(run)?;
    if !["running", "paused"].contains(&state["status"].as_str().unwrap_or("")) {
        return Ok(());
    }
    let mut seen: std::collections::HashSet<String> = state["feedback"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["id"].as_str().map(str::to_owned))
        .collect();
    let mut bindings = Vec::new();
    for action in state["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["kind"] == "launch")
    {
        if let (Some(execution), Some(task), Some(dir)) = (
            action["id"].as_str(),
            action["task_id"].as_str(),
            feedback_workspace(host_state, &action["payload"]["prepared"]),
        ) {
            bindings.push((execution.to_owned(), task.to_owned(), dir));
        }
    }
    for task in state["tasks"].as_array().into_iter().flatten() {
        if let (Some(execution), Some(id), Some(dir)) = (
            task["execution_id"].as_str(),
            task["id"].as_str(),
            feedback_workspace(host_state, &task["prepared"]),
        ) {
            bindings.push((execution.to_owned(), id.to_owned(), dir));
        }
    }
    for (execution, task, dir) in bindings {
        let key = hash(&execution);
        for path in [
            dir.join(format!(".switchbite-feedback-{key}.jsonl")),
            host_state
                .join("messages")
                .join(format!("{key}.feedback.jsonl")),
        ] {
            for report in read_reports(&path, &execution, &task) {
                let id = report["id"].as_str().unwrap_or_default().to_owned();
                if seen.insert(id) {
                    engine.report(run, &report)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reports_are_complete_bounded_and_bound_to_execution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reports");
        let mut f = File::create(&path).unwrap();
        writeln!(
            f,
            "{{\"id\":\"one\",\"kind\":\"blocked\",\"message\":\"need input\"}}"
        )
        .unwrap();
        writeln!(
            f,
            "{{\"id\":\"bad\",\"kind\":\"command\",\"message\":\"ignore\"}}"
        )
        .unwrap();
        writeln!(f, "{}", "x".repeat(70000)).unwrap();
        write!(
            f,
            "{{\"id\":\"incomplete\",\"kind\":\"finding\",\"message\":\"wait\"}}"
        )
        .unwrap();
        let reports = read_reports(&path, "execution-a", "task");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["kind"], "blocked");
        assert_ne!(
            reports[0]["id"],
            read_reports(&path, "execution-b", "task")[0]["id"]
        );
        writeln!(f).unwrap();
        assert_eq!(read_reports(&path, "execution-a", "task").len(), 2);
    }

    #[test]
    fn escaped_maximum_message_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reports");
        let message = "\\\n".repeat(4000);
        assert_eq!(message.len(), 8000);
        let encoded =
            serde_json::to_string(&json!({"id":"large","kind":"finding","message":message}))
                .unwrap();
        assert!(encoded.len() > 8192);
        fs::write(&path, format!("{encoded}\n")).unwrap();
        let reports = read_reports(&path, "execution", "task");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["message"], message);
    }

    #[test]
    fn archive_keeps_last_report_and_removes_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("scratch");
        let archive = dir.path().join("archive");
        fs::write(&scratch, b"report\n").unwrap();
        drop(ArchiveOnDrop::new(scratch.clone(), archive.clone()));
        assert!(!scratch.exists());
        assert_eq!(fs::read(archive).unwrap(), b"report\n");
    }
}
