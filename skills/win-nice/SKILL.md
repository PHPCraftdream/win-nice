---
name: win-nice
description: Reference for win-nice's Windows CLI tools for process priority, hard CPU quotas, CPU affinity, memory limits, process-count ceilings, and a wall-clock timeout (wnice, idle, belownormal, abovenormal, high, realtime, capc, capt, capm, caps, capn, uiup, admin). Use when the user asks how to limit CPU usage, priority, thread/core affinity, memory, or the number of concurrent processes for a command on Windows, wants to kill a command after a timeout, wants to avoid a build/test freezing the desktop, or mentions any of these tool names.
---

<!-- win-nice: managed-skill -->

# win-nice

Windows equivalent of the unix `nice` / `renice` / `cpulimit` family. Built for
running several parallel builds/tests/AI coding agents on one Windows box without
pegging every core and freezing the desktop (mouse, keyboard, window dragging,
audio - all of it).

Every tool below runs `<command> [args...]`, blocks until it exits, and
propagates its exit code. All of them are one native executable (`wnice.exe`,
the other names are copies of it) — no PowerShell, .NET or Node.js at runtime.

## `wnice` — every limit in one wrapper (prefer this)

`wnice [options] [--] <command> [args...]` applies any combination of limits
through **one** wrapper process and **one** Job Object, so the tree is just
`wnice.exe -> command`:

```
wnice -p idle -c 50 -t 4 -m 50 -n 64 -s 600 -- npm run build
```

`-p/--priority <idle|belownormal|normal|abovenormal|high|realtime>`,
`-c/--cpu <1-100>` (= `capc`), `-t/--threads <n>` (= `capt`),
`-m/--memory <size>` (= `capm`), `-n/--procs <n>` (= `capn`),
`-s/--timeout <seconds>` (= `caps`, exit 124), `-h`, `-V`. Options end at the
first non-option token or `--`; the rest goes to the command verbatim. The
single-purpose tools below are the same machinery with one limit each.
Every tool prints detailed help with examples for `--help` (`wnice --help`,
`capc --help`, ...); usage errors exit `1`.

## Priority tools

Windows priority classes, lowest to highest, set as a Job Object limit so they
hold for the *whole* process tree (children and grandchildren).

- `idle <command> [args...]` — `IDLE_PRIORITY_CLASS`.
- `belownormal <command> [args...]` — `BELOW_NORMAL_PRIORITY_CLASS`, a lighter
  touch than `idle`.
- `abovenormal <command> [args...]` — `ABOVE_NORMAL_PRIORITY_CLASS`, a mild
  boost for the whole tree.
- `high <command> [args...]` — `HIGH_PRIORITY_CLASS`. Tree-wide only with
  `SeIncreaseBasePriorityPrivilege` (elevated processes have it); otherwise the
  job refuses (Win32 error 1314) and only the root process gets `HIGH`, with a
  `wnice: note: ...` line on stderr.
- `realtime <command> [args...]` — `REALTIME_PRIORITY_CLASS`. **Dangerous**:
  outranks the OS's own input/audio/UI threads: a busy realtime process can
  freeze the whole desktop, the exact failure mode win-nice otherwise exists to
  prevent. Needs the same privilege as `high`; without it you get `HIGH` on the
  root process only, with the same stderr note.

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
  min(logical processors, 63). Example: `capt 4 npm run build`.
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
- `caps <seconds> <command> [args...]` — wall-clock timeout for the whole
  process tree: if the command is still running when `<seconds>` have passed, one
  `TerminateJobObject` kernel call force-kills everything still in the Job
  Object (the whole subtree, from the first instruction via the same
  suspend-then-assign-then-resume mechanism as `capc`/`capt`/`capm`), and
  `caps` exits with code 124 (unix `timeout(1)` convention). The deadline is
  absolute — computed from the system clock and armed into a one-shot
  waitable timer waited on together with the process handle
  (`WaitForMultipleObjects`, with a `GetProcessTimes` check rejecting an exit
  that only won the simultaneous-signal race after the deadline) — so time
  the machine spends asleep/suspended counts against it, and it fires
  immediately on wake if it passed during sleep; the due time is a
  system-clock (FILETIME) value, so a manual or service-driven clock
  adjustment during the wait can shorten or lengthen the actual wait relative
  to `<seconds>`. The job carries
  only `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — no resource limit — which is also
  the backstop if the `caps` wrapper itself dies non-cooperatively; released
  before a normal inside-the-deadline exit, so a daemon the command legitimately
  left running survives. `<seconds>`:
  positive whole or decimal (`2`, `2.5`), converted to whole milliseconds
  (min 1 ms); max 4294967294 ms (~49.7 days) — a deliberate usage ceiling,
  not a uint32 limit (the deadline is an absolute FILETIME and the wait
  itself is INFINITE) — larger values are a usage error, never silently
  truncated. Finishing in time propagates the exit code like every other launcher.
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

These tools can be stacked, e.g. `capm 50 capc 50 idle npm run build`, but a
single `wnice -m 50 -c 50 -p idle -- npm run build` does the same in one job
and avoids every nesting rule below — prefer it. Chained tool names resolve
through `PATH`, so chaining needs the install directory on `PATH`. Nested Job Object
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
wrapper process itself, so `capn 1 capn 5 ...` fails before the inner limit
even matters and the effective budget is not a plain `min()` - leave outer
headroom for the chain itself (one slot per inner wrapper). Priority
(`idle`/etc.) is a job limit too: with nested jobs the outermost one that sets
a priority decides. A timeout (`caps`) is
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
involved) and only goes through `cmd.exe /c` when the target is a `.bat`/
`.cmd` file, which genuinely needs it.

**Direct-launch path** (the common case: a real `.exe`): `&`, `|`, `<`, `>`,
`^`, `%`, quotes, spaces, and empty strings all pass through exactly as
given — cmd.exe is never invoked, so there's nothing to expand.

**`cmd.exe /c` fallback path** (`.bat`/`.cmd` targets only):
`&|<>^`/quotes/spaces/empty strings are still fully protected. A literal `%`
could trigger environment-variable expansion here and there's no reliable
per-character escape for that at the `cmd.exe /c` level, so this path **fails
closed**: if any argument contains `%`, the tool refuses to run, prints an
error to stderr, and exits with code `1` — the command never reaches cmd.exe.

Bare command names are resolved from absolute `PATH` entries only, in
`PATHEXT` order — **never from the current directory** — and a name found
nowhere exits `127`. cmd.exe builtins (`dir`, `echo`) are not files: wrap
them as `idle cmd /c dir`.

The tools are real `.exe` files, so PowerShell, pwsh, cmd.exe, Node's
`child_process` and Git Bash all start them the same way. What the *calling
shell* still does first: cmd.exe expands `%VAR%` on its own command line, and
Windows PowerShell 5.1 mis-quotes an argument ending in a backslash when
passing it to any native program.

Git Bash gets an extensionless `name` shim next to each `name.exe` that sets
`MSYS2_ARG_CONV_EXCL='*'` (keeps `/c` and Windows paths intact) and `exec`s
the exe. Known limitation: that variable is inherited by the wrapped command
tree, so an MSYS program run *inside* it (an inner `bash`/`sh`, a `#!/bin/sh`
git hook) stops converting POSIX paths in its own children's arguments
(`caps 600 bash -c 'node /c/proj/run.js'` fails to find the module); call
`caps.exe` directly in that case. WSL is not supported.

## Install / manage

```
npm install -g win-nice        # puts the tools above on PATH
npx win-nice status            # what's installed, where, which version
npx win-nice reinstall         # reinstall from the current package version
npx win-nice uninstall         # remove files + PATH entry
```

Not covered here: `cy`/`cx`, convenience launchers for
`claude --dangerously-skip-permissions` / `codex --dangerously-bypass-approvals-and-sandbox`
that ship in the same binary.
