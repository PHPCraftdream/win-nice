---
name: win-nice
description: Reference for win-nice's Windows CLI tools for process priority, hard CPU quotas, CPU affinity, memory limits, process-count ceilings, and a wall-clock timeout (idle, belownormal, abovenormal, high, realtime, capc, capt, capm, caps, capn, uiup, admin). Use when the user asks how to limit CPU usage, priority, thread/core affinity, memory, or the number of concurrent processes for a command on Windows, wants to kill a command after a timeout, wants to avoid a build/test freezing the desktop, or mentions any of these tool names.
---

<!-- win-nice: managed-skill -->

# win-nice

Windows equivalent of the unix `nice` / `renice` / `cpulimit` family. Built for
running several parallel builds/tests/AI coding agents on one Windows box without
pegging every core and freezing the desktop (mouse, keyboard, window dragging,
audio - all of it).

Every tool below runs `<command> [args...]`, blocks until it exits, and
propagates its exit code. None of them need Node.js at runtime.

## Priority tools

Windows priority classes, lowest to highest. `idle`/`belownormal` propagate to
the *whole* process tree (Windows inherits `IDLE`/`BELOW_NORMAL` into child
processes by default). `abovenormal`/`high`/`realtime` do **not** propagate -
only the directly wrapped process gets the boosted priority; anything it spawns
runs at ordinary `Normal` priority. Confirmed empirically, not just from docs.

- `idle <command> [args...]` — `IDLE_PRIORITY_CLASS`.
- `belownormal <command> [args...]` — `BELOW_NORMAL_PRIORITY_CLASS`, a lighter
  touch than `idle`.
- `abovenormal <command> [args...]` — `ABOVE_NORMAL_PRIORITY_CLASS`. Single
  process only (see above) — pick this over `high`/`realtime` when you just need
  a mild boost for one foreground process.
- `high <command> [args...]` — `HIGH_PRIORITY_CLASS`. Single process only.
- `realtime <command> [args...]` — `REALTIME_PRIORITY_CLASS`. **Dangerous**:
  outranks the OS's own input/audio/UI threads: a busy realtime process can
  freeze the whole desktop, the exact failure mode win-nice otherwise exists to
  prevent. Needs `SeIncreaseBasePriorityPrivilege` (elevated processes have it
  by default) — without it, Windows silently downgrades the request to `HIGH`
  instead of erroring. Single process only, same as `high`.

## Resource limits

Job-Object-based; cover the *whole* process tree from the wrapped command's
first instruction (created suspended, assigned to the job, only then resumed —
no race window), including anything it spawns, recursively via ordinary
`CreateProcess` calls. Exceptions: explicit `CREATE_BREAKAWAY_FROM_JOB`, and
processes brought up through an external broker/service (e.g. WMI's
`Win32_Process.Create`) that never goes through the tree's own `CreateProcess`.

- `capc <percent 1-100> <command> [args...]` — hard CPU quota
  (`JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`, hard cap). A real ceiling on total
  CPU%, not just scheduling priority — holds even when nothing else is
  contending for CPU. Example: `capc 50 npm run build`.
- `capt <thread-count> <command> [args...]` — short for "cap threads": restricts
  the whole tree to the first N *logical processors* via process affinity
  (`JOB_OBJECT_LIMIT_AFFINITY`). Threads, not physical cores — on
  Hyper-Threading/SMT hardware, N logical processors can be fewer physical
  cores. `<thread-count>` must be between 1 and
  `min([Environment]::ProcessorCount, 63)`. Example: `capt 4 npm run build`.
- `capm <size> <command> [args...]` — hard memory ceiling
  (`JOB_OBJECT_LIMIT_JOB_MEMORY`), aggregate across the whole tree, not
  per-process. `<size>`: bare integer `1`-`100` = percent of total physical RAM
  (same convention as `capc`'s own `<percent 1-100>`, deliberately no `%`
  character - see Chaining below), or `m`/`M` = megabytes (`512m`), or `g`/`G`
  = gigabytes (`2g`). Unlike `capc`, exceeding it doesn't throttle - it fails
  the allocation (`OutOfMemoryException`/`VirtualAlloc` failure), which
  usually crashes the wrapped program since most don't handle that
  gracefully; set it too low and even the wrapped runtime can fail to start.
  Example: `capm 512m npm run build`.
- `caps <seconds> <command> [args...]` — hard wall-clock timeout for the whole
  process tree: if the command hasn't exited within `<seconds>`, one
  `TerminateJobObject` kernel call force-kills everything still in the Job
  Object (the whole subtree, from the first instruction via the same
  suspend-then-assign-then-resume mechanism as `capc`/`capt`/`capm`), and
  `caps` exits with code 124 (unix `timeout(1)` convention). The deadline is
  absolute — computed from `DateTime.UtcNow` and armed into a one-shot
  waitable timer waited on together with the process handle
  (`WaitForMultipleObjects`, with a `GetProcessTimes` check rejecting an exit
  that only won the simultaneous-signal race after the deadline) — so time
  the machine spends asleep/suspended counts against it, and it fires
  immediately on wake if it passed during sleep. The job carries
  only `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — no resource limit — which is also
  the backstop if the `caps` wrapper itself dies non-cooperatively; released
  before a normal inside-the-deadline exit, so a daemon the command legitimately
  left running survives. `<seconds>`:
  positive whole or decimal (`2`, `2.5`), converted to whole milliseconds
  (min 1 ms); max 4294967294 ms (~49.7 days) since `WaitForMultipleObjects`'
  `dwMilliseconds` is a uint32 with `0xFFFFFFFF` reserved as INFINITE — larger
  values are a usage error, never silently truncated. Finishing in time
  propagates the exit code like every other launcher.
  Example: `caps 300 npm test`.
- `capn <count> <command> [args...]` — hard ceiling on the number of
  simultaneously active processes in the whole tree
  (`JOB_OBJECT_LIMIT_ACTIVE_PROCESS`). The count includes the directly
  wrapped process itself (it is assigned to the still-empty job before it can
  spawn anything), so `capn 1 <command>` lets the command run but fails its
  first child-spawn attempt. Exceeding the limit fails only the offending
  spawn attempt - nothing is killed or throttled, and a command within budget
  is unaffected (all confirmed empirically). `<count>`: positive whole
  number, 1 to 4294967295 (the uint32 `ActiveProcessLimit` field's own
  range). Example: `capn 10 npm run build`.

**A limit sticks to any daemon the wrapped command leaves running**, for that
daemon's whole lifetime, not just the one `capc`/`capt`/`capm`/`capn` call — Job
Object membership is permanent once assigned. Build tools that reuse a background
process to skip cold-start cost (`dotnet build`'s VBCSCompiler/MSBuild node
reuse, a Gradle daemon, `npm run watch`-style file watchers) can leave a
*later, uncapped-looking* invocation actually running inside an earlier
`capc`/`capt` call's job. Escape hatches: `dotnet build
-p:UseSharedCompilation=false`, `gradle --no-daemon` — or accept the daemon
stays limited until it's killed.

## Chaining

These tools can be stacked, e.g. `capm 50 capc 50 idle npm run build`. Bare
tool names resolve through the same `cmd.exe`/`PATHEXT` fallback as any other
target, so chaining needs the tools' install directory on `PATH`, and any `%`
in the command line still trips the fail-closed check. Nested Job Object
limits do **not** follow one universal "smaller wins" rule: CPU rate
(`capc`) is relative to its parent job and *multiplies* when nested (`capc 50
capc 50 ...` ≈ 25% of system CPU, not 50% - see
[`JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information)),
while memory (`capm`) ceilings apply independently to accounting scopes of
different sizes - a job's committed-memory accounting includes every child
job's committed memory plus its own process, a child job's accounting
doesn't see the outer wrapper's process at all - so nested `capm` ceilings
don't reduce to a simple `min(limit1, limit2)`. Process count (`capn`) limits
are likewise enforced independently per job - a spawn has to fit under every
job in the chain at once, and an outer job's count already includes the inner
wrapper process itself (plus anything it spawns, down to a PowerShell-based
inner tool's own csc.exe/CVTRES.EXE compiler children), so `capn 1 capn 5 ...`
fails before the inner limit even matters and the effective budget is not a
plain `min()` - leave outer headroom for the chain itself (roughly 3 slots
for a PowerShell-based inner tool). Priority (`idle`/etc.) isn't
a Job Object limit at all - the last one applied wins. A timeout (`caps`) is
likewise not a Job Object limit being combined - it's a deadline each `caps`
wrapper enforces on its own direct child: the innermost `caps` fires at its own
deadline (the outer propagates the 124), while an outer `caps` whose deadline
fires first kills the whole subtree including the inner wrapper, whose own
`KILL_ON_JOB_CLOSE` job then takes down everything beneath it - so the caller
sees 124 either way. See README.md's
"Chaining these tools together" for the full explanation.

## Elevation / desktop responsiveness

- `admin <command> [args...]` — runs elevated (as Administrator); triggers the
  standard UAC consent prompt if the calling shell isn't already elevated, runs
  inline with no extra prompt if it is. Blocking elevation wrapper with the same
  wait-and-propagate-exit-code semantics as the other wrappers - it does not set
  a priority class like `idle` does.
- `uiup` (no arguments) — one-shot `HIGH` priority boost for the live
  shell/UI/audio processes (`explorer`, `dwm`, `sihost`,
  `ShellExperienceHost`, `StartMenuExperienceHost`, `StartMenu`, `SearchApp`,
  `audiodg`), intended to improve desktop responsiveness while heavy
  background work runs underneath - a best-effort one-shot tweak, not a
  guarantee (memory pressure, I/O saturation, driver/GPU stalls, or a
  realtime workload elsewhere can still make the desktop stutter). Self-
  elevates via UAC. Does **not** affect apps launched from
  Explorer afterward (`HIGH` isn't inherited by default).

## Argument safety

Every tool tries to launch the wrapped command directly first (no shell
involved) and only falls back to `cmd.exe /c` when the target is a `.bat`/
`.cmd` file or a cmd.exe builtin that genuinely needs one.

**Direct-launch path** (the common case: a real `.exe`): `&`, `|`, `<`, `>`,
`^`, `%`, quotes, spaces, and empty strings all pass through exactly as
given — cmd.exe is never invoked, so there's nothing to expand.

**`cmd.exe /c` fallback path** (`.bat`/`.cmd` targets or cmd.exe builtins
only): `&|<>^`/quotes/spaces/empty strings are still fully protected. A
literal `%` used to be able to trigger environment-variable expansion here;
there's no reliable per-character escape for that at the `cmd.exe /c` level.
This path now **fails closed** instead: if any argument contains `%`, the
tool refuses to run, prints an error to stderr, and exits with code `1` — the
command never reaches cmd.exe.

**Separately, and unaffected by the fail-closed fix above:** each tool ships
as up to three files, and which one a bare `name ...` invocation resolves to
depends on the calling shell:

| Calling shell | Resolves to | `%` handling |
| --- | --- | --- |
| PowerShell | `name.ps1` | full argument safety (see above) |
| cmd.exe, or PATHEXT-based resolution (e.g. Node's `child_process` — `PATHEXT` doesn't include `.PS1` by default) | `name.bat` | corrupted before `.ps1` ever runs |
| POSIX shell (Git Bash only — ignores `PATHEXT`; WSL not supported) | `name` (extensionless shim) | full argument safety — `exec`s straight into `name.ps1` with MSYS argument conversion disabled, same as PowerShell |

The `.bat` file corrupts any literal `%` in its arguments before the command,
and before `.ps1` (and its fail-closed `%` check), ever runs at all
(cmd.exe's own batch-parameter substitution rescanning for `%...%` patterns
while parsing the `.bat` entry point itself; not fixable from inside a
`.bat`). Every other special character survives that hop untouched.

**PowerShell execution policy:** Windows client editions default to
`Restricted`, which blocks a bare `.ps1` invoked directly by PowerShell with
"running scripts is disabled on this system". One-time fix:
`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`. `.bat` files and the
Git Bash shims are unaffected — both pass `-ExecutionPolicy Bypass`
themselves (each shim runs powershell with that flag explicitly). Group
Policy can still override this in managed environments.

## Install / manage

```
npm install -g win-nice        # puts the tools above on PATH
npx win-nice status            # what's installed, where, which version
npx win-nice reinstall         # re-copy from the current package version
npx win-nice uninstall         # remove files + PATH entry
```

Not covered here: `cy`/`cx`, unrelated one-line launchers for
`claude --dangerously-skip-permissions` / `codex --dangerously-bypass-approvals-and-sandbox`
that happen to ship alongside these tools.
