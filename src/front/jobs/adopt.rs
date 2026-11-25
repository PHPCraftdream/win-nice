use super::adopt_select::{attempt, select, AttemptResult, Candidate};
use crate::front::jobs::win;
use crate::Handle;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, GetLastError, DUPLICATE_SAME_ACCESS, ERROR_NO_MORE_FILES,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, IsProcessInJob};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE,
};

#[derive(Clone)]
struct Row {
    name: String,
    pid: u32,
    parent: u32,
}

struct Snapshot(windows_sys::Win32::Foundation::HANDLE);

impl Drop for Snapshot {
    fn drop(&mut self) {
        // SAFETY: This owns a valid Toolhelp snapshot handle until it is dropped.
        unsafe { CloseHandle(self.0) };
    }
}

fn collect() -> Option<Vec<Row>> {
    // SAFETY: Toolhelp snapshot and entry are used only through their documented APIs.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() || snapshot as isize == -1 {
            return None;
        }
        // SAFETY: The successful snapshot handle is uniquely owned by this guard and closed on every return.
        let snapshot = Snapshot(snapshot);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut rows = Vec::new();
        let mut first = true;
        loop {
            let ok = if first {
                first = false;
                Process32FirstW(snapshot.0, &mut entry)
            } else {
                Process32NextW(snapshot.0, &mut entry)
            };
            if ok == 0 {
                // Capture immediately: subsequent calls, including Drop, may overwrite this error.
                let error = GetLastError();
                if error == ERROR_NO_MORE_FILES {
                    return Some(rows);
                }
                return None;
            }
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            rows.push(Row {
                name: String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase(),
                pid: entry.th32ProcessID,
                parent: entry.th32ParentProcessID,
            });
        }
    }
}

fn ancestry(rows: &[Row], own: u32) -> HashSet<u32> {
    let parents: std::collections::HashMap<u32, u32> =
        rows.iter().map(|r| (r.pid, r.parent)).collect();
    let mut out = HashSet::new();
    let mut pid = own;
    while let Some(parent) = parents.get(&pid).copied() {
        if parent == 0 || parent == pid || !out.insert(parent) {
            break;
        }
        pid = parent;
    }
    out
}

/// Whether `pid` is provably a process of the current user (query-only open).
fn same_user_pid(pid: u32, own: &win::OwnedSid) -> bool {
    win::open_query(pid).is_some_and(|process| win::same_user(&process, own) == Some(true))
}

pub struct Poller {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    pub fn start(job: &Handle, root_pid: u32, names: Vec<String>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let job_raw = job.raw() as usize;
        let thread = std::thread::spawn(move || {
            // SAFETY: duplicate a live job handle into this thread's process; output is owned here.
            let mut local_job = std::ptr::null_mut();
            if unsafe {
                DuplicateHandle(
                    windows_sys::Win32::System::Threading::GetCurrentProcess(),
                    job_raw as windows_sys::Win32::Foundation::HANDLE,
                    windows_sys::Win32::System::Threading::GetCurrentProcess(),
                    &mut local_job,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            } == 0
            {
                return;
            }
            let job = Handle::from_raw(local_job);
            // SAFETY: this API returns the calling thread's process identifier.
            let own = unsafe { GetCurrentProcessId() };
            let Some(own_sid) = win::current_user() else {
                eprintln!("wnice: adoption unavailable: current user SID query failed");
                return;
            };
            let mut attempted = HashSet::new();
            let mut decision_attempted = HashSet::new();
            while !thread_stop.load(Ordering::Relaxed) {
                let Some(rows) = collect() else {
                    for _ in 0..10 {
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    continue;
                };
                if !rows.iter().any(|row| row.pid == own) {
                    for _ in 0..10 {
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    continue;
                }
                let excluded = ancestry(&rows, own);
                for row in select(
                    rows.into_iter().map(|r| Candidate {
                        name: r.name,
                        pid: r.pid,
                    }),
                    own,
                    root_pid,
                    &excluded,
                    &names,
                    &mut attempted,
                ) {
                    // SAFETY: OpenProcess returns an owned process handle or null.
                    let process = unsafe {
                        OpenProcess(
                            PROCESS_QUERY_LIMITED_INFORMATION
                                | PROCESS_SET_QUOTA
                                | PROCESS_TERMINATE,
                            0,
                            row.pid,
                        )
                    };
                    if process.is_null() {
                        // SAFETY: GetLastError reads the immediately preceding OpenProcess failure.
                        let code = unsafe { GetLastError() };
                        // Only a refusal for a process of this user is worth a
                        // line: other users' processes are never candidates.
                        if same_user_pid(row.pid, &own_sid) {
                            eprintln!(
                                "wnice: could not adopt {} (pid {}): OpenProcess failed: {}",
                                row.name,
                                row.pid,
                                std::io::Error::from_raw_os_error(code as i32)
                            );
                        }
                        continue;
                    }
                    let process = Handle::from_raw(process);
                    let query_error = std::cell::Cell::new(None);
                    let mut assignment_error = None;
                    let result = attempt(
                        row.pid,
                        &mut decision_attempted,
                        || {
                            let result = win::same_user(&process, &own_sid);
                            if result.is_none() {
                                // SAFETY: capture the error from the failed same-user query immediately.
                                query_error.set(Some(unsafe { GetLastError() }));
                            }
                            result
                        },
                        || {
                            let mut in_job = 0;
                            // SAFETY: query-only process and owned job handles remain live.
                            if unsafe { IsProcessInJob(process.raw(), job.raw(), &mut in_job) } == 0
                            {
                                // SAFETY: capture the error from the failed membership query immediately.
                                query_error.set(Some(unsafe { GetLastError() }));
                                None
                            } else {
                                Some(in_job != 0)
                            }
                        },
                        || {
                            // SAFETY: both handles remain live; process is explicitly opened.
                            if unsafe { AssignProcessToJobObject(job.raw(), process.raw()) } != 0 {
                                true
                            } else {
                                // SAFETY: GetLastError reads the immediately preceding assignment failure.
                                assignment_error = Some(unsafe { GetLastError() });
                                false
                            }
                        },
                    );
                    match result {
                        AttemptResult::Assigned => {
                            eprintln!("wnice: adopted {} (pid {})", row.name, row.pid);
                        }
                        AttemptResult::AssignmentFailed => {
                            let code = assignment_error.unwrap_or_default();
                            let detail =
                                if code == windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED {
                                    format!(
                                        "ERROR_ACCESS_DENIED ({})",
                                        std::io::Error::from_raw_os_error(code as i32)
                                    )
                                } else {
                                    std::io::Error::from_raw_os_error(code as i32).to_string()
                                };
                            eprintln!(
                                "wnice: could not adopt {} (pid {}): {}",
                                row.name, row.pid, detail
                            );
                        }
                        AttemptResult::QueryFailed => {
                            let code = query_error.get().unwrap_or_default();
                            eprintln!(
                                "wnice: could not adopt {} (pid {}): query failed: {}",
                                row.name,
                                row.pid,
                                std::io::Error::from_raw_os_error(code as i32)
                            );
                        }
                        _ => {}
                    }
                }
                for _ in 0..10 {
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_user_pid_accepts_this_process_and_refuses_the_system_process() {
        let own = win::current_user().expect("current user SID");
        assert!(same_user_pid(std::process::id(), &own));
        // PID 4 (System) belongs to another account, or cannot be opened at all.
        assert!(!same_user_pid(4, &own));
        assert!(!same_user_pid(0, &own));
    }
}
