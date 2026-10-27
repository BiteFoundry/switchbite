//! Process-tree ownership using Windows jobs. A suspended process is placed in
//! a kill-on-close job before its initial thread is resumed. See Microsoft:
//! https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
#[cfg(windows)]
mod windows {
    use std::{
        io,
        mem::size_of,
        os::windows::{
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::CommandExt,
        },
        process::{Child, Command},
        ptr, thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, FILETIME,
            GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE, SetLastError,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
                OpenJobObjectW, QueryInformationJobObject, SetInformationJobObject,
                TerminateJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, GetCurrentProcess, GetExitCodeProcess,
                GetProcessTimes, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
                ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    };
    fn handle(raw: HANDLE) -> io::Result<OwnedHandle> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: this is a newly returned, exclusively owned Windows handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw as _) })
    }
    pub struct Tree {
        job: OwnedHandle,
        pub child: Child,
    }
    impl Tree {
        pub fn creation_time(&self) -> io::Result<u64> {
            creation(self.child.as_raw_handle() as _)
        }
        pub fn spawn(cmd: &mut Command, name: &str) -> io::Result<Self> {
            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: null attributes and a valid nul-terminated job name.
            unsafe {
                SetLastError(0);
            }
            let raw = unsafe { CreateJobObjectW(ptr::null(), name.as_ptr()) };
            let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            let job = handle(raw)?;
            if already_exists {
                return Err(io::Error::other("job name already exists"));
            }
            let mut limit: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: correctly typed and sized limit record and live job handle.
            if unsafe {
                SetInformationJobObject(
                    job.as_raw_handle() as _,
                    JobObjectExtendedLimitInformation,
                    (&limit as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            cmd.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
            let mut child = cmd.spawn()?;
            // SAFETY: the process is suspended and both handles are live.
            if unsafe {
                AssignProcessToJobObject(job.as_raw_handle() as _, child.as_raw_handle() as _)
            } == 0
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::last_os_error());
            }
            if let Err(error) = resume_primary(child.id()) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            Ok(Self { job, child })
        }
        pub fn active(&self) -> io::Result<u32> {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: correctly typed out buffer and a live job handle.
            if unsafe {
                QueryInformationJobObject(
                    self.job.as_raw_handle() as _,
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(info.ActiveProcesses)
        }
        pub fn terminate(&mut self) -> io::Result<()> {
            // SAFETY: the job is owned by this instance; no external PID is targeted.
            if unsafe { TerminateJobObject(self.job.as_raw_handle() as _, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.active()? != 0 {
                if Instant::now() >= deadline {
                    return Err(io::Error::other("job did not drain"));
                }
                thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.wait();
            Ok(())
        }
    }
    fn creation(handle: HANDLE) -> io::Result<u64> {
        let mut made = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: live process handle and four valid output records.
        if unsafe { GetProcessTimes(handle, &mut made, &mut exit, &mut kernel, &mut user) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((u64::from(made.dwHighDateTime) << 32) | u64::from(made.dwLowDateTime))
    }
    pub fn owner_creation_time() -> io::Result<u64> {
        // SAFETY: the current-process pseudo handle remains valid during this call.
        creation(unsafe { GetCurrentProcess() })
    }
    /// `Some(true)` proves the identified process exited or the PID was reused;
    /// `None` means access or identity is ambiguous.
    pub fn stopped(pid: u32, expected_creation: u64) -> Option<bool> {
        // SAFETY: opening a process for query only never signals or mutates it.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            let error = io::Error::last_os_error().raw_os_error()? as u32;
            return (error == ERROR_INVALID_PARAMETER).then_some(true);
        }
        let process = handle(raw).ok()?;
        if creation(process.as_raw_handle() as _).ok()? != expected_creation {
            return Some(true);
        }
        let mut code = 0;
        // SAFETY: a live query-only handle and writable exit-code output.
        if unsafe { GetExitCodeProcess(process.as_raw_handle() as _, &mut code) } == 0 {
            return None;
        }
        Some(code != STILL_ACTIVE as u32)
    }
    pub fn job_drained(name: &str) -> Option<bool> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: read-only JOB_OBJECT_QUERY access to the named owned job.
        let raw = unsafe { OpenJobObjectW(0x0004, 0, name.as_ptr()) };
        if raw.is_null() {
            let error = io::Error::last_os_error().raw_os_error()? as u32;
            return (error == ERROR_FILE_NOT_FOUND).then_some(true);
        }
        let job = handle(raw).ok()?;
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: correctly typed buffer and read-only handle.
        if unsafe {
            QueryInformationJobObject(
                job.as_raw_handle() as _,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                ptr::null_mut(),
            )
        } == 0
        {
            return None;
        }
        Some(info.ActiveProcesses == 0)
    }
    fn resume_primary(pid: u32) -> io::Result<()> {
        // SAFETY: snapshot handle is independently owned.
        let snapshot = handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        // SAFETY: valid snapshot and correctly initialized entry.
        let mut found = unsafe { Thread32First(snapshot.as_raw_handle() as _, &mut entry) } != 0;
        while found {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: open only this suspended process's thread for resume.
                let thread =
                    handle(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
                if unsafe { ResumeThread(thread.as_raw_handle() as _) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            found = unsafe { Thread32Next(snapshot.as_raw_handle() as _, &mut entry) } != 0;
        }
        Err(io::Error::other("suspended process thread not found"))
    }
}
#[cfg(windows)]
pub use windows::{Tree, job_drained, owner_creation_time, stopped};
#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn process_identity_distinguishes_a_reused_pid() {
        let now = super::owner_creation_time().unwrap();
        assert_eq!(super::stopped(std::process::id(), now), Some(false));
        assert_eq!(super::stopped(std::process::id(), now + 1), Some(true));
    }
}
#[cfg(not(windows))]
pub struct Tree {
    pub child: std::process::Child,
}
#[cfg(not(windows))]
impl Tree {
    pub fn creation_time(&self) -> std::io::Result<u64> {
        Err(std::io::Error::other("process identity unsupported"))
    }
    pub fn spawn(_: &mut std::process::Command, _: &str) -> std::io::Result<Self> {
        Err(std::io::Error::other("process tree ownership unsupported"))
    }
    pub fn active(&self) -> std::io::Result<u32> {
        Err(std::io::Error::other("process tree ownership unsupported"))
    }
    pub fn terminate(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("process tree ownership unsupported"))
    }
}
#[cfg(not(windows))]
pub fn owner_creation_time() -> std::io::Result<u64> {
    Err(std::io::Error::other("process identity unsupported"))
}
#[cfg(not(windows))]
pub fn stopped(_: u32, _: u64) -> Option<bool> {
    None
}
#[cfg(not(windows))]
pub fn job_drained(_: &str) -> Option<bool> {
    None
}
