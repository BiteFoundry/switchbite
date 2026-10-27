//! Deterministic schema-2 native ZIP. Verifies the executable before packaging.
use crate::{contracts, core::sha256};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

type Error = Box<dyn std::error::Error + Send + Sync>;
pub const TARGET: &str = "x86_64-pc-windows-msvc";
pub const ENTRY: &str = "bin/switchbite.exe";

fn native_pe(bytes: &[u8]) -> bool {
    if bytes.get(..2) != Some(b"MZ") {
        return false;
    }
    let Some(offset) = bytes
        .get(0x3c..0x40)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
    else {
        return false;
    };
    bytes.get(offset..offset + 4) == Some(b"PE\0\0")
        && bytes.get(offset + 4..offset + 6) == Some(&[0x64, 0x86])
        && bytes.get(offset + 24..offset + 26) == Some(&[0x0b, 0x02])
}
fn verify_binary(binary: &Path) -> Result<(), Error> {
    let mut child = Command::new(binary)
        .arg("describe")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    // Drain output concurrently: describe exceeds some platforms' pipe buffers.
    let stdout = child.stdout.take().ok_or("No verification output.")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(262_145).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            let _ = reader.join();
            return Err("Binary verification timed out.".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader.join().map_err(|_| "Verification reader failed.")??;
    if !status.success()
        || bytes.len() > 262_144
        || contracts::parse_json(&bytes, 262_144)? != contracts::describe()
    {
        return Err("Executable does not implement the current docking contract.".into());
    }
    Ok(())
}

pub fn build(binary: &Path, output: &Path) -> Result<Value, Error> {
    if cfg!(not(all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    ))) {
        return Err("This package targets x86_64-pc-windows-msvc only.".into());
    }
    if fs::symlink_metadata(binary)?.file_type().is_symlink() || !binary.is_file() {
        return Err("Binary must be a regular native file.".into());
    }
    let binary = binary.canonicalize()?;
    let bytes = fs::read(&binary)?;
    if bytes.len() > 128 * 1024 * 1024 || !native_pe(&bytes) {
        return Err("Expected bounded x64 PE executable.".into());
    }
    verify_binary(&binary)?;
    if fs::read(&binary)? != bytes {
        return Err("Binary changed during verification.".into());
    }
    let mut payload = BTreeMap::from([
        (ENTRY.to_owned(), bytes),
        (
            "schemas/route-v2.json".to_owned(),
            include_bytes!("../schemas/route-v2.json").to_vec(),
        ),
        (
            "docs/coordinator.md".to_owned(),
            include_bytes!("../docs/coordinator.md").to_vec(),
        ),
        (
            "docs/agent-experience.ko.md".to_owned(),
            include_bytes!("../docs/agent-experience.ko.md").to_vec(),
        ),
        (
            "docs/connection.md".to_owned(),
            include_bytes!("../docs/connection.md").to_vec(),
        ),
        (
            "docs/stackbite.ko.md".to_owned(),
            include_bytes!("../docs/stackbite.ko.md").to_vec(),
        ),
        (
            "docs/workflow.ko.md".to_owned(),
            include_bytes!("../docs/workflow.ko.md").to_vec(),
        ),
        (
            "docs/automatic-pools.ko.md".to_owned(),
            include_bytes!("../docs/automatic-pools.ko.md").to_vec(),
        ),
        (
            "examples/workflow-auto.json".to_owned(),
            include_bytes!("../examples/workflow-auto.json").to_vec(),
        ),
        (
            "docs/workflow-ui.ko.md".to_owned(),
            include_bytes!("../docs/workflow-ui.ko.md").to_vec(),
        ),
        (
            "examples/workflow-minimal.json".to_owned(),
            include_bytes!("../examples/workflow-minimal.json").to_vec(),
        ),
        (
            "examples/stackbite-workflow.json".to_owned(),
            include_bytes!("../examples/stackbite-workflow.json").to_vec(),
        ),
        (
            "docs/stackbite-laya.ko.md".to_owned(),
            include_bytes!("../docs/stackbite-laya.ko.md").to_vec(),
        ),
        (
            "docs/evaluation.md".to_owned(),
            include_bytes!("../docs/evaluation.md").to_vec(),
        ),
        (
            "examples/agent-minimal.json".to_owned(),
            include_bytes!("../examples/agent-minimal.json").to_vec(),
        ),
        (
            "examples/evaluation-import-template.json".to_owned(),
            include_bytes!("../examples/evaluation-import-template.json").to_vec(),
        ),
        (
            "examples/evaluation-synthetic.json".to_owned(),
            include_bytes!("../examples/evaluation-synthetic.json").to_vec(),
        ),
        ("LICENSE".to_owned(), include_bytes!("../LICENSE").to_vec()),
        (
            "README.md".to_owned(),
            include_bytes!("../README.md").to_vec(),
        ),
        (
            "THIRD_PARTY_NOTICES.html".to_owned(),
            include_bytes!("../THIRD_PARTY_NOTICES.html").to_vec(),
        ),
        (
            "RUST_STANDARD_LIBRARY_NOTICES.html".to_owned(),
            include_bytes!("../RUST_STANDARD_LIBRARY_NOTICES.html").to_vec(),
        ),
        (
            "schemas/docking-v1.json".to_owned(),
            contracts::SCHEMA_JSON.as_bytes().to_vec(),
        ),
    ]);
    let files: BTreeMap<_, _> = payload
        .iter()
        .map(|(name, raw)| (name.clone(), sha256(raw)))
        .collect();
    let manifest = json!({"schema_version":2,"id":"switchbite","version":env!("CARGO_PKG_VERSION"),"docking_api":1,
        "transport":"mcp-stdio","entrypoints":{TARGET:ENTRY},"required_capabilities":[],
        "config_schema":{"type":"object","properties":{},"additionalProperties":false},"files":files});
    payload.insert("plugin.json".into(), serde_json::to_vec_pretty(&manifest)?);
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let destination = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(output)?;
    let mut zip = ZipWriter::new(destination);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::from_date_and_time(2026, 1, 1, 0, 0, 0)?)
        .unix_permissions(0o100644);
    for (name, raw) in &payload {
        zip.start_file(name, options)?;
        zip.write_all(raw)?;
    }
    let file = zip.finish()?;
    file.sync_all()?;
    let mut check = ZipArchive::new(fs::File::open(output)?)?;
    if check.len() != payload.len() {
        return Err("Archive verification failed.".into());
    }
    for (name, expected) in &payload {
        let mut raw = Vec::new();
        check.by_name(name)?.read_to_end(&mut raw)?;
        if &raw != expected {
            return Err("Archive verification failed.".into());
        }
    }
    Ok(
        json!({"plugin_id":"switchbite","version":env!("CARGO_PKG_VERSION"),"archive":output.canonicalize()?.to_string_lossy(),
        "sha256":sha256(&fs::read(output)?),"binary_sha256":files[ENTRY],"native_executable":binary.to_string_lossy(),"payload_files":files.len()}),
    )
}
