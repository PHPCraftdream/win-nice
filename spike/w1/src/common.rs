//! Shared machinery for the w1-spike crate: jobs, child processes, boosters,
//! timing, elevation and output helpers. Experimental spike code, not product.

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectAssociateCompletionPortInformation,
    JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_ASSOCIATE_COMPLETION_PORT,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0_0, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PRIORITY_CLASS,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::SystemInformation::{GetNativeSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcess, GetCurrentThread, GetExitCodeProcess, GetPriorityClass,
    GetProcessTimes, GetThreadPriority, OpenProcess, OpenProcessToken, ResumeThread,
    SetPriorityClass, SetThreadPriority, TerminateProcess, WaitForMultipleObjects, INFINITE,
    HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, PROCESS_TERMINATE, PROCESS_INFORMATION, STARTUPINFOW,
    THREAD_PRIORITY_HIGHEST, THREAD_PRIORITY_NORMAL,
};

/// Win32 exit-code probe value (windows-sys exposes STILL_ACTIVE as the i32
/// NTSTATUS type; GetExitCodeProcess yields u32).
const STILL_ACTIVE_U32: u32 = 0x103;

/// JOB_OBJECT_MSG_NEW_PROCESS. windows-sys 0.59 ships no JOB_OBJECT_MSG_*
/// constants, so the literal is used (verified against winnt.h semantics).
pub const JOB_OBJECT_MSG_NEW_PROCESS: u32 = 6;
/// JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO (message-count table only).
pub const JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO: u32 = 4;
/// MAXIMUM_WAIT_OBJECTS lives in Win32::System::SystemServices in windows-sys
/// 0.59 (feature not enabled); the value is fixed at 64 by the ABI.
pub const MAXIMUM_WAIT_OBJECTS: usize = 64;

// ---------------------------------------------------------------- output ---

/// Print one `key=value` line to a locked, flushed stdout.
pub fn kv(key: &str, val: impl std::fmt::Display) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{key}={val}");
    let _ = out.flush();
}

// ----------------------------------------------------------------- args ----

/// Value of `--name value` or `--name=value`, if present.
pub fn arg_val(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(&format!("{name}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

/// True when the exact flag token is present.
pub fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

// ----------------------------------------------------------------- misc ----

pub fn self_exe() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `<cwd>/out`, created.
pub fn out_dir() -> PathBuf {
    let d = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("out");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// `<cwd>/out/<rel>`, created.
pub fn out_sub(rel: &str) -> PathBuf {
    let d = out_dir().join(rel);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Parse a plain key=value text record file (one `key=value` per line).
pub fn read_kv_file(path: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

/// Look up one key in a parsed record.
pub fn get_kv<'a>(rec: &'a [(String, String)], key: &str) -> Option<&'a str> {
    rec.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Write a key=value record file.
pub fn write_kv_file(path: &std::path::Path, rec: &[(&str, String)]) {
    let mut text = String::new();
    for (k, v) in rec {
        text.push_str(k);
        text.push('=');
        text.push_str(v);
        text.push('\n');
    }
    let _ = std::fs::write(path, text);
}

pub fn median(v: &mut Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    }
}

/// Nearest-rank percentile (0..=100).
pub fn percentile(v: &mut Vec<f64>, p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((p / 100.0) * (v.len() as f64 - 1.0)).round() as usize;
    v[idx.min(v.len() - 1)]
}

/// Human name for a priority-class value.
pub fn class_name(c: u32) -> String {
    match c {
        0x40 => "IDLE".into(),
        0x4000 => "BELOW_NORMAL".into(),
        0x20 => "NORMAL".into(),
        0x8000 => "ABOVE_NORMAL".into(),
        0x80 => "HIGH".into(),
        0x100 => "REALTIME".into(),
        other => format!("0x{other:x}"),
    }
}

// ------------------------------------------------------- wide / cmdline ----

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Quote one argument per the standard Windows command-line rules.
fn quote_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg.chars().any(|c| c == ' ' || c == '\t' || c == '"');
    if !needs_quotes {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                for _ in 0..backslashes * 2 + 1 {
                    out.push('\\');
                }
                backslashes = 0;
                out.push('"');
            }
            _ => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(c);
            }
        }
    }
    for _ in 0..backslashes * 2 {
        out.push('\\');
    }
    out.push('"');
    out
}

/// Full command line for `args` (argv[0] is the program itself).
pub fn command_line(args: &[&str]) -> String {
    args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------- jobs -----

/// Job object with only KILL_ON_JOB_CLOSE set (extended limit struct, mirrors
/// src/limits.rs). Err = Win32 error code.
pub fn create_job_kill_on_close() -> Result<HANDLE, u32> {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            let gle = GetLastError();
            return Err(gle);
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            let gle = GetLastError();
            CloseHandle(job);
            return Err(gle);
        }
        Ok(job)
    }
}

/// HARD_CAP cpu-rate info: `percent×100` of the machine.
pub fn cpu_rate_hard_cap(rate_x100: u32) -> JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
    let mut info: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
    info.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
    info.Anonymous = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate: rate_x100 };
    info
}

/// MIN_MAX_RATE cpu-rate info: rates are percent×100, nested WORD pair.
pub fn cpu_rate_min_max(min_x100: u32, max_x100: u32) -> JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
    let mut info: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
    info.ControlFlags =
        JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE;
    info.Anonymous = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
        Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0_0 {
            MinRate: min_x100 as u16,
            MaxRate: max_x100 as u16,
        },
    };
    info
}

/// SetInformationJobObject(JobObjectCpuRateControlInformation). Needs no
/// privilege; Err = Win32 error code.
pub fn set_job_cpu_rate(
    job: HANDLE,
    info: &JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
) -> Result<(), u32> {
    unsafe {
        if SetInformationJobObject(
            job,
            JobObjectCpuRateControlInformation,
            (info as *const JOBOBJECT_CPU_RATE_CONTROL_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
        ) == 0
        {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

/// JOB priority-class limit (JOB_OBJECT_LIMIT_PRIORITY_CLASS via the extended
/// limit struct). Fails with 1314 non-elevated for above-normal classes.
pub fn set_job_priority_class(job: HANDLE, class: u32) -> Result<(), u32> {
    unsafe {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PRIORITY_CLASS;
        info.BasicLimitInformation.PriorityClass = class;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

pub fn terminate_job(job: HANDLE) -> Result<(), u32> {
    unsafe {
        if TerminateJobObject(job, 1) == 0 {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

// ------------------------------------------------------------ processes ----

pub struct ProcHandle {
    pub process: HANDLE,
    pub thread: HANDLE,
    pub pid: u32,
}

impl Drop for ProcHandle {
    fn drop(&mut self) {
        // SAFETY: both handles come straight from CreateProcessW and are
        // closed exactly once here.
        unsafe {
            CloseHandle(self.thread);
            CloseHandle(self.process);
        }
    }
}

/// CreateProcessW with arbitrary creation flags. `redirect` = (stdout,
/// stderr) inheritable write ends; when set, STARTF_USESTDHANDLES applies and
/// handles are inherited. Err = Win32 error code.
pub fn create_child(
    exe: &str,
    args: &[&str],
    creation_flags: u32,
    inherit_handles: bool,
    redirect: Option<(HANDLE, HANDLE)>,
) -> Result<ProcHandle, u32> {
    let exe_w = wide(exe);
    // FIX(run): argv[0] must be the program path itself. Children parse
    // std::env::args().skip(1), so without the exe as the first token the
    // subcommand (args[0], e.g. "burner"/"leafprio") is swallowed and the
    // first flag is misread as the subcommand.
    let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 1);
    full_args.push(exe);
    full_args.extend_from_slice(args);
    let mut cmdline_w = wide(&command_line(&full_args));
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    if let Some((so, se)) = redirect {
        si.dwFlags = 256; // STARTF_USESTDHANDLES
        si.hStdOutput = so;
        si.hStdError = se;
    }
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    unsafe {
        // SAFETY: all buffers are nul-terminated/owned for the duration of the
        // call; pi is written in place by CreateProcessW.
        if CreateProcessW(
            exe_w.as_ptr(),
            cmdline_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            inherit_handles as i32,
            creation_flags,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        ) == 0
        {
            let gle = GetLastError();
            return Err(gle);
        }
    }
    Ok(ProcHandle { process: pi.hProcess, thread: pi.hThread, pid: pi.dwProcessId })
}

/// Assign an existing ProcHandle to a job. On failure the child is killed and
/// its handles closed; Err = Win32 error code.
pub fn assign_to_job(p: ProcHandle, job: HANDLE) -> Result<ProcHandle, u32> {
    unsafe {
        if AssignProcessToJobObject(job, p.process) == 0 {
            let gle = GetLastError();
            TerminateProcess(p.process, 1);
            drop(p);
            return Err(gle);
        }
    }
    Ok(p)
}

/// Spec'd entry point: suspended child, assigned to the job. Resume afterwards
/// via resume_proc. Priority class rides in extra_creation_flags when needed.
pub fn spawn_suspended_into_job(
    job: HANDLE,
    exe: &str,
    args: &[&str],
    extra_creation_flags: u32,
) -> Result<ProcHandle, u32> {
    let p = create_child(exe, args, 0x4 /* CREATE_SUSPENDED */ | extra_creation_flags, false, None)?;
    assign_to_job(p, job)
}

/// Like spawn_suspended_into_job but with stdout/stderr redirected to
/// inheritable write ends (used by wnrun to capture wnice output).
pub fn spawn_suspended_full(
    job: HANDLE,
    exe: &str,
    args: &[&str],
    extra_creation_flags: u32,
    redirect: Option<(HANDLE, HANDLE)>,
) -> Result<ProcHandle, u32> {
    let p = create_child(
        exe,
        args,
        0x4 /* CREATE_SUSPENDED */ | extra_creation_flags,
        redirect.is_some(),
        redirect,
    )?;
    assign_to_job(p, job)
}

pub fn resume_proc(p: &ProcHandle) -> Result<(), u32> {
    unsafe {
        let r = ResumeThread(p.thread);
        if r == u32::MAX {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

/// WaitForMultipleObjects over all handles in chunks of <= 64.
pub fn wait_all(handles: &[HANDLE], timeout_ms: u32) -> Result<(), u32> {
    unsafe {
        for chunk in handles.chunks(MAXIMUM_WAIT_OBJECTS) {
            let r = WaitForMultipleObjects(chunk.len() as u32, chunk.as_ptr(), 1, timeout_ms);
            if r == 0xFFFF_FFFF /* WAIT_FAILED */ {
                return Err(GetLastError());
            }
        }
    }
    Ok(())
}

/// TerminateProcess on any child still showing STILL_ACTIVE. Returns the
/// number of processes killed.
pub fn kill_stragglers(procs: &[ProcHandle]) -> usize {
    let mut killed = 0usize;
    unsafe {
        for p in procs {
            let mut code = 0u32;
            if GetExitCodeProcess(p.process, &mut code) != 0 && code == STILL_ACTIVE_U32 {
                if TerminateProcess(p.process, 1) != 0 {
                    killed += 1;
                }
            }
        }
    }
    killed
}

/// OpenProcess(PROCESS_TERMINATE) + TerminateProcess by pid (fallback for
/// stragglers). Returns false when the open or the terminate failed.
pub fn kill_pid(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

/// HANDLE wrapper that may cross a thread boundary: the raw Win32 handle has
/// no thread affinity and is only ever used inside FFI calls.
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

// --------------------------------------------------------------- times -----

fn ft_to_secs(ft: &windows_sys::Win32::Foundation::FILETIME) -> f64 {
    let q = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    q as f64 / 10_000_000.0
}

/// user+kernel CPU seconds of `handle`. Err = Win32 error code.
pub fn cpu_seconds(handle: HANDLE) -> Result<f64, u32> {
    unsafe {
        let mut creation = windows_sys::Win32::Foundation::FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = windows_sys::Win32::Foundation::FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel = windows_sys::Win32::Foundation::FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut user = windows_sys::Win32::Foundation::FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
            let gle = GetLastError();
            return Err(gle);
        }
        Ok(ft_to_secs(&kernel) + ft_to_secs(&user))
    }
}

// -------------------------------------------------------------- timing -----

pub fn qpc() -> u64 {
    unsafe {
        let mut v: i64 = 0;
        // SAFETY: plain pointer to a local i64; the call cannot fail on any
        // supported Windows version.
        QueryPerformanceCounter(&mut v);
        v as u64
    }
}

pub fn qpc_freq() -> u64 {
    unsafe {
        let mut v: i64 = 0;
        // SAFETY: plain pointer to a local i64; the call cannot fail on any
        // supported Windows version.
        QueryPerformanceFrequency(&mut v);
        v as u64
    }
}

/// Milliseconds between two QPC stamps.
pub fn ms_between(a: u64, b: u64) -> f64 {
    let f = qpc_freq() as f64;
    if f <= 0.0 {
        return 0.0;
    }
    (b as f64 - a as f64) * 1000.0 / f
}

// ------------------------------------------------------------ elevation ----

/// True when this process runs with an elevated token (mirrors
/// src/front/native.rs::is_elevated).
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Logical processors from GetNativeSystemInfo (returns void: no failure mode
/// to check).
pub fn logical_cpus() -> u32 {
    unsafe {
        let mut si: SYSTEM_INFO = std::mem::zeroed();
        GetNativeSystemInfo(&mut si);
        si.dwNumberOfProcessors
    }
}

// -------------------------------------------------------------- booster ----

/// One booster attempt on a NEW_PROCESS notification. `msg` is the raw
/// lpNumberOfBytesTransferred (message id per current MS Learn docs); `pid`
/// is read from lpOverlapped (per the same docs: "the value of lpOverlapped
/// is the identifier of the process added to the job").
#[derive(Clone)]
pub struct BoostRecord {
    pub arrival_qpc: u64,
    pub msg: u32,
    pub pid: u32,
    pub key: usize,
    pub open_ok: bool,
    pub set_ok: bool,
    pub gle: u32,
}

/// Completion-port booster: a thread that drains a job's notification port and
/// lifts every new process to HIGH_PRIORITY_CLASS.
pub struct Booster {
    port: HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
    records: Arc<Mutex<Vec<BoostRecord>>>,
    counts: Arc<Mutex<Vec<(u32, u64)>>>,
}

/// Private completion key for the stop sentinel (job messages always carry
/// the registered key 1).
pub const BOOSTER_STOP_KEY: usize = usize::MAX;
/// The caller-defined key registered with the job (exposed so reports can
/// verify which packets came from the job).
pub const JOB_PORT_KEY: usize = 1;

impl Booster {
    /// Associate a fresh completion port with `job` and start the pump thread.
    /// Err = Win32 error code from port creation or association.
    pub fn start(job: HANDLE) -> Result<Booster, u32> {
        let port = unsafe {
            // SAFETY: INVALID_HANDLE_VALUE + null = create a new port with one
            // concurrent thread; null check below covers failure.
            let h = CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1);
            if h.is_null() {
                let gle = GetLastError();
                return Err(gle);
            }
            h
        };
        unsafe {
            let assoc = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
                CompletionKey: JOB_PORT_KEY as *mut c_void,
                CompletionPort: port,
            };
            if SetInformationJobObject(
                job,
                JobObjectAssociateCompletionPortInformation,
                (&assoc as *const JOBOBJECT_ASSOCIATE_COMPLETION_PORT).cast(),
                std::mem::size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
            ) == 0
            {
                let gle = GetLastError();
                CloseHandle(port);
                return Err(gle);
            }
        }
        let records = Arc::new(Mutex::new(Vec::new()));
        let counts = Arc::new(Mutex::new(Vec::new()));
        let r2 = Arc::clone(&records);
        let c2 = Arc::clone(&counts);
        let port_wrapper = SendHandle(port);
        let port_copy = port_wrapper.0; // HANDLE is a raw pointer: Copy
        let thread = std::thread::spawn(move || boost_loop(port_wrapper, r2, c2));
        Ok(Booster { port: port_copy, thread: Some(thread), records, counts })
    }

    /// Snapshot of boost attempts so far.
    pub fn records(&self) -> Vec<BoostRecord> {
        match self.records.lock() {
            Ok(g) => g.clone(),
            Err(_) => Vec::new(),
        }
    }

    /// Message counts as (message_id, count) pairs (id 0xFFFF_FFFF marks
    /// GetQueuedCompletionStatus failures).
    pub fn counts(&self) -> Vec<(u32, u64)> {
        match self.counts.lock() {
            Ok(g) => g.clone(),
            Err(_) => Vec::new(),
        }
    }

    /// Post a stop sentinel, join the pump thread, close the port.
    pub fn stop(&mut self) {
        unsafe {
            // SAFETY: port is our own live handle; null OVERLAPPED = no
            // ownership transfer. If the post fails the pump still exits when
            // the port is closed below (its failure branch).
            if windows_sys::Win32::System::IO::PostQueuedCompletionStatus(
                self.port,
                0,
                BOOSTER_STOP_KEY,
                std::ptr::null(),
            ) == 0
            {
                let _ = GetLastError();
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            CloseHandle(self.port);
        }
    }
}

fn bump(counts: &Mutex<Vec<(u32, u64)>>, msg: u32) {
    if let Ok(mut v) = counts.lock() {
        if let Some(e) = v.iter_mut().find(|(m, _)| *m == msg) {
            e.1 += 1;
        } else {
            v.push((msg, 1));
        }
    }
}

/// The pump thread: drain packets forever until the stop sentinel or a port
/// failure.
fn boost_loop(
    port: SendHandle,
    records: Arc<Mutex<Vec<BoostRecord>>>,
    counts: Arc<Mutex<Vec<(u32, u64)>>>,
) {
    let port = port.0;
    loop {
        let mut bytes = 0u32;
        let mut key = 0usize;
        let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        let ok = unsafe {
            // SAFETY: all out-params are local; the port is owned by this
            // thread until Booster::stop closes it after joining.
            GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut overlapped, INFINITE)
        };
        if ok == 0 {
            let _gle = unsafe { GetLastError() };
            bump(&counts, 0xFFFF_FFFF);
            break;
        }
        if key == BOOSTER_STOP_KEY {
            break;
        }
        bump(&counts, bytes);
        if bytes == JOB_OBJECT_MSG_NEW_PROCESS {
            let arrival = qpc();
            let pid = overlapped as usize as u32;
            let (open_ok, set_ok, gle) = boost_pid(pid);
            if let Ok(mut v) = records.lock() {
                v.push(BoostRecord {
                    arrival_qpc: arrival,
                    msg: bytes,
                    pid,
                    key,
                    open_ok,
                    set_ok,
                    gle,
                });
            }
        }
    }
}

/// THE BOOSTER CALL: open by pid, SetPriorityClass(HIGH). Returns
/// (open_ok, set_ok, gle); gle is the open error when open fails, the set
/// error when set fails, 0 on success.
fn boost_pid(pid: u32) -> (bool, bool, u32) {
    unsafe {
        let h = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION,
            0,
            pid,
        );
        if h.is_null() {
            let gle = GetLastError();
            return (false, false, gle);
        }
        let ok = SetPriorityClass(h, HIGH_PRIORITY_CLASS);
        if ok == 0 {
            let gle = GetLastError();
            CloseHandle(h);
            return (true, false, gle);
        }
        CloseHandle(h);
        (true, true, 0)
    }
}

// ----------------------------------------------------------- self probes ---

/// SetPriorityClass on the current process; Result<(), u32>.
pub fn set_self_priority(class: u32) -> Result<(), u32> {
    unsafe {
        if SetPriorityClass(GetCurrentProcess(), class) == 0 {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

/// GetPriorityClass of the current process (0 on failure).
pub fn self_priority_class() -> u32 {
    unsafe { GetPriorityClass(GetCurrentProcess()) }
}

/// SetThreadPriority(THREAD_PRIORITY_HIGHEST) on the current thread;
/// Result<(), u32>.
pub fn set_self_thread_high() -> Result<(), u32> {
    unsafe {
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) == 0 {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

/// GetThreadPriority of the current thread (i32; error cannot be
/// distinguished from a value here, so only used for verification output).
pub fn self_thread_priority() -> i32 {
    unsafe { GetThreadPriority(GetCurrentThread()) }
}

pub fn restore_self_thread_normal() -> Result<(), u32> {
    unsafe {
        if SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_NORMAL) == 0 {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

/// Busy-poll GetPriorityClass(self) until HIGH or timeout. Returns
/// (saw_high, latency_ms, t_high_qpc) — latency 0.0 / t_high 0 when never
/// seen.
pub fn poll_until_high(t0: u64, timeout_ms: f64) -> (bool, f64, u64) {
    let mut iters: u64 = 0;
    loop {
        if self_priority_class() == HIGH_PRIORITY_CLASS {
            let t_high = qpc();
            return (true, ms_between(t0, t_high), t_high);
        }
        if iters % 1024 == 0 && ms_between(t0, qpc()) >= timeout_ms {
            return (false, 0.0, 0);
        }
        std::hint::spin_loop();
        iters += 1;
    }
}

pub const NORMAL_CLASS: u32 = NORMAL_PRIORITY_CLASS;
pub const HIGH_CLASS: u32 = HIGH_PRIORITY_CLASS;
