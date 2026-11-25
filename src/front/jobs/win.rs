//! Read-only Win32/NT inspection primitives for [`crate::front::jobs`].
//!
//! This is the only module under `crate::front::jobs` with `unsafe`. Every
//! function is a small wrapper that opens nothing for writing and bounds every
//! read: handle snapshots are walked only within the bytes the kernel returned.

use super::{CapState, CpuCap, LimitFields};
use crate::Handle;
use std::mem::size_of;
use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;
use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
use windows_sys::Win32::Foundation::{
    DuplicateHandle, FILETIME, HANDLE, STATUS_INFO_LENGTH_MISMATCH,
};
use windows_sys::Win32::Security::{
    CopySid, EqualSid, GetLengthSid, GetTokenInformation, IsValidSid, TokenUser, PSID,
    SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, IsProcessInJob, JobObjectBasicAccountingInformation,
    JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_AFFINITY, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_PRIORITY_CLASS,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_DUP_HANDLE,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// `SystemExtendedHandleInformation`; windows-sys 0.59 exports no constant.
const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 64;
/// `ProcessCommandLineInformation`; windows-sys 0.59 exports no constant.
const PROCESS_COMMAND_LINE_INFORMATION: i32 = 60;
/// `JOB_OBJECT_QUERY`, the job object's query access right; windows-sys 0.59
/// exports no constant for it.
const JOB_OBJECT_QUERY: u32 = 0x0004;
/// First handle-snapshot size and the growth step on a length mismatch.
const SNAPSHOT_START: usize = 1 << 20;
const SNAPSHOT_SLACK: usize = 64 * 1024;
/// Snapshot/query attempts before giving up (the table can grow between calls).
const ATTEMPTS: usize = 4;

/// Number of logical processors on this machine.
pub fn logical_cpus() -> u32 {
    unsafe {
        let mut info: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut info);
        info.dwNumberOfProcessors
    }
}

/// Open a wrapper process for handle-table walking and querying.
pub fn open_owner(pid: u32) -> Option<Handle> {
    open(pid, PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION)
}

/// Open a process for querying only (command line, CPU time, job membership).
pub fn open_query(pid: u32) -> Option<Handle> {
    open(pid, PROCESS_QUERY_LIMITED_INFORMATION)
}

fn open(pid: u32, access: u32) -> Option<Handle> {
    unsafe {
        let h = OpenProcess(access, 0, pid);
        if h.is_null() {
            None
        } else {
            Some(Handle::from_raw(h))
        }
    }
}

/// `UNICODE_STRING` as `NtQueryInformationProcess` writes it (16 bytes on x64).
/// Every field is part of the layout; `maximum_length` is not otherwise read.
#[repr(C)]
#[allow(dead_code)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

/// The full command line of `proc`, or `None` when it cannot be read.
pub fn command_line(proc: &Handle) -> Option<String> {
    let mut len = 1 << 12;
    // One retry: on a length mismatch the kernel reports the size it needs.
    for _ in 0..2 {
        let mut buf = vec![0u8; len];
        let mut needed: u32 = 0;
        let status = unsafe {
            NtQueryInformationProcess(
                proc.raw(),
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut needed,
            )
        };
        if status == 0 {
            return decode_command_line(&buf, needed as usize);
        }
        if status != STATUS_INFO_LENGTH_MISMATCH {
            return None;
        }
        let next = needed as usize;
        if next <= len {
            return None;
        }
        len = next;
    }
    None
}

/// Read the `UNICODE_STRING` header out of `buf` and copy its characters.
/// The buffer pointer must land inside `buf`; anything else is rejected.
fn decode_command_line(buf: &[u8], written: usize) -> Option<String> {
    let header = size_of::<UnicodeString>();
    if buf.len() < header {
        return None;
    }
    let us: UnicodeString = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
    let bytes = us.length as usize;
    if us.buffer.is_null() || !bytes.is_multiple_of(2) {
        return None;
    }
    let avail = if written == 0 || written > buf.len() {
        buf.len()
    } else {
        written
    };
    let offset = (us.buffer as usize).checked_sub(buf.as_ptr() as usize)?;
    if offset < header || offset.checked_add(bytes)? > avail {
        return None;
    }
    let src = buf[offset..].as_ptr().cast::<u16>();
    let units: Vec<u16> = (0..bytes / 2)
        .map(|i| unsafe { std::ptr::read_unaligned(src.add(i)) })
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Our own user SID, copied out of the process token so it outlives the
/// token handle.
pub struct OwnedSid {
    bytes: Vec<u8>,
}

impl OwnedSid {
    /// The copy as a SID pointer; `CopySid` validated it before storing.
    fn as_psid(&self) -> PSID {
        self.bytes.as_ptr() as PSID
    }
}

/// Our own user SID; `None` when the token or the SID query failed.
pub fn current_user() -> Option<OwnedSid> {
    let token = token_of(unsafe { GetCurrentProcess() })?;
    let bytes = token_sid(&token)?;
    Some(OwnedSid { bytes })
}

/// Whether `proc` runs as the same user as `own`; `None` when the token/SID
/// query failed, `false` for a different user.
pub fn same_user(proc: &Handle, own: &OwnedSid) -> Option<bool> {
    let token = token_of(proc.raw())?;
    let other = token_sid(&token)?;
    Some(unsafe { EqualSid(own.as_psid(), other.as_ptr() as PSID) } != 0)
}

/// A query-only handle on `process`'s token.
fn token_of(process: HANDLE) -> Option<Handle> {
    let mut token: HANDLE = std::ptr::null_mut();
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    if ok == 0 || token.is_null() {
        None
    } else {
        Some(Handle::from_raw(token))
    }
}

/// Copy of the `TokenUser` SID inside `token`, sized by `GetLengthSid`.
fn token_sid(token: &Handle) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; size_of::<TOKEN_USER>() + SECURITY_MAX_SID_SIZE as usize];
    let mut needed: u32 = 0;
    let ok = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }
    let user: TOKEN_USER = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
    let sid = user.User.Sid;
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return None;
    }
    let len = unsafe { GetLengthSid(sid) } as usize;
    if len == 0 || len > SECURITY_MAX_SID_SIZE as usize {
        return None;
    }
    let mut out = vec![0u8; len];
    if unsafe { CopySid(len as u32, out.as_mut_ptr().cast(), sid) } == 0 {
        return None;
    }
    Some(out)
}

/// `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`: one fixed-size entry of the extended
/// handle snapshot (40 bytes on x64, never hardcoded). Every field is part of
/// the layout; only the object address, the pid, the handle value and the type
/// index are read.
#[repr(C)]
#[allow(dead_code)]
struct EntryInfoEx {
    object: *mut std::ffi::c_void,
    unique_process_id: usize,
    handle_value: usize,
    granted_access: u32,
    creator_back_trace_index: u16,
    object_type_index: u16,
    handle_attributes: u32,
    reserved: u32,
}

/// One handle of one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Kernel job-object address used for de-duplication.
    pub object: usize,
    pub handle_value: usize,
    pub object_type_index: u16,
}

/// One `SystemExtendedHandleInformation` snapshot of the whole system.
pub struct HandleTable {
    buf: Vec<u8>,
}

impl HandleTable {
    /// Take a snapshot, retrying on `STATUS_INFO_LENGTH_MISMATCH` with the
    /// length the kernel asks for plus slack (the table grows between calls).
    pub fn query() -> Option<HandleTable> {
        let mut len = SNAPSHOT_START;
        for _ in 0..ATTEMPTS {
            let mut buf = vec![0u8; len];
            let mut needed: u32 = 0;
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_EXTENDED_HANDLE_INFORMATION,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    &mut needed,
                )
            };
            if status == 0 {
                return Some(HandleTable { buf });
            }
            if status != STATUS_INFO_LENGTH_MISMATCH {
                return None;
            }
            let next = needed as usize;
            if next <= len {
                return None;
            }
            len = next.saturating_add(SNAPSHOT_SLACK);
        }
        None
    }

    /// Every handle `pid` owns, in kernel order.
    pub fn entries_of(&self, pid: u32) -> Vec<Entry> {
        let mut out = Vec::new();
        let Some(count) = self.count() else {
            return out;
        };
        let stride = size_of::<EntryInfoEx>();
        let header = 2 * size_of::<usize>();
        for i in 0..count {
            let Some(base) = i.checked_mul(stride).and_then(|v| v.checked_add(header)) else {
                break;
            };
            // Never read past the bytes the snapshot actually holds.
            if base.saturating_add(stride) > self.buf.len() {
                break;
            }
            let entry: EntryInfoEx =
                unsafe { std::ptr::read_unaligned(self.buf.as_ptr().add(base).cast()) };
            if entry.unique_process_id == pid as usize {
                out.push(Entry {
                    object: entry.object as usize,
                    handle_value: entry.handle_value,
                    object_type_index: entry.object_type_index,
                });
            }
        }
        out
    }

    /// `NumberOfHandles` of the snapshot header.
    fn count(&self) -> Option<usize> {
        let header = 2 * size_of::<usize>();
        if self.buf.len() < header {
            return None;
        }
        Some(unsafe { std::ptr::read_unaligned(self.buf.as_ptr().cast::<usize>()) })
    }
}

/// The `ObjectTypeIndex` of a job object handle, discovered by creating one:
/// the OS-version-dependent index never has to be hardcoded.
pub fn job_type_index(table: &HandleTable) -> Option<u16> {
    // The probe must exist before the snapshot that looks for it, so take a
    // fresh one; the caller's table is the fallback if that query fails.
    let probe = unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return None;
        }
        Handle::from_raw(h)
    };
    let want = probe.raw() as usize;
    let me = std::process::id();
    let find = |t: &HandleTable| {
        t.entries_of(me)
            .into_iter()
            .find(|e| e.handle_value == want)
            .map(|e| e.object_type_index)
    };
    match HandleTable::query() {
        Some(fresh) => find(&fresh).or_else(|| find(table)),
        None => find(table),
    }
}

/// Duplicate one of `owner`'s job handles into this process, query-only.
pub fn duplicate_job(owner: &Handle, handle_value: usize) -> Option<Handle> {
    let mut dup: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        DuplicateHandle(
            owner.raw(),
            handle_value as HANDLE,
            GetCurrentProcess(),
            &mut dup,
            JOB_OBJECT_QUERY,
            0,
            0,
        )
    };
    if ok == 0 || dup.is_null() {
        None
    } else {
        Some(Handle::from_raw(dup))
    }
}

/// The limits and activity counters of one job object, each part carrying its
/// own unknown state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLimits {
    /// CPU cap: known-absent, known, or not known.
    pub cpu: CapState,
    /// `None` when the extended-limit query failed.
    pub extended: Option<LimitFields>,
    /// `None` when the accounting query failed.
    pub active: Option<u32>,
}

impl JobLimits {
    /// True when every query that produced this value succeeded.
    pub fn queries_ok(&self) -> bool {
        self.cpu != CapState::Unknown && self.extended.is_some() && self.active.is_some()
    }
}

/// Read the limits of `job`. Never fails wholesale: a query that fails leaves
/// only its own part unknown.
pub fn job_limits(job: &Handle) -> JobLimits {
    JobLimits {
        cpu: cpu_state(job),
        extended: extended_limits(job),
        active: active_processes(job),
    }
}

/// The CPU rate cap: `Unknown` on a failed query, `Uncapped` without `ENABLE`.
fn cpu_state(job: &Handle) -> CapState {
    let mut rate: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectCpuRateControlInformation,
            (&mut rate as *mut JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
            size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            &mut returned,
        )
    };
    if ok == 0 {
        return CapState::Unknown;
    }
    if (rate.ControlFlags & JOB_OBJECT_CPU_RATE_CONTROL_ENABLE) == 0 {
        return CapState::Uncapped;
    }
    // SAFETY: CpuRate is the plain-u32 member the rate control path wrote.
    CapState::Capped(CpuCap {
        cpu_rate: unsafe { rate.Anonymous.CpuRate },
        hard: (rate.ControlFlags & JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP) != 0,
    })
}

/// The `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` fields; `None` on a failed query.
fn extended_limits(job: &Handle) -> Option<LimitFields> {
    let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&mut ext as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            &mut returned,
        )
    };
    if ok == 0 {
        return None;
    }
    let flags = ext.BasicLimitInformation.LimitFlags;
    Some(LimitFields {
        job_memory: (flags & JOB_OBJECT_LIMIT_JOB_MEMORY != 0).then_some(ext.JobMemoryLimit as u64),
        active_process_limit: (flags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0)
            .then_some(ext.BasicLimitInformation.ActiveProcessLimit),
        affinity: (flags & JOB_OBJECT_LIMIT_AFFINITY != 0)
            .then_some(ext.BasicLimitInformation.Affinity as u64),
        priority: (flags & JOB_OBJECT_LIMIT_PRIORITY_CLASS != 0)
            .then_some(ext.BasicLimitInformation.PriorityClass),
    })
}

/// `JobObjectBasicAccountingInformation::ActiveProcesses`; `None` on failure.
fn active_processes(job: &Handle) -> Option<u32> {
    let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectBasicAccountingInformation,
            (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            &mut returned,
        )
    };
    (ok != 0).then_some(accounting.ActiveProcesses)
}

/// Whether `proc` is a member of `job`; `None` when the query itself failed.
pub fn is_in_job(proc: &Handle, job: &Handle) -> Option<bool> {
    let mut result: i32 = 0;
    let ok = unsafe { IsProcessInJob(proc.raw(), job.raw(), &mut result) };
    (ok != 0).then_some(result != 0)
}

/// Kernel plus user CPU time of `proc`, in 100 ns units.
pub fn cpu_time_100ns(proc: &Handle) -> Option<u64> {
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    let ok =
        unsafe { GetProcessTimes(proc.raw(), &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    Some(ticks(&kernel) + ticks(&user))
}

/// Creation time of `proc` (FILETIME ticks): with the PID it names one process.
pub fn creation_time(proc: &Handle) -> Option<u64> {
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    let ok =
        unsafe { GetProcessTimes(proc.raw(), &mut creation, &mut exit, &mut kernel, &mut user) };
    (ok != 0).then(|| ticks(&creation))
}

fn ticks(t: &FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_ticks_combine_the_two_halves() {
        let t = FILETIME {
            dwLowDateTime: 1,
            dwHighDateTime: 2,
        };
        assert_eq!(ticks(&t), (2u64 << 32) | 1);
        assert_eq!(
            ticks(&FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0
            }),
            0
        );
    }

    #[test]
    fn entry_info_matches_the_documented_x64_size() {
        assert_eq!(size_of::<EntryInfoEx>(), 3 * size_of::<usize>() + 16);
        assert_eq!(size_of::<UnicodeString>(), 2 * size_of::<usize>());
    }

    #[test]
    fn entry_starts_with_the_object_address() {
        // The object address is what makes two handles the same job object.
        assert_eq!(std::mem::offset_of!(EntryInfoEx, object), 0);
        let entry = Entry {
            object: 0x7ff6_0000,
            handle_value: 4,
            object_type_index: 7,
        };
        assert_eq!(entry.object, 0x7ff6_0000);
        assert_eq!(entry.handle_value, 4);
        assert_eq!(entry.object_type_index, 7);
    }

    #[test]
    fn decode_rejects_a_foreign_or_empty_buffer() {
        // Zeroed header: a null buffer, so nothing to decode.
        assert_eq!(decode_command_line(&[0u8; 32], 32), None);
        assert_eq!(decode_command_line(&[], 0), None);
    }

    #[test]
    fn limits_are_ok_only_when_every_query_succeeded() {
        let ok = JobLimits {
            cpu: CapState::Uncapped,
            extended: Some(LimitFields::default()),
            active: Some(0),
        };
        assert!(ok.queries_ok());
        let capped = JobLimits {
            cpu: CapState::Capped(CpuCap {
                cpu_rate: 5000,
                hard: true,
            }),
            ..ok
        };
        assert!(capped.queries_ok());
        // Any unknown part makes the whole reading incomplete.
        assert!(!JobLimits {
            cpu: CapState::Unknown,
            ..ok
        }
        .queries_ok());
        assert!(!JobLimits {
            extended: None,
            ..ok
        }
        .queries_ok());
        assert!(!JobLimits { active: None, ..ok }.queries_ok());
    }
}
