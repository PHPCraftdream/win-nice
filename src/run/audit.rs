use crate::Handle;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    IsProcessInJob, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    pub created: u64,
    pub image: String,
    pub same_user: Option<bool>,
    pub in_job: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspect {
    pub pid: u32,
    pub parent: u32,
    pub image: String,
}

/// Returns shown suspects and the count of new same-user processes that could
/// not be attributed through a validated ancestry chain and known job membership.
pub fn select_suspects(
    ps: &[Process],
    root: u32,
    root_created: u64,
    started: u64,
    cap: usize,
) -> (Vec<Suspect>, usize) {
    let map: HashMap<u32, &Process> = ps.iter().map(|p| (p.pid, p)).collect();
    let mut out = Vec::new();
    let mut unattributable = 0;
    for p in ps
        .iter()
        .filter(|p| p.created > started && p.same_user == Some(true) && p.pid != root)
    {
        let (mut pid, mut child_created, mut attributed) = (p.parent, p.created, false);
        let mut seen = HashSet::new();
        // Bound ancestry traversal; the root check consumes an iteration.
        for _ in 0..127 {
            if pid == root {
                let snapshot_matches = map
                    .get(&root)
                    .is_none_or(|entry| entry.created == root_created);
                attributed = snapshot_matches && root_created < child_created;
                break;
            }
            if !seen.insert(pid) {
                break;
            }
            let Some(parent) = map.get(&pid) else { break };
            if parent.created >= child_created {
                break;
            }
            child_created = parent.created;
            pid = parent.parent;
        }
        if attributed {
            match p.in_job {
                Some(false) => out.push(Suspect {
                    pid: p.pid,
                    parent: p.parent,
                    image: p.image.clone(),
                }),
                Some(true) => {}
                None => unattributable += 1,
            }
        } else {
            unattributable += 1;
        }
    }
    out.sort_by_key(|p| p.pid);
    out.truncate(cap);
    (out, unattributable)
}
fn ticks(t: &FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}
pub fn now_filetime() -> u64 {
    // SAFETY: FILETIME is a plain output struct; zero initialization is valid.
    let mut t: FILETIME = unsafe { std::mem::zeroed() };
    // SAFETY: GetSystemTimeAsFileTime initializes the valid writable pointer.
    unsafe { windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime(&mut t) };
    ticks(&t)
}
pub fn process_creation(handle: &Handle) -> Result<u64, String> {
    // SAFETY: FILETIME is a plain output struct; zero initialization is valid.
    let (mut c, mut x, mut k, mut u) = unsafe {
        (
            std::mem::zeroed(),
            std::mem::zeroed(),
            std::mem::zeroed(),
            std::mem::zeroed(),
        )
    };
    // SAFETY: handle is a valid process handle and each FILETIME pointer is a
    // writable output to a properly initialized struct.
    if unsafe { GetProcessTimes(handle.raw(), &mut c, &mut x, &mut k, &mut u) } == 0 {
        // SAFETY: GetLastError reads the calling thread's last Win32 error.
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        Err(format!("root process time query failed: {error}"))
    } else {
        Ok(ticks(&c))
    }
}
fn snapshot(job: &Handle) -> Result<Vec<Process>, String> {
    // SAFETY: snapshot flags are valid; zero requests the system process list.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap.is_null() || snap as isize == -1 {
        // SAFETY: GetLastError reads the calling thread's last Win32 error.
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(format!("snapshot failed: {error}"));
    }
    let same_user = |h: windows_sys::Win32::Foundation::HANDLE| -> Option<Vec<u8>> {
        let mut token = std::ptr::null_mut();
        // SAFETY: caller supplies a live process handle; TOKEN_QUERY requests
        // the token handle written to the output pointer.
        if unsafe { OpenProcessToken(h, 0x0008, &mut token) } == 0 {
            return None;
        }
        struct TokenGuard(windows_sys::Win32::Foundation::HANDLE);
        impl Drop for TokenGuard {
            fn drop(&mut self) {
                // SAFETY: guard owns the successful OpenProcessToken handle.
                unsafe { CloseHandle(self.0) };
            }
        }
        let _token_guard = TokenGuard(token);
        let word_len = 1024usize.div_ceil(std::mem::size_of::<usize>());
        let mut words = vec![0usize; word_len];
        let buffer = words.as_mut_ptr().cast::<u8>();
        let mut n = 0;
        // SAFETY: token is live, the aligned buffer provides at least 1024
        // writable bytes and n is the API's writable byte-count output.
        let ok = unsafe {
            windows_sys::Win32::Security::GetTokenInformation(token, 1, buffer.cast(), 1024, &mut n)
        } != 0;
        if !ok {
            return None;
        }
        // SAFETY: the aligned buffer contains a successful TOKEN_USER query;
        // read_unaligned additionally avoids relying on struct alignment.
        let sid = unsafe {
            std::ptr::read_unaligned(buffer.cast::<windows_sys::Win32::Security::TOKEN_USER>())
        }
        .User
        .Sid;
        // SAFETY: `sid` is non-null and points into the validated TOKEN_USER
        // result buffer, which remains live for this call.
        if sid.is_null() || unsafe { windows_sys::Win32::Security::IsValidSid(sid) } == 0 {
            return None;
        }
        // SAFETY: IsValidSid confirmed sid points to a valid SID.
        let len = unsafe { windows_sys::Win32::Security::GetLengthSid(sid) } as usize;
        if len == 0 || len > 1024 {
            return None;
        }
        // SAFETY: validated SID has exactly len readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), len) }.to_vec();
        Some(bytes)
    };
    // SAFETY: pseudo-handle is valid for the current process.
    let own = same_user(unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() });
    let result = (|| {
        // SAFETY: PROCESSENTRY32W is an output struct with valid zeroed state.
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        // SAFETY: snapshot is live and e is a writable struct with dwSize set.
        if unsafe { Process32FirstW(snap, &mut e) } == 0 {
            // SAFETY: GetLastError reads the calling thread's last Win32 error.
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            return Err(format!("snapshot first failed: {error}"));
        }
        let mut rows = Vec::new();
        loop {
            let n = e
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(e.szExeFile.len());
            let image = OsString::from_wide(&e.szExeFile[..n])
                .to_string_lossy()
                .into_owned();
            // SAFETY: PID came from the snapshot; requested access is limited.
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID) };
            if !h.is_null() {
                // SAFETY: FILETIME is a plain output struct; zero initialization is valid.
                let (mut c, mut x, mut k, mut u) = unsafe {
                    (
                        std::mem::zeroed(),
                        std::mem::zeroed(),
                        std::mem::zeroed(),
                        std::mem::zeroed(),
                    )
                };
                // SAFETY: h is live and all FILETIME pointers are writable.
                let known = unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) } != 0;
                let other_sid = same_user(h);
                let same_user = match (&own, other_sid) {
                    (Some(a), Some(b)) => Some(a == &b),
                    _ => None,
                };
                let mut member = 0;
                // SAFETY: h and job are valid handles; member is writable.
                let job_known = unsafe { IsProcessInJob(h, job.raw(), &mut member) } != 0;
                if known {
                    rows.push(Process {
                        pid: e.th32ProcessID,
                        parent: e.th32ParentProcessID,
                        created: ticks(&c),
                        image,
                        same_user,
                        in_job: job_known.then_some(member != 0),
                    });
                }
                // SAFETY: h is the valid handle returned by OpenProcess.
                unsafe { CloseHandle(h) };
            }
            // SAFETY: snapshot is valid and e is a writable entry struct.
            if unsafe { Process32NextW(snap, &mut e) } == 0 {
                break;
            }
        }
        Ok(rows)
    })();
    // SAFETY: snap is the valid snapshot handle created above.
    unsafe { CloseHandle(snap) };
    result
}

fn accounting_line(
    basic_ok: bool,
    peak_ok: bool,
    a: &JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    ext: &JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
) -> String {
    let sec = |v: i64| {
        let ms = v.max(0) / 10_000;
        format!("{}.{:03}s", ms / 1000, ms % 1000)
    };
    let field = |v: u32| if basic_ok { v.to_string() } else { "?".into() };
    let basic_time = |v: i64| if basic_ok { sec(v) } else { "?".into() };
    format!(
        "wnice audit: total={} active={} terminated={} user={} kernel={} peak-job-memory={}",
        field(a.TotalProcesses),
        field(a.ActiveProcesses),
        field(a.TotalTerminatedProcesses),
        basic_time(a.TotalUserTime),
        basic_time(a.TotalKernelTime),
        if peak_ok {
            ext.PeakJobMemoryUsed.to_string()
        } else {
            "?".into()
        }
    )
}

pub fn collect_and_report(
    job: &Handle,
    root: Option<u32>,
    root_created: Option<u64>,
    started: u64,
    err: &mut dyn FnMut(&str),
) -> Result<(), String> {
    // Build everything before emitting any report lines so a snapshot failure
    // produces exactly one diagnostic, never a partial report.
    // No root means command lookup failed; in that case only accounting is
    // available. A launched root without a creation time cannot be attributed.
    if root.is_some() && root_created.is_none() {
        return Err("root process time query failed".to_string());
    }
    let rows = snapshot(job)?;
    // SAFETY: `a` is a plain output struct and the API receives a live job,
    // writable buffer of the exact struct size, and writable return-length pointer.
    let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
    let mut ret = 0;
    let basic_ok = unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectBasicAccountingInformation,
            (&mut a as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            std::mem::size_of_val(&a) as u32,
            &mut ret,
        )
    } != 0;
    // SAFETY: `ext` is a plain output struct and the API receives a live job,
    // writable buffer of the exact struct size, and writable return-length pointer.
    let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: buffers are writable, sized for their information class, and job is live.
    let peak_ok = unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&mut ext as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&ext) as u32,
            &mut ret,
        )
    } != 0;
    let (suspects, unattributable, total_suspects) = match (root, root_created) {
        (Some(pid), Some(created)) => {
            let (all, count) = select_suspects(&rows, pid, created, started, usize::MAX);
            let total = all.len();
            (all.into_iter().take(20).collect::<Vec<_>>(), count, total)
        }
        _ => (Vec::new(), 0, 0),
    };
    err(&accounting_line(basic_ok, peak_ok, &a, &ext));

    if suspects.is_empty() {
        err("wnice audit: suspects: none");
    } else {
        for p in &suspects {
            err(&format!(
                "wnice audit suspect: pid={} image={} parent={}",
                p.pid, p.image, p.parent
            ));
        }
    }
    let omitted = total_suspects.saturating_sub(suspects.len());
    if omitted > 0 {
        err(&format!("wnice audit: ... and {omitted} more"));
    }
    if unattributable > 0 {
        err(&format!(
            "wnice audit: {unattributable} other new process(es) of this user, not attributable"
        ));
    }
    err("wnice audit limitations: Processes started by system services (WMI, Task Scheduler, COM servers, UAC), work done by reused servers that were already running, and anything whose parent chain broke are not visible.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(pid: u32, parent: u32, created: u64) -> Process {
        Process {
            pid,
            parent,
            created,
            image: format!("p{pid}.exe"),
            same_user: Some(true),
            in_job: Some(false),
        }
    }
    #[test]
    fn validated_ancestry_filters_reuse_cycle_depth_and_cap() {
        let ps = vec![p(1, 0, 10), p(2, 1, 20), p(3, 2, 30)];
        let (s, n) = select_suspects(&ps, 1, 10, 15, 20);
        assert_eq!(s.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(n, 0);
        let later_parent = vec![p(1, 0, 10), p(2, 3, 20), p(3, 1, 30)];
        let (later, unattributable) = select_suspects(&later_parent, 1, 10, 15, 20);
        assert_eq!(later.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![3]);
        assert_eq!(unattributable, 1); // p2 claims a parent created after p2.
                                       // Child-to-parent timestamps strictly decrease along a valid chain.
        let ordered = vec![p(1, 0, 10), p(2, 1, 20), p(3, 2, 30)];
        assert_eq!(select_suspects(&ordered, 1, 10, 15, 20).0.len(), 2);
        assert!(select_suspects(&ps, 1, 25, 15, 20).0.is_empty()); // reused root PID
        assert!(select_suspects(&[p(1, 0, 10), p(2, 1, 15)], 1, 10, 15, 20)
            .0
            .is_empty()); // strict start
        let mut cycle = vec![p(1, 0, 10), p(2, 3, 20), p(3, 2, 30)];
        assert!(select_suspects(&cycle, 1, 10, 15, 20).0.is_empty());
        cycle = vec![p(1, 0, 10)];
        for i in 2..132 {
            cycle.push(p(i, i - 1, i as u64 + 10));
        }
        let (deep, deep_unattributable) = select_suspects(&cycle, 1, 10, 11, 200);
        assert!(deep.iter().any(|s| s.pid == 128));
        assert!(!deep.iter().any(|s| s.pid == 129));
        assert!(deep_unattributable > 0);
        let mut many = vec![p(1, 0, 10)];
        for i in 2..30 {
            many.push(p(i, 1, i as u64 + 10));
        }
        let (shown, unattrib) = select_suspects(&many, 1, 10, 11, 20);
        assert_eq!(shown.len(), 20);
        assert_eq!(unattrib, 0);
        assert_eq!(
            select_suspects(&many, 1, 10, 11, usize::MAX).0.len() - shown.len(),
            8
        );
    }
    #[test]
    fn unattributable_count_includes_in_job_and_unknown_membership() {
        let root = p(1, 0, 10);
        let mut broken_in_job = p(2, 99, 20);
        broken_in_job.in_job = Some(true);
        let (suspects, count) = select_suspects(&[root.clone(), broken_in_job], 1, 10, 15, 20);
        assert!(suspects.is_empty());
        assert_eq!(count, 1);

        let mut unknown_membership = p(3, 1, 21);
        unknown_membership.in_job = None;
        let (suspects, count) = select_suspects(&[root.clone(), unknown_membership], 1, 10, 15, 20);
        assert!(suspects.is_empty());
        assert_eq!(count, 1);

        let attributed_in_job = Process {
            in_job: Some(true),
            ..p(4, 1, 22)
        };
        let (suspects, count) = select_suspects(&[root, attributed_in_job], 1, 10, 15, 20);
        assert!(suspects.is_empty());
        assert_eq!(count, 0);
    }
    #[test]
    fn accounting_format_marks_every_failed_query_unknown() {
        // SAFETY: false query flags make the values in these structs unread.
        let a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: false query flags make the values in these structs unread.
        let ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        assert_eq!(
            accounting_line(false, false, &a, &ext),
            "wnice audit: total=? active=? terminated=? user=? kernel=? peak-job-memory=?"
        );
    }
    #[test]
    fn unknown_queries_and_filters_are_not_suspects() {
        let mut x = p(2, 1, 20);
        x.same_user = None;
        let mut y = p(3, 1, 21);
        y.in_job = None;
        let z = Process {
            in_job: Some(true),
            ..p(4, 1, 22)
        };
        let other = Process {
            same_user: Some(false),
            ..p(5, 1, 23)
        };
        assert!(
            select_suspects(&[p(1, 0, 10), x, y, z, other], 1, 10, 15, 20)
                .0
                .is_empty()
        );
    }
}
