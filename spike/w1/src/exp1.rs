//! exp1: CPU-rate measurement spike — burner child + the 8-scenario matrix.

use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

use crate::common::{
    arg_val, cpu_rate_hard_cap, cpu_rate_min_max, create_job_kill_on_close, has_flag, kv,
    logical_cpus, cpu_seconds, kill_stragglers, median, resume_proc, self_exe,
    set_job_cpu_rate, spawn_suspended_into_job, terminate_job, wait_all, Booster,
};

/// Pure CPU burner: FNV-ish hash loop, no syscalls in the hot loop, QPC
/// checked every 65536 iterations.
fn run_burner(args: &[String]) -> i32 {
    let seconds: f64 = arg_val(args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    if has_flag(args, "--thread-high") {
        match crate::common::set_self_thread_high() {
            Ok(()) => kv("burner_thread_high", "ok"),
            Err(gle) => kv("burner_thread_high", format!("fail gle={gle}")),
        }
    }
    let t0 = crate::common::qpc();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut iters: u64 = 0;
    loop {
        for _ in 0..65536u64 {
            h ^= iters;
            h = h.wrapping_mul(0x100_0000_01b3);
            iters += 1;
        }
        if crate::common::ms_between(t0, crate::common::qpc()) >= seconds * 1000.0 {
            break;
        }
    }
    kv("burner_iters", iters);
    kv("burner_seconds", seconds);
    0
}

#[derive(Clone, Copy, PartialEq)]
enum MainCtrl {
    None,
    Hard,
    Soft,
    /// SAFETY: like Soft, but min=10%, max=100% — floor-only rate control
    /// (JOBOBJECT_CPU_RATE_CONTROL with MinRate/MaxRate semantics).
    SoftFloor,
}

#[derive(Clone, Copy, PartialEq)]
enum CompFlavor {
    None,
    Normal,
    ThreadHigh,
    BoostHigh,
}

struct Scenario {
    name: &'static str,
    main_ctrl: MainCtrl,
    comp: CompFlavor,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario { name: "hard10-alone", main_ctrl: MainCtrl::Hard, comp: CompFlavor::None },
        Scenario { name: "none-normal", main_ctrl: MainCtrl::None, comp: CompFlavor::Normal },
        Scenario { name: "hard10-normal", main_ctrl: MainCtrl::Hard, comp: CompFlavor::Normal },
        Scenario { name: "soft10-alone", main_ctrl: MainCtrl::Soft, comp: CompFlavor::None },
        Scenario { name: "soft10-normal", main_ctrl: MainCtrl::Soft, comp: CompFlavor::Normal },
        Scenario { name: "soft10-threadhigh", main_ctrl: MainCtrl::Soft, comp: CompFlavor::ThreadHigh },
        Scenario { name: "soft10-boosthigh", main_ctrl: MainCtrl::Soft, comp: CompFlavor::BoostHigh },
        Scenario { name: "hard10-boosthigh", main_ctrl: MainCtrl::Hard, comp: CompFlavor::BoostHigh },
        Scenario { name: "softfloor-normal", main_ctrl: MainCtrl::SoftFloor, comp: CompFlavor::Normal },
        Scenario { name: "softfloor-boosthigh", main_ctrl: MainCtrl::SoftFloor, comp: CompFlavor::BoostHigh },
    ]
}

struct RepResult {
    name: &'static str,
    rep: usize,
    main_pct: Option<f64>,
    comp_pct: Option<f64>,
    share: Option<f64>,
    set_ok: usize,
    set_fail: usize,
}

/// Sum of cpu_seconds over a side's ProcHandles; Err carries the first gle.
fn snapshot(procs: &[crate::common::ProcHandle]) -> Result<f64, u32> {
    let mut total = 0.0f64;
    for p in procs {
        total += cpu_seconds(p.process)?;
    }
    Ok(total)
}

fn cleanup_rep(
    main_job: HANDLE,
    comp_job: Option<HANDLE>,
    main_procs: &mut Vec<crate::common::ProcHandle>,
    comp_procs: &mut Vec<crate::common::ProcHandle>,
    booster: Option<&mut Booster>,
) {
    let _ = terminate_job(main_job);
    if let Some(j) = comp_job {
        let _ = terminate_job(j);
    }
    let mut handles: Vec<HANDLE> = main_procs.iter().map(|p| p.process).collect();
    handles.extend(comp_procs.iter().map(|p| p.process));
    let _ = wait_all(&handles, 2000);
    let stragglers = kill_stragglers(main_procs) + kill_stragglers(comp_procs);
    if stragglers > 0 {
        kv("exp1_stragglers_killed", stragglers);
    }
    if let Some(b) = booster {
        b.stop();
    }
    // ProcHandles drop here (closes process+thread handles).
}

fn run_rep(sc: &Scenario, rep: usize, seconds: f64, _boosthigh: bool) -> Option<RepResult> {
    let main_job = match create_job_kill_on_close() {
        Ok(h) => h,
        Err(gle) => {
            kv("exp1_error", format!("{}: create main job gle={gle}", sc.name));
            return None;
        }
    };
    match sc.main_ctrl {
        MainCtrl::Hard => {
            if let Err(gle) = set_job_cpu_rate(main_job, &cpu_rate_hard_cap(1000)) {
                kv("exp1_error", format!("{}: main hard cap gle={gle}", sc.name));
            }
        }
        MainCtrl::Soft => {
            if let Err(gle) = set_job_cpu_rate(main_job, &cpu_rate_min_max(1000, 1000)) {
                kv("exp1_error", format!("{}: main minmax gle={gle}", sc.name));
            }
        }
        MainCtrl::SoftFloor => {
            // min=10%, max=100%: floor-only job under saturating competition.
            if let Err(gle) = set_job_cpu_rate(main_job, &cpu_rate_min_max(1000, 10000)) {
                kv("exp1_error", format!("{}: main minmax(floor) gle={gle}", sc.name));
            }
        }
        MainCtrl::None => {}
    }

    let want_comp = sc.comp != CompFlavor::None;
    let mut comp_job: HANDLE = std::ptr::null_mut();
    if want_comp {
        comp_job = match create_job_kill_on_close() {
            Ok(h) => h,
            Err(gle) => {
                kv("exp1_error", format!("{}: create comp job gle={gle}", sc.name));
                unsafe {
                    CloseHandle(main_job);
                }
                return None;
            }
        };
        // Competitor job is itself hard-capped at 50%.
        if let Err(gle) = set_job_cpu_rate(comp_job, &cpu_rate_hard_cap(5000)) {
            kv("exp1_error", format!("{}: comp 50% cap gle={gle}", sc.name));
        }
    }

    let mut booster = if sc.comp == CompFlavor::BoostHigh {
        match Booster::start(comp_job) {
            Ok(b) => Some(b),
            Err(gle) => {
                kv("exp1_error", format!("{}: booster start gle={gle}", sc.name));
                None
            }
        }
    } else {
        None
    };

    // Burners live for the measurement window plus the 0.6s spin-up lead.
    let secs = format!("{}", seconds + 0.6);
    let mut main_procs: Vec<crate::common::ProcHandle> = Vec::new();
    let mut comp_procs: Vec<crate::common::ProcHandle> = Vec::new();
    for _ in 0..10 {
        match spawn_suspended_into_job(main_job, &self_exe(), &["burner", "--seconds", &secs], 0)
        {
            Ok(p) => main_procs.push(p),
            Err(gle) => {
                kv("exp1_error", format!("{}: spawn main burner gle={gle}", sc.name));
                cleanup_rep(main_job, if want_comp { Some(comp_job) } else { None }, &mut main_procs, &mut comp_procs, booster.as_mut());
                return None;
            }
        }
    }
    let comp_args: Vec<&str> = match sc.comp {
        CompFlavor::ThreadHigh => vec!["burner", "--seconds", &secs, "--thread-high"],
        _ => vec!["burner", "--seconds", &secs],
    };
    // FIX(run): spawn competitors only when the scenario wants them; the
    // -alone scenarios previously spawned 12 burners into a null comp_job
    // (AssignProcessToJobObject gle=6) and failed outright.
    let n_comp = if want_comp { 12 } else { 0 };
    for _ in 0..n_comp {
        match spawn_suspended_into_job(comp_job, &self_exe(), &comp_args, 0) {
            Ok(p) => comp_procs.push(p),
            Err(gle) => {
                kv("exp1_error", format!("{}: spawn comp burner gle={gle}", sc.name));
                cleanup_rep(main_job, if want_comp { Some(comp_job) } else { None }, &mut main_procs, &mut comp_procs, booster.as_mut());
                return None;
            }
        }
    }

    for p in main_procs.iter().chain(comp_procs.iter()) {
        if let Err(gle) = resume_proc(p) {
            kv("exp1_error", format!("{}: resume gle={gle}", sc.name));
        }
    }

    // 0.3s settle, then the exact measurement window.
    std::thread::sleep(Duration::from_millis(300));
    let main0 = snapshot(&main_procs);
    let comp0 = snapshot(&comp_procs);
    std::thread::sleep(Duration::from_secs_f64(seconds));
    let main1 = snapshot(&main_procs);
    let comp1 = snapshot(&comp_procs);

    let cpus = logical_cpus() as f64;
    let window = seconds;
    let d_main = match (&main0, &main1) {
        (Ok(a), Ok(b)) => Some((b - a).max(0.0)),
        _ => None,
    };
    let d_comp = match (&comp0, &comp1) {
        (Ok(a), Ok(b)) => Some((b - a).max(0.0)),
        _ => None,
    };
    let main_pct = d_main.map(|d| 100.0 * d / (window * cpus));
    let comp_pct = d_comp.map(|d| 100.0 * d / (window * cpus));
    let share = match (d_main, d_comp) {
        (Some(m), Some(c)) => {
            let busy = m + c;
            if busy > 0.0 {
                Some(m / busy)
            } else {
                Some(0.0)
            }
        }
        _ => None,
    };

    let mut set_ok = 0usize;
    let mut set_fail = 0usize;
    if let Some(b) = booster.as_ref() {
        for r in b.records() {
            if r.set_ok {
                set_ok += 1;
            } else {
                set_fail += 1;
            }
        }
    }

    kv("scenario", format!(
        "{} rep={} main_pct={} comp_pct={} main_share_of_busy={} boost_set_ok={} boost_set_fail={}",
        sc.name,
        rep,
        main_pct.map(|v| format!("{v:.2}")).unwrap_or_else(|| "n/a".into()),
        comp_pct.map(|v| format!("{v:.2}")).unwrap_or_else(|| "n/a".into()),
        share.map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".into()),
        set_ok,
        set_fail,
    ));
    if let Err(gle) = main0 {
        kv("exp1_error", format!("{}: GetProcessTimes(main) gle={gle}", sc.name));
    }
    if let Err(gle) = main1 {
        kv("exp1_error", format!("{}: GetProcessTimes(main) gle={gle}", sc.name));
    }
    if let Err(gle) = comp0 {
        kv("exp1_error", format!("{}: GetProcessTimes(comp) gle={gle}", sc.name));
    }
    if let Err(gle) = comp1 {
        kv("exp1_error", format!("{}: GetProcessTimes(comp) gle={gle}", sc.name));
    }

    cleanup_rep(
        main_job,
        if want_comp { Some(comp_job) } else { None },
        &mut main_procs,
        &mut comp_procs,
        booster.as_mut(),
    );

    Some(RepResult { name: sc.name, rep, main_pct, comp_pct, share, set_ok, set_fail })
}

fn run_exp1(args: &[String]) -> i32 {
    let seconds: f64 = arg_val(args, "--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let reps: usize = arg_val(args, "--reps")
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let boosthigh = has_flag(args, "--boosthigh");
    let only = arg_val(args, "--only");

    kv("exp1_start", format!("seconds={seconds} reps={reps} boosthigh={boosthigh} only={:?} cpus={}", only, logical_cpus()));

    let mut results: Vec<RepResult> = Vec::new();
    for rep in 1..=reps {
        for sc in scenarios() {
            if sc.comp == CompFlavor::BoostHigh && !boosthigh {
                kv("scenario_skipped", format!("{} rep={rep} (needs --boosthigh)", sc.name));
                continue;
            }
            if let Some(f) = only.as_deref() {
                if !sc.name.contains(f) {
                    continue;
                }
            }
            match run_rep(&sc, rep, seconds, boosthigh) {
                Some(r) => results.push(r),
                None => kv("scenario_failed", format!("{} rep={rep}", sc.name)),
            }
            // ~2.5s cooldown between scenarios.
            std::thread::sleep(Duration::from_millis(2500));
        }
    }

    // Final summary: median of reps per scenario.
    let mut by_name: Vec<&'static str> = results.iter().map(|r| r.name).collect();
    by_name.sort();
    by_name.dedup();
    for name in by_name {
        let mut m = Vec::new();
        let mut c = Vec::new();
        let mut s = Vec::new();
        let mut ok = 0usize;
        let mut fail = 0usize;
        for r in results.iter().filter(|r| r.name == name) {
            if let Some(v) = r.main_pct {
                m.push(v);
            }
            if let Some(v) = r.comp_pct {
                c.push(v);
            }
            if let Some(v) = r.share {
                s.push(v);
            }
            ok += r.set_ok;
            fail += r.set_fail;
        }
        kv("summary", format!(
            "scenario={name} reps={} main_pct_med={} comp_pct_med={} share_med={} boost_set_ok={} boost_set_fail={}",
            results.iter().filter(|r| r.name == name).count(),
            median(&mut m),
            median(&mut c),
            median(&mut s),
            ok,
            fail,
        ));
    }
    kv("exp1_done", 1);
    0
}

pub fn dispatch(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("burner") => run_burner(&args[1..]),
        Some("exp1") => run_exp1(&args[1..]),
        _ => {
            kv("exp1_error", "unknown subcommand (want burner|exp1)");
            2
        }
    }
}
