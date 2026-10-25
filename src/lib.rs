//! wnice core library: job-object based process limits on Windows.

pub mod cli;
pub mod front;
pub mod launch;
pub mod limits;
pub mod run;

/// RAII wrapper around a raw Win32 HANDLE. Not Clone.
///
/// `Drop` calls `CloseHandle` exactly once, directly via windows-sys.
/// CloseHandle is intentionally NOT part of the `Sys` trait — RAII owns
/// closing.
pub struct Handle {
    raw: windows_sys::Win32::Foundation::HANDLE,
    /// Test-only: fake handles minted by `FakeSys` must never reach the real
    /// `CloseHandle`; when `false`, `Drop` only clears the pointer. Real
    /// constructors always set `true`.
    #[cfg(test)]
    close_on_drop: bool,
}

/// Test-only drop counter: incremented once per fake (no-close) handle drop,
/// so tests can assert RAII single-close behavior without real kernel objects.
#[cfg(test)]
pub(crate) mod handle_test {
    use std::cell::Cell;
    thread_local! {
        pub static FAKE_HANDLE_DROPS: Cell<u32> = const { Cell::new(0) };
    }
}

impl Handle {
    /// Constructor for `RealSys` only.
    pub fn from_raw(raw: windows_sys::Win32::Foundation::HANDLE) -> Handle {
        Handle {
            raw,
            #[cfg(test)]
            close_on_drop: true,
        }
    }

    /// Test-only constructor for `FakeSys`: stores the raw value but never
    /// passes it to the real `CloseHandle` on drop.
    #[cfg(test)]
    pub(crate) fn from_raw_no_close(raw: windows_sys::Win32::Foundation::HANDLE) -> Handle {
        Handle {
            raw,
            #[cfg(test)]
            close_on_drop: false,
        }
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
        if !self.close_on_drop {
            handle_test::FAKE_HANDLE_DROPS.with(|c| c.set(c.get() + 1));
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
    pub command: Vec<String>,
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

/// Format a Win32 error as `<Fn> failed: <code>`.
pub fn win32_err(fn_name: &str, code: u32) -> String {
    format!("{fn_name} failed: {code}")
}

/// Abstraction over the fallible native calls (object-safe).
pub trait Sys {
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
}

/// Real implementation backed by windows-sys.
pub struct RealSys;

use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, CreateWaitableTimerW, GetExitCodeProcess, GetProcessTimes, ResumeThread,
    SetWaitableTimer, TerminateProcess, WaitForMultipleObjects, INFINITE, PROCESS_INFORMATION,
    STARTUPINFOW,
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
}
