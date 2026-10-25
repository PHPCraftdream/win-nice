//! Shared fake `Sys` implementation used by the run engine's test
//! modules. Extracted from `run/tests.rs` so sibling test files can use the
//! same call-recording / failure-injection fake.

use super::*;
use crate::{Child, Handle};
use std::cell::{Cell, RefCell};

#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) enum Call {
    SetJobInfo {
        class: u32,
        flags: u32,
    },
    CreateProcess {
        app: Option<String>,
        cmdline: String,
        flags: u32,
    },
    Assign,
    Resume,
    CreateTimer,
    SetTimer {
        due: i64,
    },
    Wait,
    GetProcessTimes,
    GetExitCode,
    TerminateProcess,
    TerminateJobObject,
}

pub(crate) struct FakeSys {
    pub(crate) handles_minted: Cell<u64>,
    calls: RefCell<Vec<Call>>,
    /// Each entry fails the (skip+1)-th NEXT call to the named fn, once,
    /// with `code`; earlier calls pass through untouched.
    fail: RefCell<Vec<(&'static str, u32, u32)>>,
    next_handle: Cell<u64>,
    wait_result: Result<u32, u32>,
    exit_time: i64,
    /// When set, get_process_times returns (recorded timer due + delta) —
    /// lets tie-break tests express exit time relative to the deadline
    /// the run itself computed.
    exit_time_delta: Option<i64>,
    recorded_due: Cell<i64>,
    exit_code: u32,
}

impl FakeSys {
    pub(crate) fn new() -> Self {
        FakeSys {
            calls: RefCell::new(Vec::new()),
            fail: RefCell::new(Vec::new()),
            next_handle: Cell::new(0x1000_0000),
            handles_minted: Cell::new(0),
            wait_result: Ok(0),
            exit_time: 0,
            exit_time_delta: None,
            recorded_due: Cell::new(0),
            exit_code: 0,
        }
    }
    pub(crate) fn fail_next(self, name: &'static str, code: u32) -> Self {
        self.fail_nth(name, 0, code)
    }
    /// Fail the (skip+1)-th next call to `name` (0 = the very next one).
    pub(crate) fn fail_nth(self, name: &'static str, skip: u32, code: u32) -> Self {
        self.fail.borrow_mut().push((name, skip, code));
        self
    }
    pub(crate) fn wait_on(mut self, index: u32) -> Self {
        self.wait_result = Ok(index);
        self
    }
    pub(crate) fn exit_code(mut self, code: u32) -> Self {
        self.exit_code = code;
        self
    }
    #[expect(dead_code)] // kept for future absolute-exit-time tests
    fn exit_time(mut self, t: i64) -> Self {
        self.exit_time = t;
        self
    }
    #[allow(dead_code)]
    pub(crate) fn exit_time_delta(mut self, delta: i64) -> Self {
        self.exit_time_delta = Some(delta);
        self
    }
    pub(crate) fn handle(&self) -> Handle {
        self.handles_minted.set(self.handles_minted.get() + 1);
        let v = self.next_handle.get() + 1;
        self.next_handle.set(v);
        Handle::from_raw_no_close(v as usize as *mut core::ffi::c_void)
    }
    pub(crate) fn log(&self, c: Call) {
        self.calls.borrow_mut().push(c);
    }
    pub(crate) fn take(&self) -> Vec<Call> {
        self.calls.borrow_mut().drain(..).collect()
    }
    pub(crate) fn count(&self, pred: impl Fn(&Call) -> bool) -> usize {
        self.calls.borrow().iter().filter(|c| pred(c)).count()
    }
    pub(crate) fn check(&self, name: &str) -> Result<(), u32> {
        let mut q = self.fail.borrow_mut();
        if let Some(pos) = q.iter().position(|(n, _, _)| *n == name) {
            let (_, skip, code) = q[pos];
            if skip > 0 {
                q[pos].1 = skip - 1;
                return Ok(());
            }
            q.remove(pos);
            return Err(code);
        }
        Ok(())
    }
    /// Check the plain failure key, then the class-suffixed key
    /// (`"<fn>:<class>"`) when one was queued for this info class. Lets a
    /// test target a specific SetInformationJobObject call (e.g. the
    /// CPU-rate apply, class 15) instead of just the next one.
    pub(crate) fn check_class(&self, name: &str, class: u32) -> Result<(), u32> {
        self.check(name)?;
        self.check(&format!("{name}:{class}"))
    }
    pub(crate) fn set_job_calls(&self) -> Vec<(u32, u32)> {
        self.take()
            .into_iter()
            .filter_map(|c| match c {
                Call::SetJobInfo { class, flags } => Some((class, flags)),
                _ => None,
            })
            .collect()
    }
}

impl Sys for FakeSys {
    fn create_job_object(&self) -> Result<Handle, u32> {
        self.check("CreateJobObject")?;
        Ok(self.handle())
    }
    fn set_information_job_object(&self, _h: &Handle, class: u32, bytes: &[u8]) -> Result<(), u32> {
        // Plain key first, then class-suffixed key (e.g. ":15" targets the
        // CPU-rate apply only).
        self.check_class("SetInformationJobObject", class)?;
        // LimitFlags offset derived from the real windows-sys layout (not
        // hardcoded): sum of the two top-level offset_of! lookups.
        const FLAGS_OFF: usize = std::mem::offset_of!(
            windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            BasicLimitInformation
        ) + std::mem::offset_of!(
            windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_LIMIT_INFORMATION,
            LimitFlags
        );
        let flags =
            if class == crate::JobObjectExtendedLimitInformation && bytes.len() >= FLAGS_OFF + 4 {
                u32::from_le_bytes(bytes[FLAGS_OFF..FLAGS_OFF + 4].try_into().unwrap())
            } else {
                0
            };
        self.log(Call::SetJobInfo { class, flags });
        Ok(())
    }
    fn create_process(
        &self,
        app_name: Option<&str>,
        cmdline: &str,
        creation_flags: u32,
    ) -> Result<Child, u32> {
        self.check("CreateProcess")?;
        self.log(Call::CreateProcess {
            app: app_name.map(str::to_string),
            cmdline: cmdline.to_string(),
            flags: creation_flags,
        });
        Ok(Child {
            process: self.handle(),
            thread: self.handle(),
            pid: 4242,
        })
    }
    fn assign_process_to_job_object(&self, _: &Handle, _: &Handle) -> Result<(), u32> {
        self.check("AssignProcessToJobObject")?;
        self.log(Call::Assign);
        Ok(())
    }
    fn resume_thread(&self, _: &Handle) -> Result<(), u32> {
        self.check("ResumeThread")?;
        self.log(Call::Resume);
        Ok(())
    }
    fn create_waitable_timer(&self) -> Result<Handle, u32> {
        self.check("CreateWaitableTimer")?;
        self.log(Call::CreateTimer);
        Ok(self.handle())
    }
    fn set_waitable_timer(&self, _h: &Handle, due: i64) -> Result<(), u32> {
        self.check("SetWaitableTimer")?;
        self.recorded_due.set(due);
        self.log(Call::SetTimer { due });
        Ok(())
    }
    fn wait_for_multiple_objects(&self, _hs: &[&Handle], _all: bool) -> Result<u32, u32> {
        self.check("WaitForMultipleObjects")?;
        self.log(Call::Wait);
        self.wait_result
    }
    fn get_process_times(&self, _p: &Handle) -> Result<i64, u32> {
        self.check("GetProcessTimes")?;
        self.log(Call::GetProcessTimes);
        if let Some(d) = self.exit_time_delta {
            Ok(self.recorded_due.get() + d)
        } else {
            Ok(self.exit_time)
        }
    }
    fn get_exit_code_process(&self, _p: &Handle) -> Result<u32, u32> {
        self.check("GetExitCodeProcess")?;
        self.log(Call::GetExitCode);
        Ok(self.exit_code)
    }
    fn terminate_process(&self, _p: &Handle, _code: u32) -> Result<(), u32> {
        self.check("TerminateProcess")?;
        self.log(Call::TerminateProcess);
        Ok(())
    }
    fn terminate_job_object(&self, _j: &Handle, _code: u32) -> Result<(), u32> {
        self.check("TerminateJobObject")?;
        self.log(Call::TerminateJobObject);
        Ok(())
    }
    fn global_memory_status_ex(&self) -> Result<u64, u32> {
        Ok(8 * 1024 * 1024 * 1024)
    }
    fn logical_processor_count(&self) -> u32 {
        8
    }
    fn system_directory(&self) -> String {
        "C:\\Windows\\System32".to_string()
    }
}
