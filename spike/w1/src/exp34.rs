//! exp34: WSL process probe (read-only) and the wnice-tree check.

use std::collections::{BTreeMap, BTreeSet};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_WRITE_DATA, CREATE_ALWAYS,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetPriorityClass, OpenProcess, WaitForSingleObject,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_VM_READ,
};

use crate::common::{
    arg_val, class_name, create_job_kill_on_close, kv, ms_between, out_sub, qpc, resume_proc,
    self_exe, spawn_suspended_full, terminate_job, write_kv_file, Booster,
};

// ------------------------------------------------------ process listing ----

/// (name, pid, parent pid) of every running process.
pub fn processes_with_parents() -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push((
                String::from_utf16_lossy(&entry.szExeFile[..len]),
                entry.th32ProcessID,
                entry.th32ParentProcessID,
            ));
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    out
}

// ----------------------------------------------------------- wslprobe ------

/// One strictly read-only OpenProcess probe; prints ok/gle and closes the
/// handle immediately.
fn probe(pid: u32, access: u32, label: &str) {
    unsafe {
        let h = OpenProcess(access, 0, pid);
        if h.is_null() {
            let gle = GetLastError();
            kv(label, format!("error gle={gle}"));
        } else {
            kv(label, "ok");
            CloseHandle(h);
        }
    }
}

fn wslprobe() -> i32 {
    let procs = processes_with_parents();
    kv("wslprobe_processes_seen", procs.len());

    // (a) QUERY_LIMITED | VM_READ, (b) QUERY_LIMITED, (c) SET_INFORMATION | SET_QUOTA.
    const ACC_A: u32 = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ;
    const ACC_B: u32 = PROCESS_QUERY_LIMITED_INFORMATION;
    const ACC_C: u32 = PROCESS_SET_INFORMATION | PROCESS_SET_QUOTA;

    let mut vmmem_seen = 0usize;
    let mut aux_seen = 0usize;
    for (name, pid, _ppid) in &procs {
        let lower = name.to_lowercase();
        if lower.contains("vmmem") {
            vmmem_seen += 1;
            kv("vmmem", format!("name={name} pid={pid}"));
            probe(*pid, ACC_A, &format!("vmmem_{pid}_probe_a_query_vmread"));
            probe(*pid, ACC_B, &format!("vmmem_{pid}_probe_b_query_limited"));
            probe(*pid, ACC_C, &format!("vmmem_{pid}_probe_c_setinfo_setquota"));
        } else if lower == "wslservice.exe" || lower == "vmwp.exe" {
            aux_seen += 1;
            kv("wsl_aux", format!("name={name} pid={pid}"));
            // Probes (b) and (c) only, per spec.
            probe(*pid, ACC_B, &format!("aux_{pid}_probe_b_query_limited"));
            probe(*pid, ACC_C, &format!("aux_{pid}_probe_c_setinfo_setquota"));
        }
    }
    kv("wslprobe", format!("vmmem_like={vmmem_seen} wslservice_or_vmwp={aux_seen}"));
    kv("wslprobe_done", "read_only_probes_only_never_setpriority_terminate_assign");
    0
}

// ------------------------------------------------------------- wnrun -------

fn create_capture_file(path: &std::path::Path) -> Result<HANDLE, u32> {
    let wide = crate::common::wide(&path.to_string_lossy());
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    unsafe {
        // SAFETY: wide is nul-terminated and outlives the call; sa is a plain
        // struct. CreateFileW signals failure with INVALID_HANDLE_VALUE.
        let h = CreateFileW(
            wide.as_ptr(),
            FILE_WRITE_DATA,
            FILE_SHARE_READ,
            &sa,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            let gle = GetLastError();
            return Err(gle);
        }
        Ok(h)
    }
}

fn read_text_file(path: &std::path::Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => format!("<read failed: {e}>"),
    }
}

/// True when `pid`'s ancestry chain (pid -> parent -> ...) contains `root`.
fn in_tree(parent_map: &BTreeMap<u32, u32>, pid: u32, root: u32) -> bool {
    let mut cur = pid;
    for _ in 0..4096 {
        if cur == root {
            return true;
        }
        match parent_map.get(&cur) {
            Some(&p) if p != cur => cur = p,
            _ => return false,
        }
    }
    false
}

fn wnrun(args: &[String]) -> i32 {
    let wnice = match arg_val(args, "--wnice") {
        Some(w) => w,
        None => {
            kv("wnrun_error", "missing --wnice <abs-path-to-wnice.exe>");
            return 2;
        }
    };
    let variant = arg_val(args, "--variant").unwrap_or_else(|| "A".into());
    if variant != "A" && variant != "B" {
        kv("wnrun_error", format!("bad --variant {variant} (want A|B)"));
        return 2;
    }
    if !std::path::Path::new(&wnice).is_absolute() {
        kv("wnrun_error", "--wnice must be an absolute path");
        return 2;
    }

    let dir = out_sub(&format!("wn/{variant}"));
    let leaf_path = dir.join("leaf.txt");
    let leaf_s = leaf_path.to_string_lossy().into_owned();
    let wn_out_path = dir.join("wnice-out.txt");
    let wn_err_path = dir.join("wnice-err.txt");

    let out_h = match create_capture_file(&wn_out_path) {
        Ok(h) => h,
        Err(gle) => {
            kv("wnrun_error", format!("create wnice-out capture gle={gle}"));
            return 1;
        }
    };
    let err_h = match create_capture_file(&wn_err_path) {
        Ok(h) => h,
        Err(gle) => {
            kv("wnrun_error", format!("create wnice-err capture gle={gle}"));
            unsafe {
                CloseHandle(out_h);
            }
            return 1;
        }
    };

    // Build the wnice command: variant B adds `-p high` before -c 10.
    let self_path = self_exe();
    let leaf_args_tail: [&str; 6] = [
        &self_path,
        "leafprio",
        "--out",
        &leaf_s,
        "--label",
        &variant,
    ];
    let mut wn_args: Vec<&str> = Vec::new();
    if variant == "B" {
        wn_args.push("-p");
        wn_args.push("high");
    }
    wn_args.extend_from_slice(&["-c", "10", "--", "cmd", "/c"]);
    wn_args.extend_from_slice(&leaf_args_tail);

    let job = match create_job_kill_on_close() {
        Ok(j) => j,
        Err(gle) => {
            kv("wnrun_error", format!("create job gle={gle}"));
            unsafe {
                CloseHandle(out_h);
                CloseHandle(err_h);
            }
            return 1;
        }
    };
    let mut booster = match Booster::start(job) {
        Ok(b) => b,
        Err(gle) => {
            kv("wnrun_error", format!("booster start gle={gle}"));
            unsafe {
                CloseHandle(out_h);
                CloseHandle(err_h);
                CloseHandle(job);
            }
            return 1;
        }
    };

    let wn = match spawn_suspended_full(job, &wnice, &wn_args, 0, Some((out_h, err_h))) {
        Ok(p) => p,
        Err(gle) => {
            kv("wnrun_error", format!("spawn wnice gle={gle}"));
            booster.stop();
            unsafe {
                CloseHandle(out_h);
                CloseHandle(err_h);
                CloseHandle(job);
            }
            return 1;
        }
    };
    kv("wnrun", format!("variant={variant} wnice_pid={} cmd={wnice}", wn.pid));
    if let Err(gle) = resume_proc(&wn) {
        kv("wnrun_error", format!("resume gle={gle}"));
    }

    // Sample every 100ms for up to 10s: walk ancestry, watch priority classes
    // of every pid in the wnice tree.
    let root_pid = wn.pid;
    let mut tree: BTreeMap<u32, (String, BTreeSet<u32>)> = BTreeMap::new();
    let start = qpc();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let procs = processes_with_parents();
        let mut parent_map: BTreeMap<u32, u32> = BTreeMap::new();
        for (_n, pid, ppid) in &procs {
            parent_map.insert(*pid, *ppid);
        }
        for (name, pid, _ppid) in &procs {
            if !in_tree(&parent_map, *pid, root_pid) {
                continue;
            }
            let entry = tree.entry(*pid).or_insert_with(|| (name.clone(), BTreeSet::new()));
            entry.0 = name.clone();
            unsafe {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, *pid);
                if !h.is_null() {
                    let c = GetPriorityClass(h);
                    if c != 0 {
                        entry.1.insert(c);
                    }
                    CloseHandle(h);
                }
            }
        }
        let mut code = 0u32;
        let exited =
            unsafe { GetExitCodeProcess(wn.process, &mut code) != 0 && code != STILL_ACTIVE as u32 };
        if exited || ms_between(start, qpc()) >= 10_000.0 {
            break;
        }
    }

    // Wait for wnice itself (<= 10s), then terminate the job if still alive.
    let wr = unsafe { WaitForSingleObject(wn.process, 10_000) };
    if wr == WAIT_TIMEOUT {
        let mut code = 0u32;
        let alive =
            unsafe { GetExitCodeProcess(wn.process, &mut code) != 0 && code == STILL_ACTIVE as u32 };
        if alive {
            kv("wnrun", "wnice_still_alive_after_10s: terminating job");
            if let Err(gle) = terminate_job(job) {
                kv("wnrun_error", format!("terminate job gle={gle}"));
            }
        }
    }
    booster.stop();

    // Booster records: per pid, set result + gle.
    for r in booster.records() {
        kv("wnrun_booster", format!(
            "pid={} msg={} open_ok={} set_ok={} gle={} key==jobkey={}",
            r.pid, r.msg, r.open_ok, r.set_ok, r.gle, r.key == crate::common::JOB_PORT_KEY
        ));
    }
    kv("wnrun_msgs", format!("{:?}", booster.counts()));

    // Per-pid priority classes seen in the tree.
    for (pid, (name, classes)) in &tree {
        let list: Vec<String> = classes.iter().map(|c| format!("{c}({})", class_name(*c))).collect();
        kv("wnrun_tree", format!("name={name} pid={pid} classes={list:?}"));
    }

    // leaf.txt content.
    kv("wnrun_leaf_file", leaf_path.to_string_lossy());
    println!("{}", read_text_file(&leaf_path));

    // wnice's own captured stdout/stderr.
    kv("wnrun_wnice_stdout_file", wn_out_path.to_string_lossy());
    println!("{}", read_text_file(&wn_out_path));
    kv("wnrun_wnice_stderr_file", wn_err_path.to_string_lossy());
    println!("{}", read_text_file(&wn_err_path));

    // Cleanup: the job kill above got the tree; TerminateProcess fallback on
    // our own direct child in case it somehow survived.
    crate::common::kill_pid(wn.pid); // ProcHandle drops right after: closes wnice handles
    unsafe {
        CloseHandle(out_h);
        CloseHandle(err_h);
        CloseHandle(job);
    }
    kv("wnrun_done", 1);
    0
}

// ------------------------------------------------------------ leafprio -----

/// Child probe: class at start, optional latency-to-HIGH; writes a record
/// file and prints it; exit 0.
pub fn leafprio(args: &[String]) -> i32 {
    let t0 = qpc();
    let out = arg_val(args, "--out");
    let label = arg_val(args, "--label").unwrap_or_else(|| "-".into());
    let with_latency = crate::common::has_flag(args, "--latency");
    let pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let cls0 = unsafe { GetPriorityClass(windows_sys::Win32::System::Threading::GetCurrentProcess()) };
    let (saw_high, latency, t_high) = if with_latency {
        crate::common::poll_until_high(t0, 2000.0)
    } else {
        let saw = cls0 == crate::common::HIGH_CLASS;
        (saw, 0.0, if saw { t0 } else { 0 })
    };
    if let Some(path) = out {
        write_kv_file(
            std::path::Path::new(&path),
            &[
                ("label", label.clone()),
                ("pid", pid.to_string()),
                ("t0", t0.to_string()),
                ("class_at_t0", cls0.to_string()),
                ("saw_high", saw_high.to_string()),
                ("latency_ms", format!("{latency:.3}")),
                ("t_high", t_high.to_string()),
            ],
        );
        kv("leafprio_file", path);
    }
    kv("leafprio", format!(
        "label={label} pid={pid} t0={t0} class_at_t0={cls0}({}) saw_high={saw_high} latency_ms={latency:.3}",
        class_name(cls0)
    ));
    0
}

pub fn dispatch(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("wslprobe") => wslprobe(),
        Some("wnrun") => wnrun(&args[1..]),
        Some("leafprio") => leafprio(&args[1..]),
        _ => {
            kv("exp34_error", "unknown subcommand");
            2
        }
    }
}
