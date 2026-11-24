# win-nice

[![CI](https://github.com/PHPCraftdream/win-nice/actions/workflows/ci.yml/badge.svg)](https://github.com/PHPCraftdream/win-nice/actions/workflows/ci.yml)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/PHPCraftdream/win-nice/badge)](https://scorecard.dev/viewer/?uri=github.com/PHPCraftdream/win-nice)
[![npm version](https://img.shields.io/npm/v/win-nice.svg)](https://www.npmjs.com/package/win-nice)
[![npm downloads](https://img.shields.io/npm/dm/win-nice.svg)](https://www.npmjs.com/package/win-nice)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![platform](https://img.shields.io/badge/platform-win32-lightgrey.svg)](#requirements)
[![node](https://img.shields.io/node/v/win-nice.svg)](package.json)
[![dependencies](https://img.shields.io/badge/dependencies-0-brightgreen.svg)](package.json)

Windows equivalent of the unix `nice` / `renice` / `cpulimit` family, built for a
specific pain point: running several parallel AI coding agents (Claude Code, etc.)
on one Windows box without their builds/tests pegging every core and freezing the
desktop — mouse, keyboard, window dragging, audio, all of it.

Every tool is one native executable, `wnice.exe` (Rust, no runtime
dependencies), prebuilt in CI and shipped in the npm package — nothing is
compiled on your machine, and the tools need neither PowerShell nor Node.js to
run. npm is only the distribution/version channel and installer. The other
command names (`idle`, `capc`, ...) are copies of the same binary that pick
their behavior from the name they were started as.

## `wnice [options] [--] <command> [args...]`

The combined frontend: every limit at once, in **one** wrapper process and
**one** Windows Job Object, so the process tree under it is just
`wnice.exe -> your command` — no `cmd.exe`/`powershell.exe` layers.

```
wnice -p idle -c 50 -t 4 -m 50 -n 64 -s 600 -- npm run build
```

| Option | Same as | Meaning |
| --- | --- | --- |
| `-p, --priority <class>` | `idle` ... `realtime` | `idle`, `belownormal`, `normal`, `abovenormal`, `high` or `realtime` for the whole tree |
| `-c, --cpu <1-100>` | `capc` | hard CPU cap: N percent of all logical processors, one ceiling for the whole tree (see "What a CPU cap covers") |
| `-t, --threads <n>` | `capt` | affinity: the first `n` logical processors |
| `-m, --memory <size>` | `capm` | job memory ceiling (`50` = 50% of RAM, `512m`, `2g`) |
| `-n, --procs <n>` | `capn` | maximum simultaneously active processes |
| `-s, --timeout <seconds>` | `caps` | wall-clock deadline, exit 124 |
| `--adopt <NAME[,NAME...]>` | — | put already running same-user processes with these exact image names into the command's job (details below; not accepted by the legacy names or `WNICE_AGENT_LIMITS`) |
| `--boost` | — | with `-p`: skip the job priority limit and apply the priority to every process via the booster (see "Priority above NORMAL without elevation") |
| `--no-servers` | — | disable selected build-tool server/daemon reuse for the launched command (details below; also accepted by `WNICE_AGENT_LIMITS`) |
| `--jobs` | — | read-only report of the wnice jobs running right now (see "Looking at what is running"); must be the whole command line, so it takes no command and no other option |
| `-h` / `-V` | | help / version |

Options are parsed only up to the first non-option token or `--`; everything
after belongs to the wrapped command verbatim. `--no-servers` changes only the
environment passed to the command it starts: it sets `MSBUILDDISABLENODEREUSE=1`
and `UseSharedCompilation=false` when unset, appends
`-Dorg.gradle.daemon=false` to `GRADLE_OPTS` unless that option is already
present, and chooses a currently free loopback port for `SCCACHE_SERVER_PORT`
when unset. User-set values are preserved. A port-selection failure is a note,
not a launch failure. This reduces servers for commands wnice starts; it does
not stop servers already running, cover editors delegating to an existing
instance, or affect tools not listed above. It cannot be combined with `--wsl`.

`--adopt NAME[,NAME...]` puts already running processes with these image names
into this command's job while the command runs (about once a second wnice looks
for them), so its limits also cover a server that was started before the
wrapper. Only processes of the same user are considered. Names are exact and
case-insensitive; `NAME` and `NAME.exe` are the same, paths and wildcards are
rejected. An adopted process keeps the limits until it exits; it is killed with
the job on `--timeout` (exit 124), but not on a normal wrapper exit. Processes
it had started before adoption are not covered, processes it starts afterwards
are. Do not adopt editors, IDEs, or anything that must outlive the command. A
process that already lives in an incompatible job refuses: wnice prints one line
for that process and goes on. `--adopt` cannot be combined with `--wsl` and is
not accepted by the legacy names or by `WNICE_AGENT_LIMITS`.

The single-purpose tools below are the same machinery with one limit each,
kept for their short positional syntax.

Every tool has built-in help with examples: `wnice --help`, `capc --help`,
`idle -h`, `admin /?`, ... (`cy`/`cx` pass `--help` on to `claude`/`codex`).
A usage error prints the tool's usage line and exits `1`.

### What a CPU share actually guarantees

`-c <percent>` and `-p <class>` answer two different questions, and it is
worth being precise about both:

- **`-c` is a ceiling, not a reservation.** The tree never gets more than N
  percent of the machine's total CPU no matter what else runs - measured in
  the W1 no-UAC spike (`docs/plans/2026-11-15-w1-no-uac-spike.md`): a 10%
  hard cap held 9.7-9.9% of a 16-CPU machine while twelve HIGH-priority
  competitors burned the rest, and competitors' priority never moved the
  quota. But it grants nothing: an uncapped competitor can still use
  whatever CPU the capped tree does not.
- **`-p <class>` decides who wins when demand exceeds supply** - scheduler
  precedence within the cap. `wnice -p high -c 10` means "at most 10% of
  the machine, and above-normal precedence for whatever it does run", which
  is the common benchmarking shape.
- **What cannot be guaranteed: "nobody preempts us" in the strict sense.**
  Processes of other users, elevated processes, `REALTIME` workloads, and
  non-CPU resources (memory pressure, I/O saturation, GPU/driver stalls)
  can still cause stalls; nothing here reserves CPU against them, and
  wnice sets only a hard cap - there is no floor/reservation flag.
- **Nested Job Objects multiply percentages** (see the `capc` section and
  "Chaining these tools together" below) - check whether your shell is
  already job-wrapped before trusting a cap percentage.

### Looking at what is running: `wnice --jobs`

`wnice --jobs` (on its own) prints a read-only report of the Job Objects
wnice-family wrappers of the current user hold at that moment: one row per
job with its owner process and command line, its CPU cap as a percent of all
logical processors and of one, its memory and process limits, affinity mask,
priority class and active process count - then the busiest processes of a
short sample, each with the job chain it belongs to.

That chain answers "is this work really capped?": a process whose chain
contains no CPU cap is tagged `UNCAPPED`, and one in no wnice job at all is
`NOT-IN-WNICE-JOB`. Both labels speak only about the wnice jobs the report
could see: they do not rule out a cap from a job that is not a wnice job, or
from a wrapper the report could not inspect (the footer counts those). A value a query could not establish is shown as `?`,
never as "no limit"; while any wrapper could not be inspected, a verdict that
depends on it carries `- inventory incomplete` with a one-line legend, and
the footer counts the owners that were skipped and why. A nested job shows
`nested (job N): X% of parent`, because that number is a share of the
parent's allowance, not of the machine.

It changes nothing, needs no elevation (an owner it cannot inspect is counted,
not dropped), and is accepted only as the whole command line: `wnice --jobs`
with a command or any other option is a usage error - one stderr line, exit
code 2. After the first `--` it is an ordinary argument of the command:
`wnice -- cmd --jobs` runs `cmd` with that argument.

## What a CPU cap covers

### `-c N` is N percent of all logical processors

`-c N` (and `capc N`) is one hard cap for the whole job, measured against
**all** logical processors on the machine. On a 16-logical-processor
machine `-c 10` allows 1.6 cores' worth of CPU for the entire tree - 160
in "percent of one core" units, the unit many process monitors use
(performance counters, Process Explorer). A single process inside that
tree showing 160% is the cap working, not leaking: it is using 1.6 of the 16
processors. `wnice --jobs` prints both units for every job
(`10.0% all / 160.0% one hard`), so a number seen elsewhere can be matched
against the cap. WSL mode uses the same convention over the VM's vCPUs
(`-c 50` becomes `CPUQuota=800%` on 16 vCPUs; see "WSL mode").

`-m N` follows the same pattern for memory: N percent of the machine's total
physical RAM, one ceiling on the job's aggregate committed memory, not a
per-process figure.

### Each job has its own cap

There is no machine-wide pool that per-job percentages draw from. Thirty
separate `wnice -c 10` invocations on a 16-logical-processor machine are
thirty independent 1.6-core allowances - 48 cores' worth in total, and only
the machine's real capacity stops them. Nesting goes the other way: a job
started inside another capped job takes its rate as a share of the parent's
allowance, so the percentages multiply instead of setting a machine-wide
total (see "Chaining these tools together").

## Not a sandbox: what these limits do not cover

A Job Object is a resource limiter, not a security boundary. It constrains the
processes assigned to it and nothing else, and no option here changes that.
Work outside the job stays outside the cap:

- **Processes that were already running.** A job only holds what it starts, so
  a daemon or server reused from an earlier run does the work outside this
  invocation's cap (it may have limits of its own):
  MSBuild node reuse, `VBCSCompiler`, a Gradle daemon, an `sccache` or Bazel
  server, an editor that hands the build to an instance already open. (The
  reverse case - a daemon that *was* started under a cap and is still alive -
  is in the `capm` section: the limit then sticks to that daemon.)
- **Processes a service or broker starts for the command.** WMI's
  `Win32_Process.Create`, Task Scheduler, an out-of-process COM server and UAC
  elevation all create processes that are not children of the job, so they
  never join it and run outside its limits. `admin --keep-limits` is the one case with
  a fix: it carries the caller's limits through the UAC prompt.
- **Work inside a VM.** WSL2, Docker and Hyper-V guests are separate kernels:
  a host-side job cannot limit their processes (WSL2 shows up host-side as
  `vmmem`, which an un-elevated process cannot even open). Use `wnice --wsl`,
  the VM's own `.wslconfig` ceiling (`processors=`, `memory=`) or the
  container runtime's own CPU/memory options instead.
- **Work other processes do on the command's behalf.** Antivirus scans, the
  search indexer and shared build services run in their own processes, and
  kernel/driver work (interrupts, DPCs) belongs to no process in the job;
  none of that time is charged to the job, so the machine can be busy while
  every job sits under its cap.
- **A process of the same user that lifts the limits deliberately.** A job is
  an object with a handle: a process that can obtain that handle with enough
  access can change that job's limits (it cannot leave the job), and
  `wnice --jobs` duplicates those handles for reading, with query-only rights.
  Ordinary children cannot escape: a descendant asking for
  `CREATE_BREAKAWAY_FROM_JOB` fails to launch, a nested job cannot loosen the
  outer one, and the wrapper's job handle is not inherited (see `capc`). That
  is robustness against ordinary programs, not a boundary against a user who
  means it.

What exists today to close the gaps that can be closed: `wnice --jobs` to see
which processes are actually capped, `admin --keep-limits` to carry limits
across UAC, `WNICE_AGENT_LIMITS` to cap every `cy`/`cx` agent, `wnice --wsl`
plus `.wslconfig`/container limits for VM work, and starting a reused daemon
under the same wrapper (or turning reuse off, as shown under `capm`) when its
work must be counted.

## Argument handling

Every tool tries to launch the wrapped command directly first (`CreateProcess`,
no shell involved at all) and only goes through `cmd.exe /c` when the target is
a `.bat`/`.cmd` file, which genuinely needs it.

**Direct-launch path** (the common case: the target is a real `.exe`): arguments
are immune to cmd.exe's special characters entirely — `&`, `|`, `<`, `>`, `^`,
`%`, quotes, spaces, empty strings all pass through exactly as given, standard
MSVCRT/`CommandLineToArgvW` quoting. cmd.exe is never invoked on this path, so
there's nothing to expand.

**`cmd.exe /c` fallback path** (only reached for `.bat`/`.cmd` targets): `&|<>^`/quotes/spaces/empty strings are still fully
protected. A literal `%` used to be able to trigger environment-variable
expansion here — cmd.exe pairs up `%` characters across the *entire* command
line, even across separate arguments, and there's no reliable per-character
escape for that at the `cmd.exe /c` level. Rather than risk that, this path now
**fails closed**: if any argument contains `%`, the tool refuses to run,
prints an error to stderr, and exits with code `1` — the command never reaches
cmd.exe. This is a change from just checking for the character; it's a hard
reject, not a best-effort escape.

Bare command names are resolved by the tool itself, never by Windows: only
absolute `PATH` entries are searched, in `PATHEXT` order (`.com`, `.exe`,
`.bat`, `.cmd`), and **the current directory is never searched** — an
`npm.exe` dropped into the working directory cannot hijack `capc 50 npm ...`.
A name found nowhere fails with exit code `127`. cmd.exe builtins (`dir`,
`echo`, ...) are not files, so wrap them explicitly: `idle cmd /c dir`.

The tools are real `.exe` files, so every shell — PowerShell, pwsh, cmd.exe,
Node's `child_process`, Git Bash — starts them the same way and their
arguments arrive unchanged. Two things your *calling shell* still does before
the tool ever runs: cmd.exe expands `%VAR%` on its own command line, and
Windows PowerShell 5.1 mis-quotes an argument that ends in a backslash
(`'C:\dir with space\'`) when passing it to any native program.

For Git Bash the installer also writes an extensionless `name` shim (a
`#!/bin/sh` script) next to each `name.exe`; it sets
`MSYS2_ARG_CONV_EXCL='*'` so slash-style switches (`/c`, `/d`) and Windows
paths reach the tool untouched, then `exec`s the `.exe`. Known limitation:
that variable is inherited by the wrapped command's whole process tree, so an
MSYS program spawned *inside* it (an inner `bash`/`sh`, a `#!/bin/sh` git
hook) stops converting POSIX-style paths in its own children's arguments —
e.g. `caps 600 bash -c 'node /c/proj/run.js'` fails with
`Cannot find module`. Call the `.exe` directly (`caps.exe ...`) when that
matters.

The npm package itself installs on Windows only (a WSL-side npm refuses it,
`"os": ["win32"]`) — install through Windows Node/npm. Launching commands
*into* a WSL2 distro from Windows is a different matter and is supported by
`wnice --wsl`; see "WSL mode" below.

## WSL mode

`wnice --wsl[=<distro>] [options] -- <command> [args...]` runs the command
inside a WSL2 distro. Why a separate mode: Windows mode's Job Object cannot
limit Linux processes — they run inside the WSL2 VM, invisible to Job
Objects (the VM shows up host-side as `vmmem`, which an un-elevated process
cannot even open - `OpenProcess` is denied - so a host-side limit on it is
impossible, not just unimplemented; measured in the same spike). This mode
translates the same wnice flags into systemd/cgroup-v2
constraints *inside the guest* instead: the command runs as the distro's
default user under a transient systemd scope started by root (root is only
used to start it — `systemd-run --scope` supplies the limits, `nice -n` sets
the nice level, `runuser -u` drops back to the user):

```
wnice --wsl -p idle -c 50 -t 4 -m 50 -n 64 -s 600 -- npm run build
```

| Option | Becomes in the guest |
| --- | --- |
| `-c, --cpu <1-100>` | `CPUQuota=<N*vcpu>%` — N percent of **all** guest vCPUs (from `nproc --all`), so 50 on a 16-vCPU VM is `CPUQuota=800%` |
| `-p, --priority <class>` | nice 19/10/0/-5/-20 plus CPUWeight 1/50/100/500/10000 for idle/belownormal/normal/abovenormal/high; `realtime` is **refused** (exit 1) — the Linux nice/cgroup-weight model has no realtime band, and unlike Windows mode there is no silent downgrade |
| `-m, --memory <size>` | `MemoryMax` — percent of the **guest's** RAM (from `/proc/meminfo`), or `512m`/`2g` byte sizes |
| `-t, --threads <n>` | `AllowedCPUs=0-<n-1>` — the first N guest vCPUs; N greater than the guest's vCPU count is refused |
| `-n, --procs <n>` | `TasksMax=<n>` |
| `-s, --timeout <seconds>` | `RuntimeMaxSec=<N>ms` plus `KillSignal=SIGKILL`; wnice exits 124 (see "Timeout" below) |

Exit codes follow the Windows-mode conventions: the command's own exit code,
`124` on timeout, `1` for usage/precheck errors.

### Requirements (each checked up front)

Every precheck failure prints a clear stderr message and exits `1` — no flag
is ever silently dropped:

- `wsl.exe` present (looked up in `System32`).
- The distro exists (matched exactly, then case-insensitively; a miss lists
  the available names) and is **WSL2** — WSL1 is refused.
- systemd as PID 1 (otherwise: set `[boot] systemd=true` in
  `/etc/wsl.conf` and restart WSL).
- cgroup v2 (`/sys/fs/cgroup/cgroup.controllers` non-empty).
- `systemd-run` available.
- The distro's default user is resolvable (`id -un`).
- A dry-run of the exact launch chain (`nice -n 1 runuser -u <user> --
  /bin/true`) succeeds.
- The current Windows directory, translated to `/mnt/<drive>/...`, exists in
  the guest (`test -d`).

### Working directory

The command starts in the current Windows directory translated to
`/mnt/<drive>/...` (drive letter lowercased, backslashes to slashes), passed
to `wsl.exe` via `--cd`. UNC paths are refused (there is no verifiable
guest mapping for them), and if the translated directory is not visible in
the guest (drive not mounted), the run is refused.

### Launch shape

The exact command wnice builds — useful to know what to look for in
`systemctl` and process listings:

```
wsl.exe [-d <distro>] -u root --cd /mnt/<drive>/... --exec systemd-run \
  --scope --quiet --collect --unit=wnice-<pid>-<ms> \
  -p CPUQuota=... -p CPUWeight=... -p MemoryMax=... -p AllowedCPUs=... \
  -p TasksMax=... -p RuntimeMaxSec=... -p KillSignal=SIGKILL \
  -- nice -n <nice> runuser -u <user> -- <command> [args...]
```

(Each `-p` line above appears only when the corresponding flag was given.)

### Argument safety

The whole chain travels as an argv **array**: `wsl.exe --exec` on the
Windows side and systemd-run/runuser `--` argv passthrough in the guest —
there is **no shell on either side**. `;`, `&`, `|`, `$()`, backticks,
quotes, spaces, `%`, empty strings and non-ASCII all arrive as verbatim
separate arguments, so the Windows-mode cmd.exe `%` refusal does not apply
here. One honest caveat: wsl.exe's argument relay may drop empty-string
arguments (pending verification on real WSL; there is a gated integration
test for it).

### Timeout

`-s` becomes `RuntimeMaxSec` with `KillSignal=SIGKILL`: at the deadline
systemd SIGKILLs the scope's whole cgroup — every process in it, including
`setsid`'d daemons — which is stronger than `timeout(1)`'s process-group
kill. wnice maps the deadline to exit `124` by wall clock on the Windows
side, and the same race class as Windows `caps` applies: a command that
self-exits within roughly the relay overhead (hundreds of ms) of the
deadline can be reported as 124.

If wnice itself is killed, the scope keeps running under the guest's
systemd with its limits still attached — the same contract as the
Windows-mode daemon behavior. `--collect` garbage-collects finished scopes;
a still-running leftover is cleaned up with `systemctl stop wnice-*.scope`
(or `systemctl reset-failed` for dead ones).

### Startup overhead

Launching runs 7-9 short `wsl.exe` probes first — a second or two, more on
the first call, which may boot the VM. Each probe is capped at 120 s, so a
wedged VM cannot hang wnice forever.

### `.wslconfig` is the hard ceiling

The guest VM's total vCPUs/RAM come from `.wslconfig` (`processors=` and
`memory=`); those values are the ceiling every `-c`/`-t`/`-m` percent is
computed against (e.g. `wnice --wsl -c 50 -t 16 -- ...` on a
`processors=16`, `memory=23GB` VM means `CPUQuota=800%` of 16 vCPUs and the
first 16 guest vCPUs). wnice never reads or edits `.wslconfig` — changing
the VM's size is your manual step. `processors=` is a vCPU *count*, not a
percentage - it limits parallelism, not duty cycle - and there is no
documented host-side CPU-percentage knob for WSL2 (`memory=` exists for RAM),
which is the other reason the percentage limits live in the guest.

### Checking it by hand

- `wnice --wsl -s 3 -- sleep 30` — exits 124 in about 3 seconds.
- `wnice --wsl -c 50 -- bash -c "while :; do :; done"` — then, in another
  terminal: `wsl -u root systemctl status wnice-*.scope` shows the scope
  with `CPUQuota=` (half the VM's total vCPU capacity) while it runs.
- `wnice --wsl -- /bin/true` — exits 0.
- `wnice --wsl=missing -- /bin/true` — exits 1 with a
  `WSL distribution 'missing' not found (available: ...)` message.

## Tools

### `idle <command> [args...]`
Runs `command` at `IDLE_PRIORITY_CLASS`, waits for it to exit, propagates its
exit code. The priority is a Job Object limit (`JOB_OBJECT_LIMIT_PRIORITY_CLASS`),
so it holds for the whole process tree — including children that ask for a
different priority of their own.

### `belownormal <command> [args...]`
Same as `idle`, at `BELOW_NORMAL_PRIORITY_CLASS` — a lighter touch than idle.

### `abovenormal <command> [args...]`
Same as `idle`, at `ABOVE_NORMAL_PRIORITY_CLASS` — also for the whole tree
(confirmed empirically for children and grandchildren).

### `high <command> [args...]`
Same shape, at `HIGH_PRIORITY_CLASS`. Windows only accepts `HIGH` as a job
limit from a process holding `SeIncreaseBasePriorityPrivilege` (elevated
processes have it). Without it the job refuses (Win32 error 1314) and `high`
starts the booster instead, which applies `HIGH` to every process of the tree
— typically sub-millisecond per process — and a `wnice: note: ...` line on
stderr says so (see "Priority above NORMAL without elevation" below).

### `realtime <command> [args...]`
Same shape, at `REALTIME_PRIORITY_CLASS`. **Dangerous, use with caution:**
`REALTIME` outranks the OS's own input/audio/UI threads — a busy realtime-priority
process can make the entire desktop (mouse, keyboard, everything) stop responding,
which is the exact failure mode this project otherwise exists to prevent. It needs
the same privilege as `high`; without it the job refuses (error 1314) and the
booster applies `HIGH` — `REALTIME` still downgrades — to every process of the
tree, typically sub-millisecond, with the same stderr note (see "Priority
above NORMAL without elevation" below).

### Priority above NORMAL without elevation (`--boost`)

A job priority limit above `NORMAL` needs `SeIncreaseBasePriorityPrivilege`
(Win32 error 1314 without it), and Windows children do not inherit `HIGH` by
default — they inherit `NORMAL` (only `IDLE`/`BELOW_NORMAL` propagate).
Without elevation there is therefore no way to hold an above-normal priority
tree-wide through a job limit alone.

So when the job refuses the tree-wide limit (error 1314), `wnice` starts the
**priority booster**: the job is associated with an I/O completion port, a
thread watches for the job's new-process notifications
(`JOB_OBJECT_MSG_NEW_PROCESS`), and for every process the job creates it
opens the process and calls `SetPriorityClass` with the chosen class. The
root process gets the class through its `CreateProcess` flags; `realtime`
still downgrades to `HIGH`; no elevation is needed, and the booster behaves
the same elevated or not. Per-process booster errors (a process that already
exited, an access denial) are ignored and never change the exit code, and
the other job limits are unaffected — the CPU cap stays the same hard-cap
limit, and memory, process-count, affinity and timeout work exactly as
before.

The boundaries, honestly: a boost usually lands in well under a millisecond
(median ~0.004 ms measured), but a spawn burst stretches it — ~35 ms was
observed when ~8 processes spawned at once; children run at `NORMAL` until
their boost lands; a leaf that lives ~5 ms or less can exit before it is
ever boosted; and a process that exits before the booster reaches it is
simply skipped.

`--boost` skips the job priority limit entirely and always uses the booster —
deterministic on machines where the job limit would have succeeded anyway
(elevated shells, CI runners), and it's what the test suite uses. It requires
`-p/--priority`, and no job priority limit is ever set in that mode.
Priorities below `normal` (`idle`/`belownormal`) keep using the plain job
limit, which needs no privilege there; and when the booster setup itself
fails, the old root-only fallback remains — only `--boost` turns a setup
failure into a hard error (exit 1).

When the booster takes over, stderr explains it:
`wnice: note: high priority applies to every process in the tree via the
booster (job completion port + SetPriorityClass, not a job limit); boost is
usually sub-millisecond (up to a few ms in spawn bursts), so very short-lived
children may exit before it lands`.

### `capc <percent> <command> [args...]`
Hard CPU quota (1-100) for the whole process tree, enforced by a Windows Job
Object (`JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`, hard cap). Unlike `idle`/
`belownormal`, this is a real ceiling on total CPU%, not just a scheduling
priority — it holds even when nothing else on the machine is contending for CPU.

The cap covers the whole subtree from its very first instruction: the wrapped
command is created suspended, assigned to the Job Object, and only then resumed
— there's no window where it runs uncapped. Every ordinary descendant created
via `CreateProcess` (and their children, recursively) automatically joins the
same job; this is standard Job Object behavior on any supported Windows
version, not something specific to newer ones. The known ways out: a
descendant explicitly requesting `CREATE_BREAKAWAY_FROM_JOB` (and since the job
here never sets a breakaway-allowed flag, that fails closed — the child just
fails to launch rather than silently escaping the cap), or a process brought
up through an external broker/service that never goes through the wrapped
tree's own `CreateProcess` calls (Microsoft documents, for example, that a
process started via WMI's `Win32_Process.Create` doesn't join the caller's
job).

Windows 8+ specifically matters if something inside the wrapped command creates
*its own* Job Object (some tools do, e.g. Chromium-based ones — or these tools
themselves, when chained together, see "Chaining these tools together" below):
before Windows 8 a process could belong to only one job at a time, so that
inner `AssignProcessToJobObject` call would fail. Windows 8+ allows nested
jobs, so it succeeds instead, and both jobs' limits apply — but *how* they
combine depends on the limit type, not one universal "smaller wins" rule. For
CPU rate control specifically, a nested job's rate is relative to what its
parent already lets through, so equal caps **multiply**: `capc 50 capc 50 ...`
yields roughly 25% of total system CPU, not 50% (see "Chaining these tools
together" below for the full picture across limit types). The same
multiplication applies when the tool itself runs *inside* an outer Job Object
that already has a CPU rate limit — a CI agent or wrapper shell that
job-wraps everything it starts: measured example, a 10% cap inside a parent
job capped at 10% yields ~0.9% of the machine (10% of 10%), so check whether
your shell is already job-wrapped before trusting a cap percentage.
Workarounds: set the percentage at the level that owns the outer job (the
percentages multiply, so an inner cap just compounds the restriction), or
launch these tools from a shell that is not itself inside a
CPU-rate-limited job.

Blocks until the command exits, propagates its exit code.

```
capc 50 npm run build
```

### `capt <thread-count> <command> [args...]`
Short for **cap threads**. Restricts the whole process tree to the first
`<thread-count>` logical processors via Windows process affinity
(`JOBOBJECT_BASIC_LIMIT_INFORMATION`, `JOB_OBJECT_LIMIT_AFFINITY`) — same
suspend-then-assign-then-resume Job Object mechanism as `capc`, so the same
"covers the whole subtree from the first instruction" and "breakaway fails
closed" guarantees apply.

Deliberately *threads*, not *cores*, in both the name and the semantics:
Windows affinity masks address logical processors (hardware threads), not
physical cores. On a machine with Hyper-Threading/SMT, `capt 4` pins to 4
*logical processors* — depending on which ones, that could be 2 fully-used
physical cores or 4 half-used ones; the affinity API has no concept of "whole
core" grouping on its own. `<thread-count>` must be between 1 and the number
of logical processors on the machine (capped at 63 — a single affinity mask
can't address more).

```
capt 4 npm run build
```

### `capm <size> <command> [args...]`
Hard memory ceiling for the whole process tree, enforced by a Windows Job
Object (`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, `JOB_OBJECT_LIMIT_JOB_MEMORY`)
— same suspend-then-assign-then-resume mechanism as `capc`/`capt`, so the same
"covers the whole subtree from the first instruction" and "breakaway fails
closed" guarantees apply. Caps the whole job's *aggregate* committed memory,
not any single process — like `capc`'s CPU% and `capt`'s affinity, it's one
ceiling for the whole tree, not a per-process limit.

`<size>` accepts three forms:

| Form | Meaning |
| --- | --- |
| `50` (bare integer, `1`-`100`) | percent of total physical RAM (`GlobalMemoryStatusEx`), not of whatever's currently free — the cap means the same thing regardless of what else is running on the machine at invocation time. Same convention as `capc`'s own `<percent 1-100>` — deliberately no `%` character; see "Chaining these tools together" below for why |
| `512m` / `512M` | megabytes |
| `2g` / `2G` | gigabytes |

```
capm 50 npm run build
capm 512m npm run build
capm 2g npm run build
```

**Unlike `capc`, exceeding the limit doesn't throttle — it fails the
allocation.** A CPU cap just makes things slower; a memory cap that's
exceeded causes the *allocation call itself* to fail (`VirtualAlloc`-family
APIs return an error, .NET throws `OutOfMemoryException`) rather than the OS
gracefully degrading anything. Most programs don't handle allocation failure
cleanly, so in practice this usually looks like a crash. Set it too low and
even the wrapped program's own runtime can fail to start (confirmed: capping
Windows PowerShell 5.1 itself at 30 MB crashes it with
`StackOverflowException` before it can run anything) — leave headroom above
whatever interpreter/runtime the wrapped command needs just to start, on top
of what your actual workload needs.

**A `capc`/`capt`/`capm`/`capn` limit sticks to any daemon the wrapped command leaves
running**, for that daemon's entire lifetime — not just for the wrapped
command's own run. Job Object membership is permanent for a process once
assigned (short of an explicit, disallowed breakaway); a background process
the command spawns and detaches from is still in the same job, still capped,
for as long as it stays alive. This bites build tools that reuse a persistent
process across invocations to skip cold-start cost: `dotnet build`'s
`VBCSCompiler`/MSBuild node reuse, a Gradle daemon, file-watcher processes
left running by `npm run watch`-style scripts. A follow-up **uncapped**
`dotnet build` (or `gradle`) can end up running inside the *previous* `capc`
call's Job Object without a new `capc`/`capt`/`capm` invocation of its own,
capped because a stale daemon from an earlier call is doing the work. Either
don't leave the daemon running across a call whose limit shouldn't persist
(`dotnet build -p:UseSharedCompilation=false`, `gradle --no-daemon`), or
accept that the limit is now effectively attached to the daemon until it's
killed.

### `caps <seconds> <command> [args...]`
Runs the command with a wall-clock deadline: if it's still running when
`<seconds>` have passed, it is force-killed and `caps` exits with code **124** (the unix
`timeout(1)` convention), printing
`wnice: timed out after <seconds>s - job and every process in it were force-killed`
to stderr. If the command finishes in time, `caps` propagates its exit code
exactly like every other launcher in this family.

The deadline is genuinely wall-clock: `caps` computes it once as an absolute
UTC timestamp and arms a one-shot waitable timer with that absolute due time
(`SetWaitableTimer`), then waits on the timer and the wrapped process together
(`WaitForMultipleObjects`). An absolute timer due time is a property of the
wall clock, not of an in-progress wait, so time the machine spends
asleep/suspended counts against it: if the deadline passes during sleep, the
timer is already signaled when the machine wakes, and `caps` fires immediately
instead of waiting out any leftover countdown (a relative `WaitForSingleObject`
wait does not count sleep time on Windows 8+ and keeps counting its pre-sleep
remainder after the wake). If the two signals race - a child exiting right
around the deadline - `caps` asks the kernel for the process's real exit time
(`GetProcessTimes`) and only accepts it as on-time if it actually finished at
or before the deadline. Because that due time is a value on the system clock,
a manual or service-driven system-clock adjustment during the wait moves it
with the clock: the actual wait can come out shorter or longer than the
requested `<seconds>` (sleep/suspend still counts correctly - only clock
adjustments shift the deadline).

The same "covers the whole subtree from the first instruction" guarantee
applies: same suspend-then-assign-then-resume Job Object mechanism as
`capc`/`capt`/`capm`, so at expiry a single `TerminateJobObject` kernel call
kills the direct child *and every descendant it spawned* — no process-tree
walking, no window where a grandchild outlives the child. The job carries only
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, which doubles as the backstop if the
`caps` wrapper itself is killed non-cooperatively (`taskkill` without `/T`, a
crash): Windows itself then terminates the whole job at that moment. That
backstop is released before a normal, inside-the-deadline exit, so a
background daemon the wrapped command legitimately left running survives -
`caps` only forces a whole-tree kill on its own timeout or on the wrapper's
own non-cooperative death, never on an ordinary successful exit.

`<seconds>` accepts a positive whole or decimal number (`2`, `2.5`), converted
to whole milliseconds (floored; minimum 1 ms). The maximum is 4294967294 ms
(~49.7 days) — a deliberate usage ceiling, not a Win32 API limit: the deadline
is an absolute 64-bit `FILETIME` armed into a waitable timer, and the
`WaitForMultipleObjects` wait itself always passes `INFINITE` (the timer is
what bounds it). Anything larger is rejected as a usage error (exit 1), never
silently truncated into a
different deadline.

**`caps` sets no resource limit of any kind** — no CPU, memory, priority, or
affinity limit. It is not a quota tool: `capc`/`capt`/`capm` deliberately
impose no timeout (a quota tool shouldn't unilaterally decide a legitimate long
build is stuck — same precedent as `nice`/`cpulimit`); `caps` is the
complement, for when *you* have already decided that N seconds is the limit
(see the system-clock caveat above).

```
caps 300 npm test
```

### `capn <count> <command> [args...]`
Runs the command under a hard ceiling on the number of **simultaneously active
processes** in its whole process tree (`JOBOBJECT_BASIC_LIMIT_INFORMATION`,
`JOB_OBJECT_LIMIT_ACTIVE_PROCESS`) — same suspend-then-assign-then-resume Job
Object mechanism as `capc`/`capt`/`capm`, so the same "covers the whole subtree
from the first instruction" and "breakaway fails closed" guarantees apply.

The count **includes the directly wrapped process itself**: it is assigned to
the still-empty job before it can spawn anything, so `capn 1 <command>` runs
the command but the instant it tries to spawn any child — including
children started by the command — that spawn attempt fails. Confirmed empirically, not just from docs: under `capn 1`, a wrapped
PowerShell's `Start-Process` fails with "Not enough quota is available to
process this command." and the parent keeps running and exits 0.

Unlike a `capc` throttle, exceeding the limit doesn't degrade anything — the
offending spawn attempt itself is what fails (closer to `capm`'s failed
allocation): nothing is killed, nothing already running is affected, and the
wrapped process sees an ordinary process-creation failure from its own spawn
call. A command that stays within the budget is completely unaffected —
`capn 3` wrapping a parent that spawns 2 children runs exactly like an
uncapped one (also confirmed empirically).

`<count>` is a positive whole number, minimum 1, maximum 4294967295 — the
`ActiveProcessLimit` struct field is a uint32, so anything larger (or
negative, or non-numeric) is a usage error (exit 1), never a silently
truncated limit.

```
capn 10 npm run build
```

## Chaining these tools together

These tools can be stacked by passing one as another's `<command>`:

```
capm 50 capc 50 idle npm run build
```

Each wrapper wraps everything after its own arguments, so the outermost
wrapper is whichever one you type first, and each one adds its own wrapper
process and its own nested Job Object. **Prefer one `wnice` call instead**
(`wnice -m 50 -c 50 -p idle -- npm run build`): one process, one job, and
none of the nesting rules below apply. When you do chain, the tool names
resolve through `PATH` like any other program, so the install directory must
be on `PATH`.

**Nested Job Object limits do not follow one universal "smaller wins" rule —
each limit type combines differently:**

- **CPU (`capc`)**: a nested job's CPU rate is relative to what its parent
  already lets through, so equal caps *multiply* rather than take the
  minimum — `capc 50 capc 50 ...` yields roughly 25% of total system CPU, not
  50%. This is documented Windows behavior for
  [`JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information),
  not a bug here. The same goes for an outer job you didn't create yourself:
  running these tools from a shell or agent that is already inside a
  CPU-rate-limited job makes the tool's own rate relative to that parent's
  (measured: 10% inside a 10% parent ≈ 0.9% of the machine).
- **Memory (`capm`)**: each ceiling applies independently to its own
  accounting scope, and those scopes are *not* the same size — a job's
  committed-memory accounting includes its own processes **plus every child
  job's committed memory**, while a child job's own accounting doesn't see
  the outer wrapper's process at all
  ([Nested Jobs — Resource Accounting](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs)).
  So `capm 500m capm 400m ...` is *not* guaranteed to behave like a plain
  `min(500m, 400m) = 400m` ceiling on the innermost workload — the outer
  500m job can run out of budget first purely from its own wrapper
  process's memory use, on top of whatever the inner 400m job is using.
  Don't rely on nested `capm` ceilings combining to an exact number; treat
  the outer value as an upper bound that can bind earlier than expected.
- **Priority (`idle`/`belownormal`/`abovenormal`/`high`/`realtime`)**: a
  Job Object priority-class limit — with nested jobs the outermost job that
  sets one decides, so `idle high <cmd>` still runs the command at `Idle`.
- **Affinity (`capt`)**: nested affinity is an *effective-limits* chain — a
  child job's mask can be as tight as it wants but is clamped to whatever
  the parent already allows, never wider
  ([Nested Jobs — Job Limits](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs)).
  Confirmed empirically: `capt 2 capt 3 ...` (inner asking for *more*
  processors than the outer allows) still comes back pinned to the outer's
  2, not the inner's 3 — no error, just silently clamped to the tighter mask.
- **Process count (`capn`)**: each job enforces its own `ActiveProcessLimit`
  independently against its own simultaneously-active count — a spawn has to
  fit under *every* job in the chain at once, and an outer job's count
  includes the inner wrapper process itself plus everything beneath it. Not a
  "silently clamped to the tighter limit" rule like affinity, and not a plain
  `min()`: a tighter outer job can prevent an inner wrapper from starting its
  target; leave room for the wrapper chain and the target's actual child
  processes. In the other direction,
  `capn 5 capn 1 <cmd-that-spawns>` runs the command but its child spawn is
  refused by the *inner* limit while the outer still has room.
- **Timeout (`caps`)**: not a Job Object limit being combined at all — each
  `caps` enforces its own deadline on its direct child. The innermost `caps`
  wrapping the eventual work fires at its own deadline and the outer one
  propagates the inner's 124 exit code like any other tool's. If the *outer*
  deadline fires first, its `TerminateJobObject` kills the whole subtree
  including the inner `caps` wrapper — and everything the inner wrapper had
  assigned to its own job dies with it via that inner job's own
  `KILL_ON_JOB_CLOSE` flag (the same cascade documented above). Either way the
  caller sees 124.

Test any combination you actually plan to depend on; don't assume "more
wrappers, more restrictive" holds uniformly across limit types.

### `uiup`
One-shot priority boost (`HIGH`) for the live shell/UI/audio processes,
intended to improve desktop responsiveness while heavy background work runs
underneath: `explorer`, `dwm`, `sihost`, `ShellExperienceHost`,
`StartMenuExperienceHost`, `StartMenu`, `SearchApp`, `audiodg`. This is a
best-effort one-shot tweak, not a guarantee — memory pressure, I/O
saturation, driver/GPU stalls, or a realtime-priority workload elsewhere can
still make the desktop stutter regardless.

Self-elevates via UAC — `dwm`/`sihost` run under a separate account
(`Window Manager\DWM-1`), so raising their priority needs admin rights.

The boost does **not** propagate to apps you launch from Explorer afterwards:
`HIGH` priority isn't inherited by child processes under Windows' default
`CreateProcess` rules (only `IDLE`/`BELOW_NORMAL` are). Confirmed empirically —
see the project history for the test.

### `admin <command> [args...]`
Runs `command` elevated (as Administrator), waits for it to exit, propagates
its exit code — a blocking elevation wrapper with the same wait-and-propagate
semantics as the other wrappers here, but it does not set a priority class
(unlike `idle`). If it's already elevated, runs inline sharing the current
console, with the full direct-launch argument safety described above.

If the calling shell isn't already elevated, triggers the standard UAC consent
prompt (via `ShellExecute`, always opening its own console window, incompatible
with sharing the caller's). Unlike a plain `ShellExecute("cmd.exe", "/c ...")`,
this branch also tries a direct launch first: a `.bat`/`.cmd` target still needs
the `cmd.exe /c` fallback (no elevation-capable equivalent of `CreateProcess`'s
own `.bat`/`.cmd` auto-relaunch), but any other target is elevated directly
by its resolved absolute path, never touching cmd.exe — same as the already-elevated branch, a
literal `%` in any argument is only refused when the `.bat`/`.cmd` fallback is
actually needed, since a direct launch is never exposed to `%` expansion at all.

```
admin npm install -g some-package
```

**`admin --keep-limits <command> [args...]`** (opt-in) keeps the job limits
across the UAC prompt. The elevated process is created by the AppInfo service,
not as a child of `admin`, so `capc 10 admin build.bat` otherwise runs the
elevated work with no CPU cap, memory limit or process limit at all. With the
flag, `admin` reads the job it is in and elevates this executable's `wnice`
front as `wnice <options> -- <command> [args...]`, so the work starts in a new
job with the same hard CPU cap, job memory limit, active-process limit,
affinity (when the mask is a contiguous run from CPU 0) and priority class. A
hard cap with a fractional percent is rounded down to a whole percent, never
below 1, with a note on stderr. Every limit that cannot be kept - a soft cap,
a sparse affinity mask, an unknown priority class - gets its own stderr line
and is left out; the rest is still kept. Not carried: `-s` (the deadline lives
in the wrapper process, not in the job) and the caps of outer jobs when the
caller's job is nested. No job, or a job with none of those limits: one note,
then plain `admin`. If the limits cannot be read, or the `wnice.exe` next to
`admin.exe` is not this executable's front, it exits 1 before any UAC prompt
instead of running unlimited. Already elevated, the flag changes nothing - the
command already inherits the job. `admin` without the flag is unchanged.

## AI CLI launchers

Not part of the priority/CPU-limiting toolset above — two convenience
wrappers that share the same binary and install/PATH mechanism.

### `cy [args...]`
Runs `claude --dangerously-skip-permissions [args...]`.

### `cx [args...]`
Runs `codex --dangerously-bypass-approvals-and-sandbox [args...]`.

**Both bypass the tool's own permission/approval/sandbox prompts.** Only use them
in a context where you'd already accept running that AI agent unattended (e.g.
inside an already-sandboxed/disposable environment). They do not add any sandboxing
of their own — the flag names describe exactly what they do.

### Limiting what `cy`/`cx` start: `WNICE_AGENT_LIMITS`

Both launchers read one environment variable. If `WNICE_AGENT_LIMITS` is set,
its whitespace-separated value may contain `wnice` limit options (`-p`, `-c`,
`-t`, `-m`, `-n`, `-s` each with its value, or `--boost`) and `--no-servers`,
which is accepted as a child-environment option, not a limit. The agent then
runs under exactly the job `wnice <those options> -- claude ...` (resp.
`codex ...`) would create: one job with those limits around the agent and
everything it starts, `cy -c` continuation sessions included. `--no-servers`
applies to the launched agent command's environment; it is not a job limit.
Unset or blank keeps the 0.4.0 behaviour: the agent still runs in a job, but
that job carries no limit and no CPU cap.

cmd.exe, for this shell (every `cy`/`cx` started from it is capped):

```
set WNICE_AGENT_LIMITS=-p idle -c 10 -m 10g
cy -c
```

PowerShell, for this session:

```powershell
$env:WNICE_AGENT_LIMITS = '-p idle -c 10 -m 10g'
```

Permanently for the user, in cmd.exe: `setx WNICE_AGENT_LIMITS
"-p idle -c 10 -m 10g"`; in PowerShell:
`[Environment]::SetEnvironmentVariable('WNICE_AGENT_LIMITS', '-p idle -c 10
-m 10g', 'User')`. Both take effect in new terminals. That is the stable place
for "every agent is capped": set once, nothing to add per invocation, and it
covers the `cy -c` sessions too. To cap a single agent instead, wrap it -
`wnice -c 10 -- cy -c` - and do not combine the two: the agent would then be
in a nested job and its percentage would be a share of the wrapper's.

The value goes through the same parser and validation as the `wnice` command
line, so values and error wording cannot drift. A command token, `--`,
`--wsl`, `--jobs`, `--help`/`--version`, an invalid value or a non-Unicode
value is refused: one stderr line naming the variable, exit code 1, nothing
started. `--no-servers` is accepted but sets only the launched command's
environment; it is not a limit. `wnice`, the single-purpose tools, `admin` and
`uiup` never read it.

## Install

```
npm install -g win-nice
```

GitHub Releases ship `SHA256SUMS`, two CycloneDX SBOMs and artifact
attestations, and the npm package is published with provenance - see
[docs/VERIFYING.md](docs/VERIFYING.md) for concrete verification commands.

This installs `wnice.exe` plus one `<tool>.exe` per tool name (hardlinks of
the same binary, so they cost no extra disk space; a plain copy where
hardlinks aren't supported) and the Git Bash shims into
`%LOCALAPPDATA%\win-nice\bin`, and adds that directory to your user `PATH`
(via `postinstall`). Restart your terminal afterwards so the new `PATH` takes
effect.

Upgrading while one of the tools is still running works: Windows won't let a
running `.exe` be overwritten or deleted, but it can be renamed, so the
installer moves the old files aside (`<tool>.exe.old-<n>`) and installs the new
ones in their place. The whole upgrade is one transaction — if any step fails,
every original file is restored. Leftover `.old-<n>` files are removed by the
next install/uninstall once the old process has exited.

To verify that an installed binary is the one a release published, see
[docs/VERIFYING.md](docs/VERIFYING.md).

`npm uninstall -g win-nice` does **not** reverse this - npm's `uninstall`
lifecycle script was removed (npm ≥ 7 never runs it at all; there is no
supported npm version where a `preuninstall` script would fire). Run
`npx win-nice uninstall` (see below) before or after the `npm uninstall`,
either order — `npx` re-fetches the package to run it, so it still works even
after the global package itself is gone.

The commands themselves are never registered through npm's own global `bin`
shimming — `idle`/`capc`/etc. are too generic a name to risk colliding with
someone else's global npm package. npm here is only the delivery mechanism for
a dedicated, PATH-managed install directory.

Without npm: put `wnice.exe` in a directory on your `PATH` and copy it as
`<tool>.exe` for each tool name you want.

### Managing an existing install

```
npx win-nice help        # commands and environment variables
npx win-nice status      # what's installed, where, which version
npx win-nice reinstall   # re-copy from the current package version
npx win-nice uninstall   # remove files + PATH entry
```

`uninstall`/`reinstall` treat every file recorded in the install manifest
(`%LOCALAPPDATA%\win-nice\install-manifest.json`) as owned by the package and
remove it regardless of local edits — these are managed files, not a
customization point. If the manifest itself is missing or corrupt, uninstall
falls back to scanning the install directory and only removes files it can
prove are its own (the exact shipped binary, the generated shim text, or the
`wnice.exe.managed` sidecar), so that scan doesn't delete unrelated files
sitting in the same directory.

### Upgrading from 0.3.0

Nothing to migrate by hand: `npm install -g win-nice` replaces the tools in
place (running exes are renamed aside; the upgrade is transactional). Every
0.3.0 tool name, option and exit code is unchanged, so existing scripts keep
working. Two things to know:

- Un-elevated `high`/`realtime`/`abovenormal` now apply the priority to the
  *whole tree* via the booster (0.3.0 set it on the root process only), and
  the explanatory stderr note changed its wording. Only scripts that matched
  the old `wnice: note: ... root process only` text verbatim need a one-line
  update.
- The new features are opt-in per invocation: `wnice --wsl ...` (needs a WSL2
  distro with systemd enabled) and `wnice --boost -p <class>`. Commands that
  do not use them behave exactly as in 0.3.0.

### Claude Code / Codex skill

```
npx win-nice skill install     # add the win-nice reference skill
npx win-nice skill uninstall   # remove it
```

Initial installation is a separate, opt-in step — `postinstall` never creates a
skill copy for a user who hasn't run `skill install`. Once a copy exists,
though, every subsequent ordinary package install/upgrade (including
`postinstall`) automatically refreshes that *existing* marked copy to the new
version's content, so it never goes stale after an upgrade. Explicit `skill
install` copies
[`skills/win-nice/SKILL.md`](skills/win-nice/SKILL.md) (documents every tool
above except `cy`/`cx`) to `~/.claude/skills/win-nice/SKILL.md` and
`~/.agents/skills/win-nice/SKILL.md` (Codex CLI's personal-skill location) —
`SKILL.md` is an open, cross-agent format (agentskills.io), so the same file
works for both unmodified.

Unlike the `bin/` install directory, `~/.claude/skills` and `~/.agents/skills`
aren't exclusively win-nice's — a `win-nice` folder there could belong to
something else entirely. `install` refuses to overwrite a file that's already
there without the `win-nice: managed-skill` marker comment (reports it as
skipped rather than clobbering it), and `uninstall` only removes a copy that
still carries that marker.

Exit code reflects this: `install`/`uninstall` exit 1 if any target was
skipped due to a real conflict (foreign file present for `install`;
marker-stripped/user-modified file for `uninstall`). A clean install/uninstall
exits 0, and so does `uninstall` finding nothing to remove — a missing target
isn't a conflict.

## Requirements

Windows 8 / Server 2012 or newer (Job Object CPU rate control), 64-bit. Node.js 24+
is only needed for the npm-based installer, not for the tools themselves.
`cy`/`cx` additionally need `claude`/`codex` installed and on `PATH`.

Maintainers build the binary with `npm run build:exe` (needs a Rust
toolchain; it runs `cargo build --release --locked` and stages
`bin/wnice.exe`). Users never need Rust: CI builds the exe and the published
package ships it.

### Environment variables

- `WIN_NICE_HOME` — overrides the install root (default
  `%LOCALAPPDATA%\win-nice`). Used by the test suite; also useful for a
  non-default install location.
- `WIN_NICE_SKILL_HOME` — overrides the home directory `~/.claude/skills/...`
  and `~/.agents/skills/...` are resolved against (default: the real user
  home). Mirrors `WIN_NICE_HOME`, for the skill files instead of `bin/`.
  Affects both the explicit `skill install`/`skill uninstall` commands and the
  automatic postinstall/upgrade refresh of an already-installed copy described
  above.
- `WIN_NICE_NO_PATH` — if set (to anything), `install`/`uninstall`/
  `reinstall` skip the user `PATH` update/removal entirely, only managing
  files under the install directory.
- `WNICE_AGENT_LIMITS` — `wnice` limit options applied to the agent that
  `cy`/`cx` start (see "Limiting what `cy`/`cx` start" above). Read only by
  those two launchers; unset or blank means no limits.

## Testing

From a source checkout (`test/` and `scripts/` aren't part of the published
npm package - the `test`/`test:elevated`/`release-check` scripts below only
work when run from a git clone, not against an installed package):

```
cargo test                # Rust unit tests + the real-process integration harness
npm test                  # installer logic (fast; isolated scratch registry key, cleaned up automatically)
npm run test:pester       # the installed tools driven from PowerShell (needs bin/wnice.exe)
```

`npm test` and `npm run test:pester` run inside `scripts/lib/guarded.ps1`:
a private hidden desktop (no test window reaches your screen), a cap on live
processes, Idle priority for the whole tree and a timeout. Because that
sandbox pins everything to Idle, the priority cases are skipped there and run
in CI. The Pester suite uses Pester 3 syntax; `scripts/run-pester.ps1`
already pins `-MaximumVersion 3.99` in case a newer Pester is also installed.

One Pester case exercises `admin`'s already-elevated branch and needs the
whole runner to be elevated; another only makes sense when *not* elevated.
`npm run test:elevated` (`test/pester/run-elevated.ps1`) asks for elevation
once and runs the suite in that session — run both for full coverage. No test
ever triggers a UAC prompt by itself.

`npm run release-check` (`scripts/release-check.js`) is a maintainer-only,
source-checkout-only command that packs the actual npm tarball, installs it
into an isolated temp directory, and verifies the real installed artifact
(exact file set, manifest version, `capc`/`capt`/`capm`/`caps`/`capn`/`idle` and
Git Bash shim exit-code smoke tests, and CHANGELOG/tag consistency) - this is what `publish.yml` runs right
before `npm publish`. It needs `scripts/` and git tag history, neither of
which is part of the published package, so it can't run against an installed
copy. The gated WSL integration tests (a real WSL2 distro) are described in
[CONTRIBUTING.md](CONTRIBUTING.md); `cargo test` and `npm test` stay green
without them.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache License, Version 2.0](LICENSE-APACHE),
at your option.
