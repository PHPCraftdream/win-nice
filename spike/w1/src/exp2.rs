//! exp2: priority-boost spike — selfset probes, latency fan-out, 3-gen chain.

use std::collections::BTreeMap;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetPriorityClass, OpenProcess, SetPriorityClass,
    WaitForMultipleObjects, WaitForSingleObject, ABOVE_NORMAL_PRIORITY_CLASS,
    HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION,
};

use crate::common::{
    arg_val, assign_to_job, class_name, create_child, create_job_kill_on_close, get_kv,
    is_elevated, kill_stragglers, kv, median, ms_between, out_dir, out_sub, percentile,
    poll_until_high, qpc, read_kv_file, resume_proc, self_exe, set_job_priority_class,
    set_self_priority, terminate_job, wait_all, write_kv_file, Booster, ProcHandle,
    JOB_OBJECT_MSG_NEW_PROCESS,
};

// ------------------------------------------------------------- selfset -----

fn wait_and_report(p: &ProcHandle, label: &str, out_file: &std::path::Path) {
    let r = unsafe { WaitForSingleObject(p.process, 15000) };
    if r == WAIT_TIMEOUT {
        kv(&format!("{label}_wait"), "timeout");
        return;
    }
    if r == 0xFFFF_FFFF {
        let gle = unsafe { GetLastError() };
        kv(&format!("{label}_wait"), format!("fail gle={gle}"));
        return;
    }
    let rec = read_kv_file(out_file);
    let pid = get_kv(&rec, "pid").unwrap_or("?");
    let cls: u32 = get_kv(&rec, "class_at_t0").and_then(|v| v.parse().ok()).unwrap_or(0);
    let saw = get_kv(&rec, "saw_high").unwrap_or("?");
    let lat = get_kv(&rec, "latency_ms").unwrap_or("?");
    kv(&format!("{label}_child"), format!(
        "pid={pid} class_at_t0={cls}({}) saw_high={saw} latency_ms={lat}",
        class_name(cls)
    ));
}

fn selfset() -> i32 {
    kv("selfset_pid", unsafe { GetCurrentProcessId() });
    kv("selfset_elevated", is_elevated());

    // p1: self HIGH then restore NORMAL.
    let old = unsafe { GetPriorityClass(GetCurrentProcess()) };
    kv("p1_old_class", format!("{old}({})", class_name(old)));
    match set_self_priority(HIGH_PRIORITY_CLASS) {
        Ok(()) => {
            let now = unsafe { GetPriorityClass(GetCurrentProcess()) };
            kv("p1_set_high", format!("ok class_now={now}({})", class_name(now)));
        }
        Err(gle) => kv("p1_set_high", format!("fail gle={gle}")),
    }
    match set_self_priority(NORMAL_PRIORITY_CLASS) {
        Ok(()) => kv("p1_restore", "ok"),
        Err(gle) => kv("p1_restore", format!("fail gle={gle}")),
    }

    // p2: self ABOVE_NORMAL then restore NORMAL.
    match set_self_priority(ABOVE_NORMAL_PRIORITY_CLASS) {
        Ok(()) => {
            let now = unsafe { GetPriorityClass(GetCurrentProcess()) };
            kv("p2_set_above_normal", format!("ok class_now={now}({})", class_name(now)));
        }
        Err(gle) => kv("p2_set_above_normal", format!("fail gle={gle}")),
    }
    match set_self_priority(NORMAL_PRIORITY_CLASS) {
        Ok(()) => kv("p2_restore", "ok"),
        Err(gle) => kv("p2_restore", format!("fail gle={gle}")),
    }

    let self_path = self_exe();
    let dir = out_dir();

    // p3: child born with the HIGH_PRIORITY_CLASS creation flag (suspended).
    {
        let f = dir.join("selfset-p3.txt");
        let fs = f.to_string_lossy().into_owned();
        match create_child(
            &self_path,
            &["leafprio", "--out", &fs, "--label", "p3"],
            0x4 /* CREATE_SUSPENDED */ | HIGH_PRIORITY_CLASS,
            false,
            None,
        ) {
            Ok(p) => {
                if let Err(gle) = resume_proc(&p) {
                    kv("p3_resume", format!("fail gle={gle}"));
                }
                wait_and_report(&p, "p3", &f);
            }
            Err(gle) => kv("p3_spawn", format!("fail gle={gle}")),
        }
    }

    // p4: child created suspended (no flag), parent sets HIGH before resume.
    {
        let f = dir.join("selfset-p4.txt");
        let fs = f.to_string_lossy().into_owned();
        match create_child(
            &self_path,
            &["leafprio", "--out", &fs, "--label", "p4"],
            0x4, // CREATE_SUSPENDED
            false,
            None,
        ) {
            Ok(p) => {
                match set_child_priority(p.process, HIGH_PRIORITY_CLASS) {
                    Ok(()) => kv("p4_suspended_set", "ok"),
                    Err(gle) => kv("p4_suspended_set", format!("fail gle={gle}")),
                }
                if let Err(gle) = resume_proc(&p) {
                    kv("p4_resume", format!("fail gle={gle}"));
                }
                wait_and_report(&p, "p4", &f);
            }
            Err(gle) => kv("p4_spawn", format!("fail gle={gle}")),
        }
    }

    // p5: THE BOOSTER CALL — running child, parent opens by pid, sets HIGH.
    {
        let f = dir.join("selfset-p5.txt");
        let fs = f.to_string_lossy().into_owned();
        match create_child(
            &self_path,
            &["leafprio", "--latency", "--out", &fs, "--label", "p5"],
            0, // running, not suspended
            false,
            None,
        ) {
            Ok(p) => {
                match open_by_pid(p.pid) {
                    Ok(h) => {
                        kv("p5_open", format!("ok pid={}", p.pid));
                        match set_child_priority(h, HIGH_PRIORITY_CLASS) {
                            Ok(()) => kv("p5_set_high", "ok"),
                            Err(gle) => kv("p5_set_high", format!("fail gle={gle}")),
                        }
                        unsafe {
                            CloseHandle(h);
                        }
                    }
                    Err(gle) => kv("p5_open", format!("fail gle={gle}")),
                }
                wait_and_report(&p, "p5", &f);
            }
            Err(gle) => kv("p5_spawn", format!("fail gle={gle}")),
        }
    }

    // p6: JOB priority-class limit — expect 1314 non-elevated. No process is
    // ever assigned to this job.
    {
        match create_job_kill_on_close() {
            Ok(job) => {
                match set_job_priority_class(job, HIGH_PRIORITY_CLASS) {
                    Ok(()) => kv("p6_job_priority_limit", "ok (unexpected when non-elevated)"),
                    Err(gle) => kv("p6_job_priority_limit", format!(
                        "fail gle={gle}{}",
                        if gle == 1314 { " (ERROR_PRIVILEGE_NOT_HELD — the project premise)" } else { "" }
                    )),
                }
                unsafe {
                    CloseHandle(job);
                }
            }
            Err(gle) => kv("p6_job_create", format!("fail gle={gle}")),
        }
    }

    // p7: thread priority up + verify + restore.
    match crate::common::set_self_thread_high() {
        Ok(()) => {
            let tp = crate::common::self_thread_priority();
            kv("p7_thread_high", format!("ok get={tp} (2 == THREAD_PRIORITY_HIGHEST)"));
        }
        Err(gle) => kv("p7_thread_high", format!("fail gle={gle}")),
    }
    match crate::common::restore_self_thread_normal() {
        Ok(()) => kv("p7_restore", "ok"),
        Err(gle) => kv("p7_restore", format!("fail gle={gle}")),
    }

    kv("selfset_done", 1);
    0
}

/// SetPriorityClass on a process handle. Err = Win32 error code.
fn set_child_priority(h: HANDLE, class: u32) -> Result<(), u32> {
    unsafe {
        if SetPriorityClass(h, class) == 0 {
            Err(GetLastError())
        } else {
            Ok(())
        }
    }
}

/// THE BOOSTER CALL's open half: OpenProcess(QUERY_LIMITED | SET_INFORMATION).
fn open_by_pid(pid: u32) -> Result<HANDLE, u32> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION, 0, pid);
        if h.is_null() {
            Err(GetLastError())
        } else {
            Ok(h)
        }
    }
}

// ----------------------------------------------------------- lat-probe -----

/// Child-side poller: t0 at handler entry, busy-poll for HIGH.
pub fn lat_probe(args: &[String]) -> i32 {
    let t0 = qpc();
    let out = arg_val(args, "--out").unwrap_or_else(|| "out/lat-probe.txt".into());
    let timeout_ms: f64 = arg_val(args, "--timeout-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000.0);
    let pid = unsafe { GetCurrentProcessId() };
    let cls0 = unsafe { GetPriorityClass(GetCurrentProcess()) };
    let (saw_high, latency, t_high) = poll_until_high(t0, timeout_ms);
    write_kv_file(
        std::path::Path::new(&out),
        &[
            ("pid", pid.to_string()),
            ("t0", t0.to_string()),
            ("t_high", t_high.to_string()),
            ("class_at_t0", cls0.to_string()),
            ("saw_high", saw_high.to_string()),
            ("latency_ms", format!("{latency:.3}")),
        ],
    );
    kv("lat_probe", format!(
        "pid={pid} class_at_t0={cls0}({}) saw_high={saw_high} latency_ms={latency:.3}",
        class_name(cls0)
    ));
    0
}

// -------------------------------------------------------- exp2-latency -----

fn read_lat_file(path: &std::path::Path) -> (bool, f64, Option<u32>) {
    let rec = read_kv_file(path);
    let saw = get_kv(&rec, "saw_high").map(|v| v == "true").unwrap_or(false);
    let lat = get_kv(&rec, "latency_ms").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let cls = get_kv(&rec, "class_at_t0").and_then(|v| v.parse().ok());
    (saw, lat, cls)
}

fn exp2_latency(args: &[String]) -> i32 {
    let n: usize = arg_val(args, "--n").and_then(|s| s.parse().ok()).unwrap_or(30);
    let n = n.clamp(1, 300);
    let job = match create_job_kill_on_close() {
        Ok(j) => j,
        Err(gle) => {
            kv("exp2lat_error", format!("create job gle={gle}"));
            return 1;
        }
    };
    let dir = out_sub("exp2lat");
    let mut booster = match Booster::start(job) {
        Ok(b) => b,
        Err(gle) => {
            kv("exp2lat_error", format!("booster start gle={gle}"));
            unsafe {
                CloseHandle(job);
            }
            return 1;
        }
    };

    let self_path = self_exe();
    let mut procs: Vec<ProcHandle> = Vec::new();
    for i in 0..n {
        let f = dir.join(format!("child-{i}.txt"));
        let fs = f.to_string_lossy().into_owned();
        match create_child(&self_path, &["lat-probe", "--out", &fs], 0x4, false, None) {
            Ok(p) => match assign_to_job(p, job) {
                Ok(p) => {
                    if let Err(gle) = resume_proc(&p) {
                        kv("exp2lat_error", format!("resume gle={gle}"));
                    }
                    procs.push(p);
                }
                Err(gle) => kv("exp2lat_error", format!("assign gle={gle}")),
            },
            Err(gle) => kv("exp2lat_error", format!("spawn gle={gle}")),
        }
    }
    let n_spawned = procs.len();

    let handles: Vec<HANDLE> = procs.iter().map(|p| p.process).collect();
    let _ = wait_all(&handles, 30_000);
    std::thread::sleep(Duration::from_millis(2000)); // drain
    let _ = terminate_job(job);
    let _killed = kill_stragglers(&procs);
    booster.stop();

    // Booster accounting.
    let counts = booster.counts();
    let n_notifications = counts
        .iter()
        .find(|(m, _)| *m == JOB_OBJECT_MSG_NEW_PROCESS)
        .map(|(_, c)| *c as usize)
        .unwrap_or(0);
    let mut set_ok = 0usize;
    let mut set_fail_by_gle: BTreeMap<u32, usize> = BTreeMap::new();
    let mut dead_at_boost = 0usize;
    for r in booster.records() {
        if r.open_ok {
            if r.set_ok {
                set_ok += 1;
            } else {
                *set_fail_by_gle.entry(r.gle).or_insert(0) += 1;
            }
        } else {
            dead_at_boost += 1;
            *set_fail_by_gle.entry(r.gle).or_insert(0) += 1;
        }
    }
    kv("exp2lat", format!(
        "n_spawned={n_spawned} n_notifications={n_notifications} missed={}",
        n_spawned.saturating_sub(n_notifications)
    ));
    kv("exp2lat", format!(
        "set_ok={set_ok} set_fail={set_fail_by_gle:?} dead_at_boost={dead_at_boost}"
    ));
    kv("exp2lat_msgs", format!("{counts:?}"));

    // Child latencies from the out files.
    let mut lats = Vec::new();
    let mut never = 0usize;
    let mut no_file = 0usize;
    let mut classes_at_t0: BTreeMap<u32, usize> = BTreeMap::new();
    for i in 0..n_spawned {
        let f = dir.join(format!("child-{i}.txt"));
        if !f.exists() {
            no_file += 1;
            continue;
        }
        let (saw, lat, cls) = read_lat_file(&f);
        if saw {
            lats.push(lat);
        } else {
            never += 1;
        }
        if let Some(c) = cls {
            *classes_at_t0.entry(c).or_insert(0) += 1;
        }
    }
    let med = if lats.is_empty() { 0.0 } else { median(&mut lats.clone()) };
    let p90 = if lats.is_empty() { 0.0 } else { percentile(&mut lats.clone(), 90.0) };
    let max = lats.iter().cloned().fold(0.0f64, f64::max);
    kv("exp2lat_child_latency_ms", format!(
        "median={med:.3} p90={p90:.3} max={max:.3} n={} never_saw_high={never} no_file={no_file}",
        lats.len()
    ));
    kv("exp2lat_class_at_t0", format!(
        "{:?}",
        classes_at_t0
            .iter()
            .map(|(c, n)| format!("{}({}):{}", c, class_name(*c), n))
            .collect::<Vec<_>>()
    ));
    kv("exp2lat_done", 1);
    0
}

// ------------------------------------------------------------ chain --------

const LEAF_LIFETIMES: [u64; 6] = [5, 10, 40, 120, 250, 400];

/// gen2: spawn 6 leaves with cycling lifetimes, wait, record, exit.
fn chain(args: &[String]) -> i32 {
    let t0 = qpc();
    let out_dir_arg = arg_val(args, "--out-dir").unwrap_or_else(|| ".".into());
    let gen = arg_val(args, "--gen").and_then(|s| s.parse().ok()).unwrap_or(2u32);
    let pid = unsafe { GetCurrentProcessId() };
    let cls0 = unsafe { GetPriorityClass(GetCurrentProcess()) };

    let self_path = self_exe();
    let mut leaves: Vec<ProcHandle> = Vec::new();
    for (i, lifetime) in LEAF_LIFETIMES.iter().enumerate() {
        let f = std::path::Path::new(&out_dir_arg).join(format!("leaf-{pid}-{i}.txt"));
        let fs = f.to_string_lossy().into_owned();
        match create_child(
            &self_path,
            &["leaf", "--lifetime-ms", &lifetime.to_string(), "--gen", "3", "--out", &fs],
            0,
            false,
            None,
        ) {
            Ok(p) => leaves.push(p),
            Err(gle) => kv("chain_error", format!("gen{gen} spawn leaf gle={gle}")),
        }
    }

    // Wait for the leaves while polling own class (the booster may lift us).
    let mut saw_high = cls0 == HIGH_PRIORITY_CLASS;
    let mut lat = if saw_high { ms_between(t0, qpc()) } else { 0.0 };
    let start = qpc();
    loop {
        let c = unsafe { GetPriorityClass(GetCurrentProcess()) };
        if c == HIGH_PRIORITY_CLASS && !saw_high {
            saw_high = true;
            lat = ms_between(t0, qpc());
        }
        let handles: Vec<HANDLE> = leaves.iter().map(|p| p.process).collect();
        if handles.is_empty() {
            break;
        }
        let r = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 1, 5) };
        if r == WAIT_OBJECT_0 || r == 0xFFFF_FFFF {
            break; // all leaves done (or wait failed)
        }
        if ms_between(start, qpc()) > 3000.0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = kill_stragglers(&leaves);
    drop(leaves);

    write_kv_file(
        &std::path::Path::new(&out_dir_arg).join(format!("g2-{pid}.txt")),
        &[
            ("pid", pid.to_string()),
            ("gen", gen.to_string()),
            ("lifetime", "0".into()),
            ("saw_high", saw_high.to_string()),
            ("latency_ms_or_0", format!("{lat:.3}")),
            ("class_at_t0", cls0.to_string()),
        ],
    );
    kv("chain", format!(
        "gen={gen} pid={pid} leaves_spawned={} saw_high={saw_high} latency_ms={lat:.3} class_at_t0={cls0}({})",
        LEAF_LIFETIMES.len(),
        class_name(cls0)
    ));
    0
}

/// leaf: busy-wait N ms polling GetPriorityClass like lat-probe, record, exit.
pub fn leaf(args: &[String]) -> i32 {
    let t0 = qpc();
    let lifetime_ms: f64 = arg_val(args, "--lifetime-ms")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100.0);
    let gen = arg_val(args, "--gen").and_then(|s| s.parse().ok()).unwrap_or(3u32);
    let out = arg_val(args, "--out");
    let pid = unsafe { GetCurrentProcessId() };
    let cls0 = unsafe { GetPriorityClass(GetCurrentProcess()) };
    let (saw_high, latency, t_high) = poll_until_high(t0, lifetime_ms);
    let path = out.unwrap_or_else(|| format!("out/exp2chain/leaf-{pid}.txt"));
    write_kv_file(
        std::path::Path::new(&path),
        &[
            ("pid", pid.to_string()),
            ("gen", gen.to_string()),
            ("lifetime", format!("{lifetime_ms:.0}")),
            ("saw_high", saw_high.to_string()),
            ("latency_ms_or_0", format!("{latency:.3}")),
            ("class_at_t0", cls0.to_string()),
            ("t0", t0.to_string()),
            ("t_high", t_high.to_string()),
        ],
    );
    kv("leaf", format!(
        "pid={pid} gen={gen} lifetime={lifetime_ms:.0} saw_high={saw_high} latency_ms={latency:.3} class_at_t0={cls0}({})",
        class_name(cls0)
    ));
    0
}

fn exp2_chain() -> i32 {
    let job = match create_job_kill_on_close() {
        Ok(j) => j,
        Err(gle) => {
            kv("exp2chain_error", format!("create job gle={gle}"));
            return 1;
        }
    };
    let dir = out_sub("exp2chain");
    let mut booster = match Booster::start(job) {
        Ok(b) => b,
        Err(gle) => {
            kv("exp2chain_error", format!("booster start gle={gle}"));
            unsafe {
                CloseHandle(job);
            }
            return 1;
        }
    };

    let self_path = self_exe();
    let dir_s = dir.to_string_lossy().into_owned();
    let mut gen2: Vec<ProcHandle> = Vec::new();
    for _ in 0..8 {
        match create_child(
            &self_path,
            &["chain", "--gen", "2", "--out-dir", &dir_s],
            0x4, // CREATE_SUSPENDED
            false,
            None,
        ) {
            Ok(p) => match assign_to_job(p, job) {
                Ok(p) => {
                    if let Err(gle) = resume_proc(&p) {
                        kv("exp2chain_error", format!("resume gle={gle}"));
                    }
                    gen2.push(p);
                }
                Err(gle) => kv("exp2chain_error", format!("assign gle={gle}")),
            },
            Err(gle) => kv("exp2chain_error", format!("spawn gle={gle}")),
        }
    }
    let n_spawned = gen2.len();

    let handles: Vec<HANDLE> = gen2.iter().map(|p| p.process).collect();
    let _ = wait_all(&handles, 30_000);
    std::thread::sleep(Duration::from_millis(2000)); // drain
    let _ = terminate_job(job);
    let killed = kill_stragglers(&gen2);
    if killed > 0 {
        kv("exp2chain_stragglers_killed", killed);
    }
    booster.stop();

    let counts = booster.counts();
    let n_notifications = counts
        .iter()
        .find(|(m, _)| *m == JOB_OBJECT_MSG_NEW_PROCESS)
        .map(|(_, c)| *c as usize)
        .unwrap_or(0);
    let mut set_ok = 0usize;
    let mut set_fail_by_gle: BTreeMap<u32, usize> = BTreeMap::new();
    let mut dead_at_boost = 0usize;
    for r in booster.records() {
        if r.open_ok {
            if r.set_ok {
                set_ok += 1;
            } else {
                *set_fail_by_gle.entry(r.gle).or_insert(0) += 1;
            }
        } else {
            dead_at_boost += 1;
            *set_fail_by_gle.entry(r.gle).or_insert(0) += 1;
        }
    }
    kv("exp2chain", format!("gen2_spawned={n_spawned} n_notifications={n_notifications}"));
    kv("exp2chain", format!(
        "set_ok={set_ok} set_fail={set_fail_by_gle:?} dead_at_boost={dead_at_boost}"
    ));
    kv("exp2chain_msgs", format!("{counts:?}"));

    // Accounting by generation from record files.
    let mut leaves_total = 0usize;
    let mut leaves_never_high = 0usize;
    let mut gen2_records = 0usize;
    let mut gen2_never_high = 0usize;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let rec = read_kv_file(&e.path());
            let gen: u32 = get_kv(&rec, "gen").and_then(|v| v.parse().ok()).unwrap_or(0);
            let saw = get_kv(&rec, "saw_high").map(|v| v == "true").unwrap_or(false);
            if gen == 3 {
                leaves_total += 1;
                if !saw {
                    leaves_never_high += 1;
                }
            } else if gen == 2 {
                gen2_records += 1;
                if !saw {
                    gen2_never_high += 1;
                }
            }
        }
    }
    kv("exp2chain_leaves", format!("records={leaves_total} finished_without_high={leaves_never_high}"));
    kv("exp2chain_gen2", format!("records={gen2_records} finished_without_high={gen2_never_high}"));
    kv("exp2chain_done", 1);
    0
}

pub fn dispatch(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("selfset") => selfset(),
        Some("exp2-latency") => exp2_latency(&args[1..]),
        Some("exp2-chain") => exp2_chain(),
        Some("chain") => chain(&args[1..]),
        Some("lat-probe") => lat_probe(&args[1..]),
        Some("leaf") => leaf(&args[1..]),
        _ => {
            kv("exp2_error", "unknown subcommand");
            2
        }
    }
}
