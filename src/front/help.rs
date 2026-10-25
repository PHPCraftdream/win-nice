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
                           high/realtime need SeIncreaseBasePriorityPrivilege
                           (elevated shell); without it only the root process
                           gets HIGH and a note is printed to stderr
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

The single-purpose tools idle, belownormal, abovenormal, high, realtime,
capc, capt, capm, caps and capn are this binary with one limit each, and
admin (run elevated) and uiup (boost the desktop shell) ship with it; run any
of them with --help. cy and cx run 'claude --dangerously-skip-permissions'
and 'codex --dangerously-bypass-approvals-and-sandbox' with your arguments
(so 'cy --help' shows claude's own help)."#;

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
const NOTE_HIGH: &str = "\nHIGH/REALTIME as a job limit need SeIncreaseBasePriorityPrivilege\n(an elevated shell). Without it only the root process gets HIGH and a note\nis printed to stderr.\n";
const NOTE_REALTIME: &str = "\nDANGEROUS: realtime outranks the system's own input, audio and UI threads; a\nbusy realtime process can freeze the whole desktop. Needs\nSeIncreaseBasePriorityPrivilege (an elevated shell); without it only the\nroot process gets HIGH and a note is printed to stderr.\n";

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
  admin cmd /c "net stop spooler && net start spooler""#;

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
    fn wnice_help_documents_every_option_and_exit_code() {
        let t = text("wnice").unwrap();
        for opt in [
            "--priority",
            "--cpu",
            "--threads",
            "--memory",
            "--procs",
            "--timeout",
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
}
