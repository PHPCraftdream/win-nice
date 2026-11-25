//! wnice core library: job-object based process limits on Windows.

pub mod cli;
pub mod front;
pub mod launch;
pub mod limits;
pub mod run;

fn windows_environment_block(environment: &[(std::ffi::OsString, std::ffi::OsString)]) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    // Sorted by name, case-insensitively (ASCII upper-cased, as the OS does for
    // names): `ProgramFiles` before `ProgramFiles(x86)`. The `=` of an entry
    // is no part of the name, so it must not take part in the comparison.
    let upper = |unit: u16| {
        if (b'a' as u16..=b'z' as u16).contains(&unit) {
            unit - 32
        } else {
            unit
        }
    };
    let mut sorted: Vec<&(std::ffi::OsString, std::ffi::OsString)> = environment.iter().collect();
    sorted.sort_by_cached_key(|(key, _)| key.encode_wide().map(upper).collect::<Vec<u16>>());
    let entries: Vec<Vec<u16>> = sorted
        .into_iter()
        .map(|(key, value)| {
            let mut entry: Vec<u16> = key.encode_wide().collect();
            entry.push(b'=' as u16);
            entry.extend(value.encode_wide());
            entry.push(0);
            entry
        })
        .collect();
    let mut block: Vec<u16> = entries.into_iter().flatten().collect();
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    block
}

#[cfg(test)]
mod windows_environment_block_tests {
    use super::windows_environment_block;
    use std::ffi::OsString;

    #[test]
    fn entries_are_sorted_by_name_case_insensitively() {
        let pairs = [
            ("ProgramFiles(x86)", "a"),
            ("zeta", "1"),
            ("ProgramFiles", "b"),
            ("Alpha", "2"),
            ("=C:", "C:\\x"),
            ("alpha2", "3"),
        ];
        let env: Vec<(OsString, OsString)> = pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect();
        let block = windows_environment_block(&env);
        let text: Vec<String> = block
            .split(|unit| *unit == 0)
            .filter(|entry| !entry.is_empty())
            .map(String::from_utf16_lossy)
            .collect();
        assert_eq!(
            text,
            [
                "=C:=C:\\x",
                "Alpha=2",
                "alpha2=3",
                "ProgramFiles=b",
                "ProgramFiles(x86)=a",
                "zeta=1",
            ]
        );
        assert_eq!(&block[block.len() - 2..], [0, 0]);
    }

    #[test]
    fn empty_environment_has_double_nul_terminator() {
        assert_eq!(windows_environment_block(&[]), [0, 0]);
    }

    #[test]
    fn non_empty_environment_has_double_nul_terminator() {
        let block = windows_environment_block(&[(OsString::from("KEY"), OsString::from("value"))]);
        assert_eq!(
            block,
            [
                b'K' as u16,
                b'E' as u16,
                b'Y' as u16,
                b'=' as u16,
                b'v' as u16,
                b'a' as u16,
                b'l' as u16,
                b'u' as u16,
                b'e' as u16,
                0,
                0,
            ]
        );
        assert_eq!(&block[block.len() - 2..], [0, 0]);
    }
}

fn no_servers_environment<F>(
    inherited: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    choose_port: F,
) -> (Vec<(std::ffi::OsString, std::ffi::OsString)>, bool)
where
    F: FnOnce() -> std::io::Result<u16>,
{
    use std::ffi::OsString;
    let mut vars: Vec<(OsString, OsString)> = inherited.into_iter().collect();
    let has = |name: &str, vars: &[(OsString, OsString)]| {
        vars.iter()
            .any(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
    };
    for (key, value) in [
        ("MSBUILDDISABLENODEREUSE", "1"),
        ("UseSharedCompilation", "false"),
    ] {
        if !has(key, &vars) {
            vars.push((key.into(), value.into()));
        }
    }
    if !has("GRADLE_OPTS", &vars) {
        vars.push(("GRADLE_OPTS".into(), "-Dorg.gradle.daemon=false".into()));
    } else if let Some((_, value)) = vars
        .iter_mut()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("GRADLE_OPTS"))
    {
        let current = value.to_string_lossy();
        if !current.contains("org.gradle.daemon=") {
            value.push(" -Dorg.gradle.daemon=false");
        }
    }
    let port_failed = if has("SCCACHE_SERVER_PORT", &vars) {
        false
    } else {
        match choose_port() {
            Ok(port) => vars.push(("SCCACHE_SERVER_PORT".into(), port.to_string().into())),
            Err(_) => return (vars, true),
        }
        false
    };
    (vars, port_failed)
}

#[cfg(test)]
mod no_servers_tests {
    use super::no_servers_environment;
    use std::ffi::OsString;

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    fn get(vars: &[(OsString, OsString)], key: &str) -> Option<String> {
        vars.iter()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(key))
            .map(|(_, v)| v.to_string_lossy().into_owned())
    }

    #[test]
    fn injects_unset_server_controls_and_port() {
        let (vars, failed) = no_servers_environment(env(&[("PATH", "x")]), || Ok(1234));
        assert!(!failed);
        assert_eq!(get(&vars, "MSBUILDDISABLENODEREUSE").as_deref(), Some("1"));
        assert_eq!(get(&vars, "UseSharedCompilation").as_deref(), Some("false"));
        assert_eq!(
            get(&vars, "GRADLE_OPTS").as_deref(),
            Some("-Dorg.gradle.daemon=false")
        );
        assert_eq!(get(&vars, "SCCACHE_SERVER_PORT").as_deref(), Some("1234"));
    }

    #[test]
    fn preserves_user_values_and_gradle_daemon_option() {
        let (vars, failed) = no_servers_environment(
            env(&[
                ("MSBUILDDISABLENODEREUSE", "0"),
                ("UseSharedCompilation", "true"),
                ("GRADLE_OPTS", "-Xmx2g -Dorg.gradle.daemon=true"),
                ("SCCACHE_SERVER_PORT", "4321"),
            ]),
            || panic!("must not choose port when user supplied it"),
        );
        assert!(!failed);
        assert_eq!(get(&vars, "MSBUILDDISABLENODEREUSE").as_deref(), Some("0"));
        assert_eq!(get(&vars, "UseSharedCompilation").as_deref(), Some("true"));
        assert_eq!(
            get(&vars, "GRADLE_OPTS").as_deref(),
            Some("-Xmx2g -Dorg.gradle.daemon=true")
        );
        assert_eq!(get(&vars, "SCCACHE_SERVER_PORT").as_deref(), Some("4321"));
    }

    #[test]
    fn appends_gradle_option_and_reports_port_failure() {
        let (vars, failed) = no_servers_environment(env(&[("GRADLE_OPTS", "-Xmx2g")]), || {
            Err(std::io::Error::other("no port"))
        });
        assert!(failed);
        assert_eq!(
            get(&vars, "GRADLE_OPTS").as_deref(),
            Some("-Xmx2g -Dorg.gradle.daemon=false")
        );
        assert_eq!(get(&vars, "SCCACHE_SERVER_PORT"), None);
    }
}

/// RAII wrapper around a raw Win32 HANDLE. Not Clone.
///
/// `Drop` calls `CloseHandle` exactly once, directly via windows-sys.
/// CloseHandle is intentionally NOT part of the `Sys` trait — RAII owns
/// closing. Test builds additionally carry a per-instance accounting book
/// on fake handles (see `FakeHandleBook`); there is no global drop counter.
pub struct Handle {
    raw: windows_sys::Win32::Foundation::HANDLE,
    /// Test-only: fake handles minted by `FakeSys` must never reach the real
    /// `CloseHandle`; when `false`, `Drop` skips the real close (booked
    /// handles increment their book instead). Real constructors set `true`.
    #[cfg(test)]
    close_on_drop: bool,
    /// Test-only: per-instance accounting for fake handles (`FakeSys`-minted
    /// handles carry `Some`); `Drop` increments `dropped` instead of touching
    /// any global.
    #[cfg(test)]
    book: Option<std::sync::Arc<FakeHandleBook>>,
}

/// Test-only, per-`FakeSys`-instance handle accounting: every fake handle
/// minted by one `FakeSys` shares its `Arc<FakeHandleBook>`, so mint/drop
/// counts stay isolated per test even when `cargo test` runs tests in
/// parallel threads, and drops on the priority booster thread land in the
/// owning test's book (an `Arc` is Send + Sync; no global counters).
#[cfg(test)]
pub(crate) struct FakeHandleBook {
    pub(crate) minted: std::sync::atomic::AtomicU64,
    pub(crate) dropped: std::sync::atomic::AtomicU64,
}

impl Handle {
    /// Constructor for `RealSys` only.
    pub fn from_raw(raw: windows_sys::Win32::Foundation::HANDLE) -> Handle {
        Handle {
            raw,
            #[cfg(test)]
            close_on_drop: true,
            #[cfg(test)]
            book: None,
        }
    }

    /// Test-only constructor for `FakeSys`: stores the raw value but never
    /// passes it to the real `CloseHandle` on drop; no accounting — used for
    /// re-owned raw values that arrive without a book.
    #[cfg(test)]
    pub(crate) fn from_raw_no_close(raw: windows_sys::Win32::Foundation::HANDLE) -> Handle {
        Handle {
            raw,
            #[cfg(test)]
            close_on_drop: false,
            #[cfg(test)]
            book: None,
        }
    }

    /// Test-only constructor for `FakeSys`-minted handles: never reaches the
    /// real `CloseHandle` on drop; its drop is counted in the owning
    /// instance's book.
    #[cfg(test)]
    pub(crate) fn from_raw_no_close_booked(
        raw: windows_sys::Win32::Foundation::HANDLE,
        book: std::sync::Arc<FakeHandleBook>,
    ) -> Handle {
        Handle {
            raw,
            #[cfg(test)]
            close_on_drop: false,
            #[cfg(test)]
            book: Some(book),
        }
    }

    /// The book this fake handle reports its drop to (`None` for real
    /// handles).
    #[cfg(test)]
    pub(crate) fn book(&self) -> Option<std::sync::Arc<FakeHandleBook>> {
        self.book.clone()
    }

    pub fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.raw
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        #[cfg(not(test))]
        if !self.raw.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.raw);
            }
        }
        #[cfg(test)]
        if self.close_on_drop && !self.raw.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.raw);
            }
        }
        #[cfg(test)]
        if let Some(book) = &self.book {
            // Fake handle: never a real kernel object — only its instance's
            // drop counter moves (works from any thread, incl. the booster's).
            book.dropped
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.raw = std::ptr::null_mut();
    }
}

/// Native child process info from `create_process`.
pub struct Child {
    pub process: Handle,
    pub thread: Handle,
    pub pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityClass {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl PriorityClass {
    pub fn to_u32(self) -> u32 {
        match self {
            PriorityClass::Idle => 0x40,          // IDLE_PRIORITY_CLASS
            PriorityClass::BelowNormal => 0x4000, // BELOW_NORMAL_PRIORITY_CLASS
            PriorityClass::Normal => 0x20,        // NORMAL_PRIORITY_CLASS
            PriorityClass::AboveNormal => 0x8000, // ABOVE_NORMAL_PRIORITY_CLASS
            PriorityClass::High => 0x80,          // HIGH_PRIORITY_CLASS
            PriorityClass::Realtime => 0x100,     // REALTIME_PRIORITY_CLASS
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemSpec {
    Percent(u32),
    Bytes(u64),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub priority: Option<PriorityClass>,
    pub cpu_percent: Option<u32>,
    pub threads: Option<u32>,
    pub memory: Option<MemSpec>,
    pub procs: Option<u32>,
    pub timeout_ms: Option<u32>,
    pub boost: bool,
    pub audit: bool,
    pub command: Vec<String>,
    /// WSL mode: when set, `run` dispatches to the WSL guest engine
    /// (`run::wsl`) instead of the Job Object engine. `None` (the default)
    /// keeps Windows behavior untouched.
    pub wsl: Option<crate::run::wsl::WslMode>,
    pub no_servers: bool,
    pub adopt: Vec<String>,
}

// Constants
pub const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
pub const JOB_OBJECT_LIMIT_AFFINITY: u32 = 0x10;
pub const JOB_OBJECT_LIMIT_JOB_MEMORY: u32 = 0x200;
pub const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x8;
pub const JOB_OBJECT_LIMIT_PRIORITY_CLASS: u32 = 0x20;
// Info-class codes kept at their Win32 names per spec.
#[allow(non_upper_case_globals)]
pub const JobObjectExtendedLimitInformation: u32 = 9;
#[allow(non_upper_case_globals)]
pub const JobObjectCpuRateControlInformation: u32 = 15;
pub const JOB_OBJECT_CPU_RATE_CONTROL_ENABLE: u32 = 0x1;
pub const JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP: u32 = 0x4;
pub const CREATE_SUSPENDED: u32 = 0x4;
pub const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
// windows-sys 0.59 does not export the JOB_OBJECT_MSG_* constants (the W1
// spike used the literal 6); message id 4 is ACTIVE_PROCESS_ZERO, NOT
// END_OF_JOB_TIME.
pub const JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO: u32 = 4;
pub const JOB_OBJECT_MSG_NEW_PROCESS: u32 = 6;
#[allow(non_upper_case_globals)]
pub const JobObjectAssociateCompletionPortInformation: u32 = 7;
pub const PROCESS_SET_INFORMATION: u32 = 0x0200;
pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
/// GetQueuedCompletionStatus timeout: no packet within the deadline.
pub const WAIT_TIMEOUT: u32 = 0x102;

/// Format a Win32 error as `<Fn> failed: <code>`.
pub fn win32_err(fn_name: &str, code: u32) -> String {
    format!("{fn_name} failed: {code}")
}

/// Abstraction over the fallible native calls (object-safe). Implementations
/// must be Send + Sync because the priority booster thread uses the same
/// implementation.
pub trait Sys: Send + Sync {
    fn create_job_object(&self) -> Result<Handle, u32>;
    fn set_information_job_object(
        &self,
        h: &Handle,
        info_class: u32,
        bytes: &[u8],
    ) -> Result<(), u32>;
    fn create_process(
        &self,
        app_name: Option<&str>,
        cmdline: &str,
        creation_flags: u32,
    ) -> Result<Child, u32>;
    fn create_process_with_environment(
        &self,
        app_name: Option<&str>,
        cmdline: &str,
        creation_flags: u32,
        environment: &[(std::ffi::OsString, std::ffi::OsString)],
    ) -> Result<Child, u32> {
        let _ = environment;
        self.create_process(app_name, cmdline, creation_flags)
    }
    fn assign_process_to_job_object(&self, job: &Handle, proc: &Handle) -> Result<(), u32>;
    fn resume_thread(&self, t: &Handle) -> Result<(), u32>;
    fn create_waitable_timer(&self) -> Result<Handle, u32>;
    fn set_waitable_timer(&self, h: &Handle, due_time_filetime: i64) -> Result<(), u32>;
    fn wait_for_multiple_objects(&self, handles: &[&Handle], wait_all: bool) -> Result<u32, u32>;
    fn get_process_times(&self, p: &Handle) -> Result<i64, u32>;
    fn get_exit_code_process(&self, p: &Handle) -> Result<u32, u32>;
    fn terminate_process(&self, p: &Handle, code: u32) -> Result<(), u32>;
    fn terminate_job_object(&self, j: &Handle, code: u32) -> Result<(), u32>;
    fn global_memory_status_ex(&self) -> Result<u64, u32>;
    fn logical_processor_count(&self) -> u32;
    fn system_directory(&self) -> String;
    /// Create a new I/O completion port (the booster's job-message queue).
    fn create_completion_port(&self) -> Result<Handle, u32>;
    /// Associate a Job Object with a completion port so job notifications are
    /// queued there (`JOBOBJECT_ASSOCIATE_COMPLETION_PORT` via
    /// SetInformationJobObject). The key is 0 and unused.
    fn associate_job_with_completion_port(&self, job: &Handle, port: &Handle) -> Result<(), u32>;
    /// Dequeue one job notification from a completion port. `Ok((message, pid))`
    /// is a dequeued packet where `message` is the JOB_OBJECT_MSG_* id (read from
    /// the transferred-byte-count out parameter) and `pid` is the new process id
    /// (read from the lpOverlapped VALUE, not from what it points at — the
    /// runtime behavior confirmed by the W1 spike differs from the old MSDN
    /// wording). `Err(WAIT_TIMEOUT)` means no packet arrived within `timeout_ms`;
    /// any other error is a real port failure.
    fn get_queued_completion_status(
        &self,
        port: &Handle,
        timeout_ms: u32,
    ) -> Result<(u32, usize), u32>;
    /// Open a process by id with the given access mask.
    fn open_process(&self, pid: u32, access: u32) -> Result<Handle, u32>;
    /// Set a process's priority class (`class` is a PRIORITY_CLASS_* value).
    fn set_priority_class(&self, p: &Handle, class: u32) -> Result<(), u32>;
}

/// Real implementation backed by windows-sys.
pub struct RealSys;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, CreateWaitableTimerW, GetExitCodeProcess, GetProcessTimes, OpenProcess,
    ResumeThread, SetPriorityClass, SetWaitableTimer, TerminateProcess, WaitForMultipleObjects,
    CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};

impl Sys for RealSys {
    fn create_job_object(&self) -> Result<Handle, u32> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return Err(windows_sys::Win32::Foundation::GetLastError());
            }
            Ok(Handle::from_raw(h))
        }
    }

    fn set_information_job_object(
        &self,
        h: &Handle,
        info_class: u32,
        bytes: &[u8],
    ) -> Result<(), u32> {
        unsafe {
            if SetInformationJobObject(
                h.raw(),
                info_class as i32,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
            ) == 0
            {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(())
            }
        }
    }

    fn create_process(
        &self,
        app_name: Option<&str>,
        cmdline: &str,
        creation_flags: u32,
    ) -> Result<Child, u32> {
        let app_wide: Option<Vec<u16>> = app_name.map(|a| {
            let mut v: Vec<u16> = a.encode_utf16().collect();
            v.push(0);
            v
        });
        let app_ptr = app_wide.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
        let mut wide: Vec<u16> = cmdline.encode_utf16().collect();
        wide.push(0);
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        unsafe {
            // Optional explicit application name (lpApplicationName), NULL
            // env/cwd, bInheritHandles = TRUE (mirrors the existing C#
            // DllImports exactly). The mutable command line buffer may be
            // written in place by CreateProcessW.
            if CreateProcessW(
                app_ptr,
                wide.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // bInheritHandles = TRUE
                creation_flags,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            ) == 0
            {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(Child {
                    process: Handle::from_raw(pi.hProcess),
                    thread: Handle::from_raw(pi.hThread),
                    pid: pi.dwProcessId,
                })
            }
        }
    }

    fn create_process_with_environment(
        &self,
        app_name: Option<&str>,
        cmdline: &str,
        creation_flags: u32,
        environment: &[(std::ffi::OsString, std::ffi::OsString)],
    ) -> Result<Child, u32> {
        let app_wide: Option<Vec<u16>> = app_name.map(|a| {
            let mut v: Vec<u16> = a.encode_utf16().collect();
            v.push(0);
            v
        });
        let app_ptr = app_wide.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
        let mut wide: Vec<u16> = cmdline.encode_utf16().collect();
        wide.push(0);
        let mut env_wide = windows_environment_block(environment);
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: The pointers refer to live NUL-terminated UTF-16 command and environment
        // buffers, and initialized STARTUPINFO/PROCESS_INFORMATION, for the duration of
        // CreateProcessW.
        unsafe {
            if CreateProcessW(
                app_ptr,
                wide.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                creation_flags | CREATE_UNICODE_ENVIRONMENT,
                env_wide.as_mut_ptr().cast(),
                std::ptr::null(),
                &si,
                &mut pi,
            ) == 0
            {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(Child {
                    process: Handle::from_raw(pi.hProcess),
                    thread: Handle::from_raw(pi.hThread),
                    pid: pi.dwProcessId,
                })
            }
        }
    }

    fn assign_process_to_job_object(&self, job: &Handle, proc: &Handle) -> Result<(), u32> {
        unsafe {
            if AssignProcessToJobObject(job.raw(), proc.raw()) == 0 {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(())
            }
        }
    }

    fn resume_thread(&self, t: &Handle) -> Result<(), u32> {
        let prev = unsafe { ResumeThread(t.raw()) };
        // 0xFFFFFFFF (-1 as u32) indicates failure.
        if prev == 0xFFFF_FFFF {
            Err(unsafe { windows_sys::Win32::Foundation::GetLastError() })
        } else {
            Ok(())
        }
    }

    fn create_waitable_timer(&self) -> Result<Handle, u32> {
        unsafe {
            // manual-reset = TRUE, anonymous (no name)
            let h = CreateWaitableTimerW(std::ptr::null(), 1, std::ptr::null());
            if h.is_null() {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(Handle::from_raw(h))
            }
        }
    }

    fn set_waitable_timer(&self, h: &Handle, due_time_filetime: i64) -> Result<(), u32> {
        unsafe {
            // one-shot (period 0), no completion routine (NULL APC), fResume = FALSE
            if SetWaitableTimer(
                h.raw(),
                &due_time_filetime,
                0,
                None,
                std::ptr::null_mut(),
                0,
            ) == 0
            {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(())
            }
        }
    }

    fn wait_for_multiple_objects(&self, handles: &[&Handle], wait_all: bool) -> Result<u32, u32> {
        let raw: Vec<HANDLE> = handles.iter().map(|h| h.raw()).collect();
        unsafe {
            let n = raw.len() as u32;
            let rc = WaitForMultipleObjects(n, raw.as_ptr(), wait_all as i32, INFINITE);
            if rc == 0xFFFF_FFFF {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else if rc < WAIT_OBJECT_0 + n {
                Ok(rc - WAIT_OBJECT_0)
            } else {
                Err(rc)
            }
        }
    }

    fn get_process_times(&self, p: &Handle) -> Result<i64, u32> {
        let mut creation: i64 = 0;
        let mut exit: i64 = 0;
        let mut kernel: i64 = 0;
        let mut user: i64 = 0;
        unsafe {
            if GetProcessTimes(
                p.raw(),
                &mut creation as *mut i64 as *mut _,
                &mut exit as *mut i64 as *mut _,
                &mut kernel as *mut i64 as *mut _,
                &mut user as *mut i64 as *mut _,
            ) == 0
            {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(exit)
            }
        }
    }

    fn get_exit_code_process(&self, p: &Handle) -> Result<u32, u32> {
        let mut code: u32 = 0;
        unsafe {
            if GetExitCodeProcess(p.raw(), &mut code) == 0 {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(code)
            }
        }
    }

    fn terminate_process(&self, p: &Handle, code: u32) -> Result<(), u32> {
        unsafe {
            if TerminateProcess(p.raw(), code) == 0 {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(())
            }
        }
    }

    fn terminate_job_object(&self, j: &Handle, code: u32) -> Result<(), u32> {
        unsafe {
            if TerminateJobObject(j.raw(), code) == 0 {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(())
            }
        }
    }

    fn global_memory_status_ex(&self) -> Result<u64, u32> {
        let mut ms: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        ms.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        unsafe {
            if GlobalMemoryStatusEx(&mut ms) == 0 {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(ms.ullTotalPhys)
            }
        }
    }

    fn logical_processor_count(&self) -> u32 {
        unsafe {
            let mut si: SYSTEM_INFO = std::mem::zeroed();
            GetSystemInfo(&mut si);
            si.dwNumberOfProcessors
        }
    }

    fn system_directory(&self) -> String {
        unsafe {
            let mut buf = [0u16; 512];
            let len = windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                buf.as_mut_ptr(),
                buf.len() as u32,
            );
            if len == 0 || len as usize >= buf.len() {
                return String::new();
            }
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }

    fn create_completion_port(&self) -> Result<Handle, u32> {
        unsafe {
            // NULL template/file handle + 0 max-threads creates a NEW port.
            let h = CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 0);
            if h.is_null() {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(Handle::from_raw(h))
            }
        }
    }

    fn associate_job_with_completion_port(&self, job: &Handle, port: &Handle) -> Result<(), u32> {
        // Serialize JOBOBJECT_ASSOCIATE_COMPLETION_PORT field by field into a
        // zeroed buffer (same discipline as limits::extended_bytes — the struct's
        // repr(C) padding must never be viewed as raw bytes).
        use std::mem::{offset_of, size_of};
        use windows_sys::Win32::System::JobObjects::JOBOBJECT_ASSOCIATE_COMPLETION_PORT as A;
        let mut buf = vec![0u8; size_of::<A>()];
        let put = |buf: &mut [u8], off: usize, bytes: &[u8]| {
            buf[off..off + bytes.len()].copy_from_slice(bytes);
        };
        // CompletionKey is 0 (unused); CompletionPort is the port handle. Both
        // fields are pointer-sized; write their raw usize values.
        put(
            &mut buf,
            offset_of!(A, CompletionKey),
            &0usize.to_le_bytes(),
        );
        put(
            &mut buf,
            offset_of!(A, CompletionPort),
            &(port.raw() as usize).to_le_bytes(),
        );
        unsafe {
            if SetInformationJobObject(
                job.raw(),
                crate::JobObjectAssociateCompletionPortInformation as i32,
                buf.as_ptr().cast(),
                buf.len() as u32,
            ) == 0
            {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(())
            }
        }
    }

    fn get_queued_completion_status(
        &self,
        port: &Handle,
        timeout_ms: u32,
    ) -> Result<(u32, usize), u32> {
        unsafe {
            let mut bytes: u32 = 0;
            let mut key: usize = 0;
            let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
            if GetQueuedCompletionStatus(
                port.raw(),
                &mut bytes,
                &mut key,
                &mut overlapped,
                timeout_ms,
            ) == 0
            {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                // The job queues the NEW process id as the lpOverlapped VALUE
                // (W1 spike, 92/92 processes): cast the pointer value, never
                // dereference it.
                Ok((bytes, overlapped as usize))
            }
        }
    }

    fn open_process(&self, pid: u32, access: u32) -> Result<Handle, u32> {
        unsafe {
            let h = OpenProcess(access, 0, pid);
            if h.is_null() {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(Handle::from_raw(h))
            }
        }
    }

    fn set_priority_class(&self, p: &Handle, class: u32) -> Result<(), u32> {
        unsafe {
            if SetPriorityClass(p.raw(), class) == 0 {
                Err(windows_sys::Win32::Foundation::GetLastError())
            } else {
                Ok(())
            }
        }
    }
}
