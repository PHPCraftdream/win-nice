//! Priority booster: when the job refused tree-wide priority (a limit above
//! NORMAL needs SeIncreaseBasePriorityPrivilege, which un-elevated runs don't
//! hold — Win32 error 1314), a companion thread delivers the requested
//! priority class to every process in the job's tree, one notification at a
//! time. Mechanism validated by the W1 spike on Windows 10 19045
//! un-elevated: 92/92 processes boosted, median 0.004 ms per
//! SetPriorityClass, bursts up to ~35 ms under process storms.
//!
//! The job is associated with a completion port. On
//! `JOB_OBJECT_MSG_NEW_PROCESS` (6) the new pid arrives as the lpOverlapped
//! VALUE (runtime behavior differs from the old MSDN wording — documented in
//! the `Sys::get_queued_completion_status` trait method docs); the booster
//! opens the process and sets the class on it. Per-process errors (already
//! exited, access denied) are deliberately non-fatal: the rest of the tree
//! still gets boosted. The thread ends on `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO`
//! (4) (job empty, nothing left to boost), on the stop flag, or on a port
//! error.

use super::*;
use crate::Handle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// How often the booster wakes up (`get_queued_completion_status` timeout) to
/// re-check the stop flag.
const BOOSTER_POLL_MS: u32 = 250;
/// Upper bound for joining the booster thread when the run engine finishes;
/// beyond it the thread detaches and self-terminates within one poll interval.
const BOOSTER_JOIN_BUDGET_MS: u64 = 2000;

/// Map a requested priority class to the class handed to new processes.
/// Realtime downgrades to High — the same convention as the existing
/// job-limit and creation-flag paths.
pub(crate) fn boost_class(prio: crate::PriorityClass) -> u32 {
    if prio == crate::PriorityClass::Realtime {
        crate::PriorityClass::High.to_u32()
    } else {
        prio.to_u32()
    }
}

/// The stderr note printed when the booster takes over the priority: says
/// WHAT applies the priority and its honest latency envelope.
pub(crate) fn booster_note(prio: crate::PriorityClass) -> String {
    format!(
        "wnice: note: {} priority applies to every process in the tree via the booster (job completion port + SetPriorityClass, not a job limit); boost is usually sub-millisecond (up to a few ms in spawn bursts), so very short-lived children may exit before it lands",
        format!("{prio:?}").to_lowercase()
    )
}

/// Handle one dequeued job notification. Returns true when the booster must
/// stop: only `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO` (job empty, nothing left
/// to boost) does. Job-time end and other notifications are irrelevant here.
pub(crate) fn handle_message(sys: &dyn Sys, msg: u32, pid: usize, class: u32) -> bool {
    match msg {
        crate::JOB_OBJECT_MSG_NEW_PROCESS => {
            // Open failure (process already exited / access denied) is
            // deliberately non-fatal: per-process errors never stop the
            // booster, the rest of the tree still gets boosted.
            if let Ok(p) = sys.open_process(
                pid as u32,
                crate::PROCESS_QUERY_LIMITED_INFORMATION | crate::PROCESS_SET_INFORMATION,
            ) {
                let _ = sys.set_priority_class(&p, class);
            }
            false
        }
        crate::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO => true,
        _ => false,
    }
}

/// Why the booster loop ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BoosterExit {
    /// Job empty: every boosted process exited, nothing left to boost.
    JobEmpty,
    /// The stop flag was set (the run engine finished).
    Stopped,
    /// The completion port failed with this Win32 error code.
    PortFailed(u32),
}

/// Poll the completion port until the job empties, the stop flag is set, or
/// the port itself fails. The short poll timeout bounds how long a set stop
/// flag can go unnoticed.
pub(crate) fn booster_loop(
    sys: &dyn Sys,
    port: &Handle,
    class: u32,
    stop: &AtomicBool,
) -> BoosterExit {
    loop {
        if stop.load(Ordering::SeqCst) {
            return BoosterExit::Stopped;
        }
        match sys.get_queued_completion_status(port, BOOSTER_POLL_MS) {
            Ok((msg, pid)) => {
                if handle_message(sys, msg, pid, class) {
                    return BoosterExit::JobEmpty;
                }
            }
            Err(crate::WAIT_TIMEOUT) => {} // no packet: re-check the stop flag
            Err(code) => return BoosterExit::PortFailed(code),
        }
    }
}

/// Which booster-setup step failed, with its Win32 error code.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BoosterSetupError {
    /// CreateIoCompletionPort failed.
    CreatePort(u32),
    /// SetInformationJobObject (associate completion port) failed.
    Associate(u32),
    /// The booster thread could not be spawned (Win32 last-error).
    Thread(u32),
}

/// Runs the booster thread. Drop it to signal the stop flag and join the
/// thread (bounded; see `BOOSTER_JOIN_BUDGET_MS`).
pub(crate) struct BoosterGuard {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BoosterGuard {
    /// Create the completion port, associate `job` with it, and start the booster
    /// thread. The port handle moves into the thread: `Handle` is deliberately
    /// not `Send` (single-close RAII), so the raw value transfers and the thread
    /// re-owns it — a kernel HANDLE is valid on other threads and RAII closes it
    /// exactly once, on the thread that owns it. On failure up to (and incl.) the
    /// association no handle is left behind. `Err` says which setup step failed
    /// (`BoosterSetupError`) with its Win32 code.
    pub(crate) fn start(
        sys: &Arc<dyn Sys>,
        job: &Handle,
        class: u32,
    ) -> Result<BoosterGuard, BoosterSetupError> {
        let port = sys
            .create_completion_port()
            .map_err(BoosterSetupError::CreatePort)?;
        sys.associate_job_with_completion_port(job, &port)
            .map_err(BoosterSetupError::Associate)?;
        let stop = Arc::new(AtomicBool::new(false));
        // Move the port into the thread without closing it here. usize is
        // Send; the raw HANDLE pointer is not.
        let raw = port.raw() as usize;
        #[cfg(test)]
        let book = port.book();
        std::mem::forget(port);
        let sys_for_thread = Arc::clone(sys);
        let stop_for_thread = Arc::clone(&stop);
        // Test-only: the thread's port drop must land in the minting
        // instance's book. The closure gets a cheap Arc clone of it so the
        // spawn-failure arm below can still re-own through `book` itself.
        #[cfg(test)]
        let book_for_thread = book.clone();
        let thread = std::thread::Builder::new()
            .name("wnice-boost".to_string())
            .spawn(move || {
                #[cfg(test)]
                let port = reown_port(raw, book_for_thread);
                #[cfg(not(test))]
                let port = reown_port(raw);
                let _exit = booster_loop(sys_for_thread.as_ref(), &port, class, &stop_for_thread);
                // `port` drops here: exactly one CloseHandle on this thread
                // (and, in test builds, its drop lands in the owning
                // instance's book).
            });
        match thread {
            Ok(t) => Ok(BoosterGuard {
                stop,
                thread: Some(t),
            }),
            // Thread spawn failed: re-own the raw port handle so its RAII close
            // still runs exactly once. Read the error code first — a real
            // CloseHandle below could clobber last-error.
            Err(_) => {
                let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                #[cfg(test)]
                drop(reown_port(raw, book));
                #[cfg(not(test))]
                drop(reown_port(raw));
                Err(BoosterSetupError::Thread(code))
            }
        }
    }
}

/// Re-own the raw port handle after crossing the thread boundary. Real
/// builds close it via RAII exactly once; test builds use the no-close
/// constructor so a fake handle never reaches the real `CloseHandle` and
/// still counts its drop (the same contract as every FakeSys-minted
/// handle): when the minting instance's book is known it is re-attached,
/// keeping the port's drop counted per instance.
#[cfg(test)]
fn reown_port(raw: usize, book: Option<std::sync::Arc<crate::FakeHandleBook>>) -> Handle {
    match book {
        Some(b) => Handle::from_raw_no_close_booked(raw as *mut core::ffi::c_void, b),
        None => Handle::from_raw_no_close(raw as *mut core::ffi::c_void),
    }
}

/// Real-build `reown_port`: plain RAII close, exactly once, no book (see
/// the test variant for the fake-handle contract).
#[cfg(not(test))]
fn reown_port(raw: usize) -> Handle {
    Handle::from_raw(raw as *mut core::ffi::c_void)
}

impl Drop for BoosterGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(BOOSTER_JOIN_BUDGET_MS);
            while !t.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if t.is_finished() {
                let _ = t.join();
            }
            // else: drop the JoinHandle — dropping detaches the thread. It
            // self-terminates within one poll interval and its port handle
            // closes via RAII, keeping the run's exit bounded.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::fakesys::{Call, FakeSys};
    use crate::PriorityClass;

    /// Assert this instance's minted handle count equals its dropped count —
    /// per-instance (each FakeSys owns a book), so parallel tests cannot bleed
    /// into each other and booster-thread drops are still counted (shared Arc).
    fn assert_handles_balance(sys: &FakeSys, ctx: &str) {
        let minted = sys.handles_minted();
        let drops = sys.handles_dropped();
        assert_eq!(
            minted, drops,
            "{ctx}: fake handles minted {minted} but only {drops} dropped"
        );
    }

    /// Bounded spin (1 ms steps, one join budget) until `pred` holds, then
    /// assert it. The booster thread runs concurrently with the test thread,
    /// so observable progress (a logged call, a dropped handle) can only be
    /// awaited this way; the final assertions stay exact, nothing passes or
    /// fails based on elapsed time.
    fn spin_until(ctx: &str, pred: impl Fn() -> bool) {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(BOOSTER_JOIN_BUDGET_MS);
        while !pred() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            pred(),
            "{ctx}: condition not reached within the join budget"
        );
    }

    #[test]
    fn boost_class_maps_realtime_to_high_and_passes_others_through() {
        assert_eq!(
            boost_class(PriorityClass::Realtime),
            PriorityClass::High.to_u32()
        );
        assert_eq!(
            boost_class(PriorityClass::High),
            PriorityClass::High.to_u32()
        );
        assert_eq!(
            boost_class(PriorityClass::AboveNormal),
            PriorityClass::AboveNormal.to_u32()
        );
        assert_eq!(
            boost_class(PriorityClass::Idle),
            PriorityClass::Idle.to_u32()
        );
        assert_eq!(
            boost_class(PriorityClass::BelowNormal),
            PriorityClass::BelowNormal.to_u32()
        );
        assert_eq!(
            boost_class(PriorityClass::Normal),
            PriorityClass::Normal.to_u32()
        );
    }

    #[test]
    fn new_process_message_opens_and_sets_priority_once() {
        let sys = FakeSys::new();
        let class = PriorityClass::High.to_u32();
        let access = crate::PROCESS_QUERY_LIMITED_INFORMATION | crate::PROCESS_SET_INFORMATION;
        assert!(!handle_message(
            &sys,
            crate::JOB_OBJECT_MSG_NEW_PROCESS,
            777,
            class
        ));
        assert_eq!(
            sys.take(),
            vec![
                Call::OpenProcess { pid: 777, access },
                Call::SetPriorityClass { class },
            ]
        );
        // The opened fake process handle dropped exactly once (RAII), into
        // this instance's own book.
        assert_eq!(sys.handles_dropped(), 1);
    }

    #[test]
    fn open_process_failure_is_not_fatal() {
        let sys = FakeSys::new().fail_next("OpenProcess", 5);
        let class = PriorityClass::Normal.to_u32();
        assert!(!handle_message(
            &sys,
            crate::JOB_OBJECT_MSG_NEW_PROCESS,
            9,
            class
        ));
        // A failed call is not logged (the FakeSys-wide journal convention), and
        // with the open refused there is nothing to boost: no SetPriorityClass.
        assert_eq!(sys.count(|c| matches!(c, Call::OpenProcess { .. })), 0);
        assert_eq!(sys.count(|c| matches!(c, Call::SetPriorityClass { .. })), 0);
    }

    #[test]
    fn set_priority_class_failure_is_not_fatal() {
        let sys = FakeSys::new().fail_next("SetPriorityClass", 6);
        let class = PriorityClass::Idle.to_u32();
        assert!(!handle_message(
            &sys,
            crate::JOB_OBJECT_MSG_NEW_PROCESS,
            9,
            class
        ));
        assert_eq!(sys.count(|c| matches!(c, Call::OpenProcess { .. })), 1);
        // The open succeeded and IS logged; the refused SetPriorityClass is not.
        assert_eq!(sys.count(|c| matches!(c, Call::SetPriorityClass { .. })), 0);
    }

    #[test]
    fn active_process_zero_message_stops() {
        let sys = FakeSys::new();
        assert!(handle_message(
            &sys,
            crate::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO,
            0,
            PriorityClass::High.to_u32()
        ));
        assert!(sys.take().is_empty());
    }

    #[test]
    fn unknown_message_is_ignored() {
        let sys = FakeSys::new();
        // 1 = end-of-job-time: irrelevant to the booster.
        assert!(!handle_message(&sys, 1, 0, PriorityClass::High.to_u32()));
        assert!(sys.take().is_empty());
    }

    #[test]
    fn loop_drains_queue_then_stops_on_active_zero() {
        let sys = FakeSys::new()
            .queue_boost(crate::JOB_OBJECT_MSG_NEW_PROCESS, 111)
            .queue_boost(crate::JOB_OBJECT_MSG_NEW_PROCESS, 222)
            .queue_boost(crate::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO, 0);
        let port = sys.handle();
        let stop = AtomicBool::new(false);
        let class = PriorityClass::High.to_u32();
        assert_eq!(
            booster_loop(&sys, &port, class, &stop),
            BoosterExit::JobEmpty
        );
        let access = crate::PROCESS_QUERY_LIMITED_INFORMATION | crate::PROCESS_SET_INFORMATION;
        assert_eq!(
            sys.take(),
            vec![
                Call::OpenProcess { pid: 111, access },
                Call::SetPriorityClass { class },
                Call::OpenProcess { pid: 222, access },
                Call::SetPriorityClass { class },
            ]
        );
        drop(port);
        assert_handles_balance(&sys, "loop_drains_queue_then_stops_on_active_zero");
    }

    #[test]
    fn loop_returns_stopped_when_flag_is_set() {
        let sys = FakeSys::new();
        let port = sys.handle();
        let stop = AtomicBool::new(true);
        assert_eq!(
            booster_loop(&sys, &port, PriorityClass::High.to_u32(), &stop),
            BoosterExit::Stopped
        );
        // The stop flag is checked before the first poll: no sys calls at all.
        assert!(sys.take().is_empty());
    }

    #[test]
    fn loop_port_failure_ends_loop_with_the_code() {
        let sys = FakeSys::new().fail_next("GetQueuedCompletionStatus", 87);
        let port = sys.handle();
        let stop = AtomicBool::new(false);
        assert_eq!(
            booster_loop(&sys, &port, PriorityClass::High.to_u32(), &stop),
            BoosterExit::PortFailed(87)
        );
        drop(port);
        assert_handles_balance(&sys, "loop_port_failure_ends_loop_with_the_code");
    }

    #[test]
    fn guard_start_associates_boosts_queued_processes_and_joins_on_drop() {
        let fake = Arc::new(
            FakeSys::new()
                .queue_boost(crate::JOB_OBJECT_MSG_NEW_PROCESS, 4242)
                .queue_boost(crate::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO, 0),
        );
        let job = fake.handle();
        let fake_ref = Arc::clone(&fake);
        let sys: Arc<dyn Sys> = fake;
        let guard =
            BoosterGuard::start(&sys, &job, PriorityClass::High.to_u32()).expect("booster start");
        // The booster thread runs concurrently with this test: wait until the
        // queued NEW_PROCESS was boosted before dropping the guard, else the
        // stop flag could win the race and skip the queue.
        spin_until("queued process boosted", || {
            fake_ref.count(|c| matches!(c, Call::SetPriorityClass { .. })) >= 1
        });
        // The thread ended on ACTIVE_PROCESS_ZERO, so the join returns
        // promptly instead of burning the budget.
        drop(guard);
        drop(job);
        spin_until("all fake handles dropped", || {
            fake_ref.handles_dropped() == fake_ref.handles_minted()
        });
        assert_eq!(
            fake_ref.take(),
            vec![
                Call::CreatePort,
                Call::AssociatePort,
                Call::OpenProcess {
                    pid: 4242,
                    access: crate::PROCESS_QUERY_LIMITED_INFORMATION
                        | crate::PROCESS_SET_INFORMATION
                },
                Call::SetPriorityClass {
                    class: PriorityClass::High.to_u32()
                },
            ]
        );
        assert_handles_balance(&fake_ref, "guard_start_associates_boosts_queued");
    }

    #[test]
    fn guard_drop_stops_thread_when_no_messages_come() {
        let fake = Arc::new(FakeSys::new());
        let job = fake.handle();
        let fake_ref = Arc::clone(&fake);
        let sys: Arc<dyn Sys> = fake;
        let guard =
            BoosterGuard::start(&sys, &job, PriorityClass::High.to_u32()).expect("booster start");
        // Empty queue: the stop-flag path ends the thread within one poll and
        // the FakeSys poll timeout returns instantly, so the join is prompt.
        drop(guard);
        drop(job);
        spin_until("all fake handles dropped", || {
            fake_ref.handles_dropped() == fake_ref.handles_minted()
        });
        assert_eq!(fake_ref.take(), vec![Call::CreatePort, Call::AssociatePort]);
        assert_eq!(fake_ref.count(|c| matches!(c, Call::OpenProcess { .. })), 0);
        assert_eq!(
            fake_ref.count(|c| matches!(c, Call::SetPriorityClass { .. })),
            0
        );
        assert_handles_balance(&fake_ref, "guard_drop_stops_thread");
    }
}
