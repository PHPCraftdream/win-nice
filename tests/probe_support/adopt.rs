//! Process-adoption integration coverage for the harness-free probe binary.
use super::{probe_exe, CREATE_NO_WINDOW};
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, TerminateProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

struct Guard {
    child: Child,
    handle: Option<HANDLE>,
}
impl Guard {
    fn new(child: Child) -> Self {
        let mut guard = Self {
            child,
            handle: None,
        };
        // SAFETY: the child ID identifies the spawned process; requested rights are used for cleanup/query.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                0,
                guard.child.id(),
            )
        };
        guard.handle = (!handle.is_null()).then_some(handle);
        assert!(guard.handle.is_some(), "retaining process handle failed");
        guard
    }
    fn wait(&mut self) -> std::process::ExitStatus {
        self.child.wait().expect("wait")
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: the retained handle is owned by this guard and remains valid through the API calls.
        unsafe {
            let Some(handle) = self.handle else {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            };
            let mut code = 0;
            if GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32 {
                TerminateProcess(handle, 1);
            }
            let _ = windows_sys::Win32::System::Threading::WaitForSingleObject(handle, 5000);
            CloseHandle(handle);
        }
        let _ = self.child.wait();
    }
}

pub fn run() {
    let root = std::env::temp_dir().join(format!(
        "wnice-adopt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).expect("create unique temp dir");
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _temp = Temp(root.clone());
    let exe = probe_exe();
    let first_name = format!("wnice-adopt-{}.exe", std::process::id());
    let second_name = format!("wnice-unselected-{}.exe", std::process::id());
    let first = root.join(&first_name);
    let second = root.join(&second_name);
    std::fs::copy(&exe, &first).expect("copy selected helper");
    std::fs::copy(&exe, &second).expect("copy unselected helper");
    let go = root.join("selected-go");
    let second_go = root.join("unselected-go");
    let marker = root.join("selected-result");
    let second_marker = root.join("unselected-result");
    // SAFETY: pseudo-handle identifies this harness process; NULL asks whether it is in any job.
    let harness_in_job = unsafe {
        let mut in_job = 0;
        assert_ne!(
            IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job),
            0,
            "query harness job membership failed: {}",
            std::io::Error::last_os_error()
        );
        in_job != 0
    };
    let start = |path: &PathBuf, go_path: &PathBuf, out: &PathBuf| {
        let mut c = Command::new(path);
        c.creation_flags(CREATE_NO_WINDOW)
            .env("WNICE_PROBE", "adopt-wait")
            .env(
                "WNICE_PROBE_ARGS",
                format!("{}\u{1f}{}", go_path.display(), out.display()),
            )
            .env("WNICE_PROBE_DEPTH", "1");
        Guard::new(c.spawn().expect("spawn helper"))
    };
    let mut selected = start(&first, &go, &marker);
    let mut unselected = start(&second, &second_go, &second_marker);

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wnice"));
    cmd.creation_flags(CREATE_NO_WINDOW)
        .args([
            "--adopt",
            first_name.as_str(),
            "-c",
            "20",
            "--",
            exe.to_str().unwrap(),
            "sleep",
            "10000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut wrapper = Guard::new(cmd.spawn().expect("spawn wnice"));
    let stderr = wrapper.child.stderr.take().expect("stderr");
    let (tx, rx) = std::sync::mpsc::channel();
    let stderr_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut observed = false;
    let mut containment = false;
    let mut stderr_lines = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) if line.contains(&format!("wnice: adopted {first_name} (pid ")) => {
                stderr_lines.push(line);
                observed = true;
                break;
            }
            Ok(line)
                if line.contains(&format!("wnice: could not adopt {first_name} (pid "))
                    && line.contains("ERROR_ACCESS_DENIED")
                    && line.contains("os error 5") =>
            {
                stderr_lines.push(line);
                containment = true;
                break;
            }
            Ok(line) if line.contains("ERROR_ACCESS_DENIED") => {
                panic!("unexpected access-denied adoption error: {line}")
            }
            Ok(line) => stderr_lines.push(line),
            _ => {}
        }
    }
    assert!(
        observed || containment,
        "no adoption confirmation or explicit access-denied result"
    );
    let in_job = |handle: HANDLE| {
        // SAFETY: retained process handle remains valid; NULL asks whether it is in any job.
        unsafe {
            let mut value = 0;
            assert_ne!(
                IsProcessInJob(handle, std::ptr::null_mut(), &mut value),
                0,
                "query child job membership failed: {}",
                std::io::Error::last_os_error()
            );
            value != 0
        }
    };
    let helper_in_job = in_job(selected.handle.unwrap());
    let wrapper_in_job = in_job(wrapper.handle.unwrap());
    if containment {
        // Release only after the refusal line, so the helper's own adopt-wait
        // probe records its current job membership independently of stale PIDs.
        std::fs::write(&go, "go").expect("release selected helper for membership proof");
        let proof_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < proof_deadline && !marker.exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let proof = std::fs::read_to_string(&marker).expect("selected helper membership proof");
        assert!(
            harness_in_job && helper_in_job && wrapper_in_job,
            "explicit inherited CI job containment: nested assignment refusal is established only when the preexisting harness job is observed on the direct-child helper and wrapper too (harness={harness_in_job}, helper={helper_in_job}, wrapper={wrapper_in_job}); helper probe={proof}; this is not a skipped test"
        );
        assert!(
            proof.starts_with("member:true:"),
            "helper's own adopt-wait probe did not confirm job membership: {proof}"
        );
        std::fs::write(&second_go, "go").expect("query unselected helper");
        let second_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < second_deadline && !second_marker.exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let second_result =
            std::fs::read_to_string(&second_marker).expect("unselected helper result");
        assert!(
            second_result == "outside" || second_result.starts_with("member:true:"),
            "invalid unselected helper report: {second_result}"
        );
        assert_eq!(selected.wait().code(), Some(0));
        assert_eq!(unselected.wait().code(), Some(0));
        // SAFETY: retained process handle belongs to the live wrapper child; termination is intentional cleanup.
        unsafe { TerminateProcess(wrapper.handle.unwrap(), 0) };
        let _ = wrapper.wait();
        let _ = stderr_thread.join();
        stderr_lines.extend(rx.try_iter());
        assert!(
            !stderr_lines.iter().any(|line| line.contains(&second_name)),
            "wrapper logged adoption of unselected helper through exit: {stderr_lines:?}"
        );
        return;
    }
    assert!(observed, "no positive adoption confirmation");
    std::fs::write(&go, "go").expect("release selected helper after confirmation");
    let cap_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < cap_deadline && !marker.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let cap = std::fs::read_to_string(&marker).expect("selected helper result");
    assert_eq!(
        cap, "member:true:2000:true",
        "membership, CPU cap, hard-cap flag"
    );
    // Only after the selected helper has observed its cap, independently release
    // the other helper to query whether it was adopted or capped.
    std::fs::write(&second_go, "go").expect("query unselected helper");
    let second_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < second_deadline && !second_marker.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let second_result = std::fs::read_to_string(&second_marker).expect("unselected helper result");
    assert!(
        second_result == "outside" || second_result.starts_with("member:true:"),
        "invalid unselected helper report: {second_result}"
    );
    assert_eq!(selected.wait().code(), Some(0));
    assert_eq!(unselected.wait().code(), Some(0));
    stderr_lines.extend(rx.try_iter());
    assert!(
        !stderr_lines.iter().any(|line| line.contains(&second_name)),
        "wrapper logged adoption of unselected helper: {stderr_lines:?}"
    );
    // Both helpers have reported; stop the long-running wrapped command and
    // drain stderr through wrapper exit before finalizing the negative proof.
    // SAFETY: retained process handle belongs to the live wrapper child; termination is intentional cleanup.
    unsafe { TerminateProcess(wrapper.handle.unwrap(), 0) };
    let _ = wrapper.wait();
    let _ = stderr_thread.join();
    stderr_lines.extend(rx.try_iter());
    assert!(
        !stderr_lines.iter().any(|line| line.contains(&second_name)),
        "wrapper logged adoption of unselected helper through exit: {stderr_lines:?}"
    );
}

/// `--adopt` with `-n` is refused at parse time: exit 2, one stderr line,
/// nothing started (joining past the active-process limit would kill the process).
pub fn procs_conflict_is_refused() {
    let output = Command::new(env!("CARGO_BIN_EXE_wnice"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["--adopt", "wnice-no-such-image", "-n", "4", "--"])
        .arg(probe_exe())
        .args(["exit", "0"])
        .output()
        .expect("run wnice");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(output.stdout.is_empty());
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.starts_with("wnice: --adopt cannot be combined with -n/--procs"),
        "{stderr}"
    );
}
