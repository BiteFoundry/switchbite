//! Durable per-run workspace lookup for unbound MCP servers.
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
static MUTATION: Mutex<()> = Mutex::new(());
pub struct Index {
    root: Option<PathBuf>,
}
impl Default for Index {
    fn default() -> Self {
        let data = std::env::var_os(if cfg!(windows) {
            "LOCALAPPDATA"
        } else {
            "XDG_DATA_HOME"
        })
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|p| PathBuf::from(p).join(".local/share"))
        });
        Self {
            root: data
                .filter(|p| p.is_absolute())
                .map(|p| p.join("Switchbite").join("workflow-index")),
        }
    }
}
impl Index {
    pub fn new(root: PathBuf) -> Self {
        Self { root: Some(root) }
    }
    fn locked(&self) -> Result<(std::sync::MutexGuard<'static, ()>, fs::File, PathBuf), Value> {
        let guard = MUTATION.lock().map_err(|_| storage_error())?;
        let root = self.root.clone().ok_or_else(storage_error)?;
        fs::create_dir_all(&root).map_err(|_| storage_error())?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".lock"))
            .map_err(|_| storage_error())?;
        lock_file(&lock)?;
        Ok((guard, lock, root))
    }
    fn path(root: &Path, run: &str) -> Result<PathBuf, Value> {
        if run.is_empty()
            || run.len() > 128
            || !run
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(not_found());
        }
        Ok(root.join(format!("{run}.json")))
    }
    pub fn insert(&self, run: &str, workspace: &Path) -> Result<(), Value> {
        let (_guard, _lock, root) = self.locked()?;
        let path = Self::path(&root, run)?;
        if path.exists() {
            return Err(storage_error());
        }
        let temp = root.join(format!(".{run}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|_| storage_error())?;
            serde_json::to_writer(&mut file, &json!({"workspace":workspace}))
                .map_err(|_| storage_error())?;
            file.flush()
                .and_then(|_| file.sync_all())
                .map_err(|_| storage_error())?;
            fs::rename(&temp, path).map_err(|_| storage_error())
        })();
        let _ = fs::remove_file(temp);
        result
    }
    pub fn lookup(&self, run: &str) -> Result<PathBuf, Value> {
        let (_guard, _lock, root) = self.locked()?;
        let bytes = fs::read(Self::path(&root, run)?).map_err(|_| not_found())?;
        if bytes.len() > 32768 {
            return Err(not_found());
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| not_found())?;
        let path = value["workspace"]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(not_found)?;
        if !path.is_absolute() || !path.is_dir() {
            return Err(not_found());
        }
        Ok(path)
    }
}
fn not_found() -> Value {
    json!({"error":{"code":"not_found","message":"Run not found in the workspace index. Check the run ID or start a new run."}})
}
fn storage_error() -> Value {
    json!({"error":{"code":"storage_error","message":"Unable to save or read the workspace index. Check user data directory permissions."}})
}
#[cfg(unix)]
fn lock_file(file: &fs::File) -> Result<(), Value> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    if unsafe { flock(file.as_raw_fd(), 2) } == 0 {
        Ok(())
    } else {
        Err(storage_error())
    }
}
#[cfg(windows)]
fn lock_file(file: &fs::File) -> Result<(), Value> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut std::ffi::c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFileEx(
            file: *mut std::ffi::c_void,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    let mut o = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: std::ptr::null_mut(),
    };
    if unsafe { LockFileEx(file.as_raw_handle(), 2, 0, 1, 0, &mut o) } != 0 {
        Ok(())
    } else {
        Err(storage_error())
    }
}
