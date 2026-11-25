//! Detailed `--help` text for `wnice` and every single-purpose frontend.

const WNICE: &str = r#"wnice - run a command under Windows priority and resource limits

usage: wnice [options] [--] <command> [args...]

Every limit you pass is applied through ONE Windows Job Object that the
command joins before its first instruction, so it covers the whole process
tree (children, grandchildren, ...). The process tree is just
wnice.exe -> <command>: no cmd.exe or powershell.exe layer in between.
wnice waits for the command and exits with its exit code.

options:
  -p, --priority <class>   priority class for the whole tree:
                           idle | belownormal | normal | abovenormal | high | realtime
                           high/realtime (and abovenormal) need
                           SeIncreaseBasePriorityPrivilege (an elevated shell)
                           for a job-wide limit; without it wnice starts the
                           priority booster, which applies the class to every
                           process of the tree itself (see --boost)
  -c, --cpu <1-100>        hard CPU cap, percent of the whole machine's CPU
                           (throttles even when the machine is otherwise idle)
  -t, --threads <n>        run on the first <n> logical processors only
                           (1..min(logical processors, 63))
  -m, --memory <size>      ceiling on the tree's total committed memory:
                           <1-100>   percent of physical RAM, e.g. 50
                           <n>m      megabytes, e.g. 512m (decimals allowed: 1.5g)
                           <n>g      gigabytes, e.g. 2g
                           an allocation above it fails (it is not throttled)
  -n, --procs <n>          at most <n> processes alive at once, the command
                           itself included; a spawn above it fails
  -s, --timeout <seconds>  wall-clock deadline (2, 2.5, ...; sleep counts);
                           on expiry the whole tree is killed and wnice exits 124
  --wsl[=<distro>]         run the command inside a WSL2 distro (the one named,
                           or wsl.exe's default) under a systemd scope; the
                           other options become cgroup v2 limits in the guest
                           (see WSL MODE below)
  --boost                  skip the job priority limit; apply -p to every
                           process of the tree via the booster (job completion
                           port + SetPriorityClass per new process; usually
                           sub-millisecond, up to a few ms in spawn bursts);
                           requires -p/--priority
  -h, --help               this help
  -V, --version            print the version

Options end at the first argument that is not an option, or at '--'.
Everything after that is passed to the command exactly as given.

command lookup:
  A bare name is searched in the absolute PATH entries, in PATHEXT order
  (.com, .exe, .bat, .cmd). The current directory is NEVER searched.
  .bat/.cmd files run through 'cmd.exe /d /v:off /s /c'; on that path an
  argument containing '%' is refused (cmd.exe would expand it).
  cmd.exe builtins are not files: wrap them, e.g. wnice -p idle -- cmd /c dir

WSL mode (--wsl):
  The command runs inside the guest as its default user; root is only used
  to start it (systemd-run --scope supplies the limits, nice(1) sets the
  nice level, runuser(1) drops back to the user). Flag mapping:
    -c   CPUQuota = N percent of ALL guest vCPUs (nproc --all)
    -p   Nice 19/10/0/-5/-20 and CPUWeight 1/50/100/500/10000 for
         idle/belownormal/normal/abovenormal/high; realtime is REFUSED
         (the Linux nice/weight model has no realtime band)
    -m   MemoryMax: percent of the guest's RAM, or m/g byte sizes
    -t   AllowedCPUs = the first N guest vCPUs
    -n   TasksMax
    -s   RuntimeMaxSec plus a cgroup-wide SIGKILL at the deadline;
         wnice then exits 124 like Windows mode
  Requirements, all checked up front with a clear error: WSL2 (WSL1 is
  refused), systemd as PID 1 (enable [boot] systemd=true in /etc/wsl.conf),
  cgroup v2, systemd-run, runuser/nice. The command starts in the current
  Windows directory translated to /mnt/<drive>/...; if that directory is
  not visible in the guest, the run is refused. Arguments travel as a raw
  argv array through 'wsl.exe --exec' - no shell on either side - so the
  cmd.exe '%' refusal of Windows mode does not apply here. Startup runs a
  few WSL probes, so expect a second or two of overhead. The guest's total
  vCPUs/RAM are whatever .wslconfig (processors=/memory=) gives the VM;
  wnice never edits that file - it is the hard ceiling.

exit codes:
  <n>   the command's own exit code
  124   timed out (-s); the whole tree was killed
  127   command not found
  1     usage error, '%' refused, or a Windows API failure (message on stderr)

background processes:
  A process the command leaves running keeps the limits for its whole life,
  but is not killed when the command exits normally (only on timeout or if
  wnice itself is killed).

examples:
  wnice -p idle -- npm run build
      build in the background without slowing the desktop
  wnice -c 50 -t 4 -- cargo build --release
      at most half the CPU, on 4 logical processors
  wnice -m 2g -n 32 -- node server.js
      2 GB for the whole tree, at most 32 processes
  wnice -s 300 -- npm test
      kill the test run and everything it started after 5 minutes
  wnice -p belownormal -c 70 -m 50 -s 1800 -- npm run e2e
      everything at once, still one wrapper process and one job
  wnice --wsl -c 50 -s 300 -- ./bench.sh
      run a Linux benchmark capped at half the VM's CPU, killed after
      5 minutes (exit 124)

The single-purpose tools idle, belownormal, abovenormal, high, realtime,
capc, capt, capm, caps and capn are this binary with one limit each, and
admin (run elevated) and uiup (boost the desktop shell) ship with it; run any
of them with --help. cy and cx run 'claude --dangerously-skip-permissions'
and 'codex --dangerously-bypass-approvals-and-sandbox' with your arguments
(so 'cy --help' shows claude's own help).

inspection (--jobs):
  wnice --jobs       list every Job Object a wnice-family wrapper holds on this
                     machine right now: the owner process and its command line,
                     the CPU cap as a percent of ALL logical processors AND as a
                     percent of one processor (hard or soft), the memory and
                     process-count limits, the affinity mask, the priority class
                     and how many processes are active in it. Below that, the
                     busiest processes over a short sample, each with the jobs it
                     belongs to and the tag UNCAPPED when none of them caps the
                     CPU, or NOT-IN-WNICE-JOB. Read-only, needs no elevation; an
                     owner it cannot inspect (elevated, protected or another
                     user) is counted in the footer instead of being dropped.
                     -c is a percent of ALL logical processors, so a 10% cap is
                     160% of one processor on 16 of them.
                     Under the job table: 'job N command: ...' with the whole
                     command line of a row that was cut (may contain secrets),
                     and for a nested job 'job N machine share: at most X% all
                     / Y% one (...)', the product of the hard caps along its
                     proven parent chain ('?' when a link is unknown).
                     --jobs takes no command and no other option (exit 2); after
                     the command's first word it is the command's own argument.

no-servers (--no-servers):
  wnice --no-servers [limit options] -- <command> [args...]
                     disable MSBuild node reuse, Roslyn shared compilation,
                     Gradle daemons, and the sccache server for the launched
                     command when those variables are not already set. Existing
                     GRADLE_OPTS is preserved and gets -Dorg.gradle.daemon=false
                     unless it already contains org.gradle.daemon=. SCCACHE_SERVER_PORT
                     uses a currently free loopback port when unset (new on
                     every run: a sccache server it starts is not shared and
                     lives until sccache's idle timeout); failure to choose
                     one prints a note and does not stop the command.
                     This affects only servers started by this command, not
                     servers already running or tools delegating to them.
                     Cannot be combined with --wsl (exit 2).

audit (--audit, or WNICE_AUDIT=1 for legacy tools):
  Reports job accounting and same-user processes created after command start,
  outside the job, whose parent chain validates creation times back to the root.
  Suspects are not proven leaks. System-service starts (WMI, Task Scheduler,
  COM servers, UAC), work in reused servers already running, and broken parent
  chains are not visible. The command's exit code is passed through unchanged.
  cy, cx, admin and uiup do not accept this option; legacy tools use
  WNICE_AUDIT=1 (exact value), which --wsl ignores. --audit cannot be combined
  with --wsl (exit 2).

adopt (--adopt):
  wnice --adopt NAME[,NAME...] [limit options] -- <command> [args...]
                     while the command runs, put already running processes of
                     the same user whose image name is NAME (exact match,
                     case-insensitive, no wildcards, paths or leading '-';
                     NAME and NAME.exe are the same) into the command's job.
                     A name is an image name, so 'node' takes every node.exe
                     of the user. An adopted process keeps the limits until it
                     exits; --timeout (exit 124) or a killed wnice (Ctrl+C,
                     closed terminal) kills it with the job, a normal wrapper
                     exit does not. Do not adopt editors, IDEs or
                     anything that must outlive the command. A process in an
                     incompatible job may refuse (one line per process on
                     stderr). An adopted process counts toward -m (the job's
                     committed memory), so a large server may start failing
                     allocations. -n is refused with --adopt (exit 2): a
                     process that would pass the active-process limit is
                     terminated when it joins. Not available to the legacy
                     names or WNICE_AGENT_LIMITS; with --wsl: usage error
                     (exit 2).

agent limits (WNICE_AGENT_LIMITS, read only by cy and cx):
  set WNICE_AGENT_LIMITS=-p idle -c 10 -m 10g
                     cy and cx then run the agent under the job that
                     'wnice <those options> -- claude ...' (or codex) would
                     create: one job around the agent and everything it
                     starts, 'cy -c' sessions included. Allowed: -p, -c, -t,
                     -m, -n, -s with values, --boost and --no-servers. A
                     command, '--', --wsl, --jobs, --audit, --adopt, a help or
                     version flag, or an invalid value is refused: one line on
                     stderr, exit 1, nothing started. Unset or blank: the
                     agent's job has no limits, as before. wnice and the other
                     tools never read it."#;

const PRIORITY: &str = r#"{tool} - run a command at {CLASS} priority

usage: {tool} <command> [args...]

The priority is a Job Object limit, so it applies to the whole process tree,
including children that ask for another priority. Waits for the command and
exits with its exit code (127 if it is not found). Everything after the tool
name is the command, exactly as given.
{note}
Same as: wnice -p {tool} -- <command> [args...]

examples:
  {tool} npm run build
  {tool} cmd /c "build.bat && test.bat"
  {tool} capc 50 cargo test        (chained: {tool} priority plus a 50% CPU cap)"#;

const NOTE_LOW: &str = "\nUse it to keep long builds/tests from making the desktop sluggish.\n";
const NOTE_HIGH: &str = "\nHIGH/REALTIME as a job limit need SeIncreaseBasePriorityPrivilege\n(an elevated shell). Without it the job refuses (Win32 error 1314)\nand the priority booster applies HIGH to every process of the\ntree; a note on stderr says so.\n";
const NOTE_REALTIME: &str = "\nDANGEROUS: realtime outranks the system's own input, audio and UI threads; a\nbusy realtime process can freeze the whole desktop. Needs\nSeIncreaseBasePriorityPrivilege (an elevated shell); without it the job\nrefuses (Win32 error 1314) and the priority booster applies HIGH to every\nprocess of the tree; a note on stderr says so.\n";

const CAPC: &str = r#"capc - hard CPU cap for a whole process tree

usage: capc <percent 1-100> <command> [args...]

Caps the total CPU use of the command and everything it starts at
<percent> of the whole machine (Job Object hard cap). Unlike idle, it holds
even when nothing else wants the CPU. Exits with the command's exit code.

Same as: wnice -c <percent> -- <command> [args...]

examples:
  capc 50 npm run build
  capc 25 cargo build --release
  capc 50 capm 50 npm test     (chained: also cap memory at 50% of RAM)"#;

const CAPT: &str = r#"capt - pin a whole process tree to the first N logical processors

usage: capt <thread-count> <command> [args...]

<thread-count>: 1..min(logical processors, 63). These are logical processors
(hardware threads), not physical cores. Applies to the command and everything
it starts. Exits with the command's exit code.

Same as: wnice -t <thread-count> -- <command> [args...]

examples:
  capt 4 npm run build
  capt 2 cargo test"#;

const CAPM: &str = r#"capm - memory ceiling for a whole process tree

usage: capm <size> <command> [args...]

<size>:
  1-100   percent of total physical RAM, e.g. 50 (no '%' sign)
  <n>m    megabytes, e.g. 512m (decimals allowed: 0.5g)
  <n>g    gigabytes, e.g. 2g

Limits the total committed memory of the command and everything it starts.
An allocation above the limit FAILS (most programs then crash); it is not
throttled. Leave headroom for the runtime itself. Exits with the command's
exit code.

Same as: wnice -m <size> -- <command> [args...]

examples:
  capm 50 npm run build
  capm 512m node script.js
  capm 2g cargo build"#;

const CAPS: &str = r#"caps - wall-clock timeout for a whole process tree

usage: caps <seconds> <command> [args...]

<seconds>: positive whole or decimal number (2, 2.5), floored to whole
milliseconds; at most 4294967294 ms (~49.7 days). Time the machine spends
asleep counts. If the command is still running at the deadline, it and
everything it started are killed in one call and caps exits 124; otherwise
caps exits with the command's exit code. Sets no other limit.

Same as: wnice -s <seconds> -- <command> [args...]

examples:
  caps 300 npm test
  caps 2.5 ping -n 10 127.0.0.1    (exits 124)"#;

const CAPN: &str = r#"capn - limit how many processes a tree may run at once

usage: capn <count> <command> [args...]

<count>: 1..4294967295 simultaneously active processes, the command itself
included (capn 1 lets the command run but refuses its first child). A spawn
above the limit fails; nothing already running is affected. Exits with the
command's exit code.

Same as: wnice -n <count> -- <command> [args...]

examples:
  capn 10 npm run build
  capn 32 make -j 64"#;

const ADMIN: &str = r#"admin - run a command as Administrator and wait for it

usage: admin <command> [args...]

Already elevated: runs the command inline in this console.
Not elevated: shows the UAC prompt and runs it in its own window (Windows
gives an elevated process its own console). A real .exe is started directly
by its absolute path; .bat/.cmd files, cmd.exe builtins and unknown names go
through cmd.exe, where an argument containing '%' is refused. Exits with the
command's exit code, or 1 if elevation was cancelled.

examples:
  admin npm install -g some-package
  admin cmd /c "net stop spooler && net start spooler"

--keep-limits, opt-in: keep the limits of the job this admin is in.
usage: admin --keep-limits <command> [args...]
When this admin was started inside a limited Job Object (capc 10 admin
build.bat, say) and has to elevate, the UAC-elevated process is not a child
of admin and would run unlimited. With the flag, admin elevates wnice with
the caller's job limits instead of the command, so the elevated tree runs
with the same CPU rate cap, job memory limit, active-process limit, affinity
and priority class. The elevated wnice creates its own new job: it is a
separate pool, not the caller's job. A hard CPU cap with a fractional
percent is rounded down to a whole percent, but never below 1 (wnice -c
takes whole percents), and admin says so on stderr. Every limit that cannot be kept -
a CPU rate control that is not a hard cap, an affinity mask that is not a
contiguous run from CPU 0, an unknown priority class - gets its own stderr
line and is left out; the rest is still kept. Not kept by design: the
wall-clock timeout -s (it lives in the wrapper process, not in the job),
and the caps of outer jobs when the caller's job is nested; when two or more
wnice jobs around admin prove the nesting, one line on stderr says so (a CPU
cap there is a share of its parent's allowance, so the elevated tree may get
a larger share of the machine than the caller). In no job, or
with none of those limits, admin prints one note and behaves exactly like
plain admin. When the caller's job limits cannot be read, or the wnice.exe
next to admin.exe is not this executable's front, admin refuses to elevate
(exit 1) instead of running unlimited. Already elevated, the flag changes
nothing: the command inherits the job it already has."#;

const UIUP: &str = r#"uiup - raise the desktop shell processes to HIGH priority

usage: uiup

One-shot boost for explorer, dwm, sihost, ShellExperienceHost,
StartMenuExperienceHost, StartMenu, SearchApp and audiodg, to keep the
desktop responsive while heavy work runs. Asks for elevation (UAC) when
needed, because dwm and sihost run under another account. Prints a table of
old and new priorities. Apps started later are not affected."#;

/// Full help for `stem`, or `None` if it isn't one of the tools.
pub fn text(stem: &str) -> Option<String> {
    let priority = |class: &str, note: &str| {
        PRIORITY
            .replace("{tool}", stem)
            .replace("{CLASS}", class)
            .replace("{note}", note)
    };
    Some(match stem {
        "wnice" => WNICE.to_string(),
        "idle" => priority("IDLE", NOTE_LOW),
        "belownormal" => priority("BELOW_NORMAL", NOTE_LOW),
        "abovenormal" => priority("ABOVE_NORMAL", ""),
        "high" => priority("HIGH", NOTE_HIGH),
        "realtime" => priority("REALTIME", NOTE_REALTIME),
        "capc" => CAPC.to_string(),
        "capt" => CAPT.to_string(),
        "capm" => CAPM.to_string(),
        "caps" => CAPS.to_string(),
        "capn" => CAPN.to_string(),
        "admin" => ADMIN.to_string(),
        "uiup" => UIUP.to_string(),
        _ => return None,
    })
}

/// The line printed after a usage error.
pub fn hint(stem: &str) -> String {
    format!("Run '{stem} --help' for details and examples.")
}

/// True when the arguments ask a frontend for its help.
pub fn wants_help(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("-h" | "--help" | "/?")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 13] = [
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
        "admin",
        "uiup",
    ];

    #[test]
    fn every_tool_has_help_with_its_own_usage_and_examples() {
        for stem in ALL {
            let t = text(stem).unwrap();
            assert!(t.starts_with(&format!("{stem} - ")), "{stem}");
            assert!(t.contains(&format!("usage: {stem}")), "{stem}");
            assert!(!t.contains('{'), "{stem}: unexpanded placeholder");
            if stem != "uiup" {
                assert!(t.contains("examples:"), "{stem}");
            }
        }
        assert_eq!(text("normal"), None);
        assert_eq!(text("cy"), None, "cy/cx forward --help to the agent");
    }

    #[test]
    fn wnice_help_states_the_adopt_audit_and_servers_caveats() {
        let t = text("wnice").unwrap();
        for want in [
            "takes every node.exe",
            "closed terminal",
            "counts toward -m",
            "-n is refused with --adopt (exit 2)",
            "which --wsl ignores",
            "idle timeout",
            "after
                     the command's first word it is the command's own argument",
        ] {
            assert!(t.contains(want), "{want}");
        }
    }

    #[test]
    fn wnice_help_documents_every_option_and_exit_code() {
        let t = text("wnice").unwrap();
        for opt in [
            "--priority",
            "--cpu",
            "--threads",
            "--memory",
            "--procs",
            "--timeout",
            "--wsl",
            "--boost",
            "--jobs",
            "--no-servers",
            "--audit",
            "WNICE_AUDIT=1",
            "--adopt",
            "WNICE_AGENT_LIMITS",
            "--help",
            "--version",
        ] {
            assert!(t.contains(opt), "{opt}");
        }
        for code in ["124", "127"] {
            assert!(t.contains(code), "{code}");
        }
    }

    #[test]
    fn help_flag_only_as_first_argument() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(wants_help(&a(&["--help"])));
        assert!(wants_help(&a(&["-h", "x"])));
        assert!(wants_help(&a(&["/?"])));
        assert!(!wants_help(&a(&["cmd", "--help"])));
        assert!(!wants_help(&a(&[])));
    }

    #[test]
    fn boost_flag_documented_with_priority_requirement() {
        let t = text("wnice").unwrap();
        assert!(t.contains("--boost"));
        assert!(t.contains("requires -p"));
        assert!(!text("high").unwrap().contains("root process"));
        assert!(!text("realtime").unwrap().contains("root process"));
    }
}
