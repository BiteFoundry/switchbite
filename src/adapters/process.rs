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
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
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
        pub fn spawn(cmd: &mut Command) -> io::Result<Self> {
            // SAFETY: null attributes create an unnamed private job.
            let job = handle(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
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
pub use windows::Tree;
#[cfg(not(windows))]
pub struct Tree {
    pub child: std::process::Child,
}
#[cfg(not(windows))]
impl Tree {
    pub fn spawn(_: &mut std::process::Command) -> std::io::Result<Self> {
        Err(std::io::Error::other("process tree ownership unsupported"))
    }
    pub fn active(&self) -> std::io::Result<u32> {
        Err(std::io::Error::other("process tree ownership unsupported"))
    }
    pub fn terminate(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("process tree ownership unsupported"))
    }
}
