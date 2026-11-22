//! The real (Win32-backed) inspection pass behind `wnice --jobs`.

use super::win::{self, HandleTable};
use super::{Inspector, JobInfo, ProcInfo, Scan, SAMPLE_WINDOW, TOP_PROCESSES};
use crate::Handle;

/// Multicall frontend names (image stems) that hold a wrapped command's job.
const WRAPPER_NAMES: [&str; 15] = [
    "wnice",
    "idle",
    "belownormal",
    "abovenormal",
    "high",
    "realtime",
    "capc",
    "capt",
    "capm",
    "caps",
    "capn",
    "cy",
    "cx",
    "admin",
    "uiup",
];

/// True when `image` (a bare file name such as `"wnice.exe"`) is one of the
/// wnice-family multicall frontends: the stem is lowercased and `.exe` stripped.
pub fn is_wrapper_image(image: &str) -> bool {
    let lower = image.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    WRAPPER_NAMES.contains(&stem)
}

/// A duplicated job handle kept alive together with its row, so the
/// membership pass does not duplicate it again.
struct OwnedJob {
    handle: Handle,
    handle_value: usize,
    info: JobInfo,
}

/// The real inspector: no state, one snapshot per [`Inspector::scan`] call.
pub struct RealInspector;

impl Inspector for RealInspector {
    fn scan(&self) -> Scan {
        let logical_cpus = win::logical_cpus();
        let window_ms = SAMPLE_WINDOW.as_millis() as u64;
        let me = std::process::id();

        // The handle table is the only way to reach another process's job
        // handles; the job type index is resolved at runtime from a probe job.
        let table = HandleTable::query();
        let job_type = table.as_ref().and_then(win::job_type_index);

        let processes = crate::front::native::processes();
        // Indices of the recognised wrapper owners, so the list is borrowed
        // once here and again by the sampling pass below.
        let owners: Vec<usize> = processes
            .iter()
            .enumerate()
            .filter(|(_, (image, pid))| *pid != me && is_wrapper_image(image))
            .map(|(i, _)| i)
            .collect();
        let mut uninspected_owners = 0usize;
        let mut owned: Vec<OwnedJob> = Vec::new();

        match (table.as_ref(), job_type) {
            (Some(table), Some(job_type)) => {
                for &i in &owners {
                    let (image, pid) = &processes[i];
                    let Some(owner) = win::open_owner(*pid) else {
                        uninspected_owners += 1;
                        continue;
                    };
                    let command = win::command_line(&owner);
                    let entries = table.entries_of(*pid);
                    if entries.is_empty() {
                        continue;
                    }
                    let mut failed = false;
                    for entry in entries.iter().filter(|e| e.object_type_index == job_type) {
                        let Some(handle) = win::duplicate_job(&owner, entry.handle_value) else {
                            failed = true;
                            continue;
                        };
                        let Some(limits) = win::job_limits(&handle) else {
                            failed = true;
                            continue;
                        };
                        owned.push(OwnedJob {
                            handle,
                            handle_value: entry.handle_value,
                            info: JobInfo {
                                id: 0,
                                owner_pid: *pid,
                                owner_image: image.clone(),
                                owner_command: command.clone(),
                                cpu: limits.cpu,
                                job_memory: limits.job_memory,
                                active_process_limit: limits.active_process_limit,
                                affinity: limits.affinity,
                                priority: limits.priority,
                                active_processes: limits.active_processes,
                            },
                        });
                    }
                    if failed {
                        uninspected_owners += 1;
                    }
                }
            }
            // No handle snapshot or no job type index means no owner could be
            // examined, so the empty job list is "not inspected", not "no jobs".
            _ => uninspected_owners = owners.len(),
        }

        // Deterministic display order: owner, then the owner's handle value.
        owned.sort_by_key(|j| (j.info.owner_pid, j.handle_value));
        for (i, job) in owned.iter_mut().enumerate() {
            job.info.id = i + 1;
        }
        let jobs: Vec<JobInfo> = owned.iter().map(|j| j.info.clone()).collect();

        // Two CPU-time samples around the window; the delta is the CPU each
        // process burned during it.
        let mut first: Vec<(u32, String, Handle, u64)> = Vec::new();
        for (image, pid) in &processes {
            if *pid == me {
                continue;
            }
            let Some(handle) = win::open_query(*pid) else {
                continue;
            };
            let Some(start) = win::cpu_time_100ns(&handle) else {
                continue;
            };
            first.push((*pid, image.clone(), handle, start));
        }
        std::thread::sleep(SAMPLE_WINDOW);
        let mut samples: Vec<(u32, String, f64)> = Vec::new();
        for (pid, image, handle, start) in first {
            let Some(end) = win::cpu_time_100ns(&handle) else {
                continue;
            };
            let delta = end.saturating_sub(start) as f64 * 1e-7;
            if delta > 0.0 {
                samples.push((pid, image, delta));
            }
        }
        // Busiest first, pid ascending to break ties.
        samples.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));

        let mut busy: Vec<ProcInfo> = Vec::new();
        for (pid, image, cpu_seconds) in samples.into_iter().take(TOP_PROCESSES) {
            let mut ids: Vec<usize> = Vec::new();
            if let Some(proc) = win::open_query(pid) {
                for job in &owned {
                    if win::is_in_job(&proc, &job.handle) {
                        ids.push(job.info.id);
                    }
                }
            }
            ids.sort_unstable();
            busy.push(ProcInfo {
                pid,
                image,
                cpu_seconds,
                jobs: ids,
            });
        }

        Scan {
            jobs,
            processes: busy,
            uninspected_owners,
            logical_cpus,
            window_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_images_are_the_multicall_names() {
        assert!(is_wrapper_image("wnice.exe"));
        assert!(is_wrapper_image("CAPC.EXE"));
        assert!(is_wrapper_image("capm"));
        assert!(is_wrapper_image("Admin.exe"));
        assert!(is_wrapper_image("uiup.exe"));
        assert!(!is_wrapper_image("node.exe"));
        assert!(!is_wrapper_image("wnice.cmd"));
        assert!(!is_wrapper_image("wnice2.exe"));
        assert!(!is_wrapper_image(""));
    }
}
