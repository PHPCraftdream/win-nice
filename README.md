# win-nice

[![CI](https://github.com/PHPCraftdream/win-nice/actions/workflows/ci.yml/badge.svg)](https://github.com/PHPCraftdream/win-nice/actions/workflows/ci.yml)
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

No compiled binaries, no runtime dependencies. Just `.bat` + `.ps1` files that
call the relevant Win32 APIs directly (Job Objects, process priority classes).
npm is used only as a distribution/version channel and for the install CLI —
none of the tools themselves need Node.js to run.

## Argument handling

Every tool tries to launch the wrapped command directly first (`CreateProcess`,
no shell involved at all) and only falls back to `cmd.exe /c` when the target
turns out to be a `.bat`/`.cmd` file or a cmd.exe builtin that genuinely needs
one.

**Direct-launch path** (the common case: the target is a real `.exe`): arguments
are immune to cmd.exe's special characters entirely — `&`, `|`, `<`, `>`, `^`,
`%`, quotes, spaces, empty strings all pass through exactly as given, standard
MSVCRT/`CommandLineToArgvW` quoting. cmd.exe is never invoked on this path, so
there's nothing to expand.

**`cmd.exe /c` fallback path** (only reached for `.bat`/`.cmd` targets or
cmd.exe builtins): `&|<>^`/quotes/spaces/empty strings are still fully
protected. A literal `%` used to be able to trigger environment-variable
expansion here — cmd.exe pairs up `%` characters across the *entire* command
line, even across separate arguments, and there's no reliable per-character
escape for that at the `cmd.exe /c` level. Rather than risk that, this path now
**fails closed**: if any argument contains `%`, the tool refuses to run,
prints an error to stderr, and exits with code `1` — the command never reaches
cmd.exe. This is a change from just checking for the character; it's a hard
reject, not a best-effort escape.

**Separately, and unaffected by the fail-closed fix above:** each tool ships
as up to three files — `name.bat`, `name.ps1`, and a plain extensionless
`name` shim (a `#!/bin/sh` script) — and which one a bare `name ...`
invocation resolves to depends on the calling shell:

| Calling shell | Resolves to | `%` handling |
| --- | --- | --- |
| PowerShell | `name.ps1` | full argument safety (see above) |
| cmd.exe, or PATHEXT-based resolution (e.g. Node's `child_process`, which doesn't include `.PS1` in `PATHEXT` by default) | `name.bat` | corrupted before `.ps1` ever runs (see below) |
| POSIX shell (Git Bash only — ignores `PATHEXT`/bare-name extension resolution entirely) | `name` (no extension) | full argument safety — the shim `exec`s straight into `name.ps1` via `powershell -File`, the same direct-to-`.ps1` path PowerShell itself uses, with MSYS argument conversion disabled so slash-style switches (`/c`, `/d`) and Windows paths arrive untouched; no `.bat`/cmd.exe hop involved |

The extensionless shims are Git Bash-specific; WSL is not supported — WSL has
no bare `powershell` (only `powershell.exe`), Windows PowerShell can't resolve
the `/mnt/...` script path such a shim would pass to `-File`, and a WSL-side
npm refuses this package anyway (`"os": ["win32"]` in `package.json`). Install
through Windows Node/npm and call the tools from Git Bash.

The `.bat` file corrupts any literal `%` in its own arguments before your
command, and before `.ps1`, ever runs at all, confirmed with nothing more
than a bare `echo %1` in a plain `.bat`. This is cmd.exe's own
batch-parameter substitution (`%1`/`%*`) rescanning for `%...%` patterns
across the whole line while parsing the `.bat` entry point itself; there's no
fix for it from inside a `.bat` file, and it happens before the `.ps1` (and
its fail-closed `%` check) ever sees the arguments. Every other special
character (`&|<>^`) survives this hop untouched.

## Tools

### `idle <command> [args...]`
Runs `command` at `IDLE_PRIORITY_CLASS`, waits for it to exit, propagates its exit
code. Priority applies to the whole spawned process tree automatically — Windows'
`CreateProcess` inherits `IDLE`/`BELOW_NORMAL` priority by default when a child
process doesn't request a priority of its own.

### `belownormal <command> [args...]`
Same as `idle`, at `BELOW_NORMAL_PRIORITY_CLASS` — a lighter touch than idle.

### `abovenormal <command> [args...]`
Runs `command` at `ABOVE_NORMAL_PRIORITY_CLASS`, waits for it to exit, propagates
its exit code.

**Unlike `idle`/`belownormal`, this does *not* apply to the whole process tree** —
confirmed empirically. Windows only inherits `IDLE`/`BELOW_NORMAL` priority into
child processes by default; `ABOVE_NORMAL` and higher are not, so anything the
wrapped command spawns runs back at ordinary `Normal` priority. Only useful when
the command you're wrapping does the actual work itself rather than delegating to
child processes.

### `high <command> [args...]`
Same as `abovenormal`, at `HIGH_PRIORITY_CLASS` — a stronger boost. Same
single-process-only caveat applies.

### `realtime <command> [args...]`
Same shape, at `REALTIME_PRIORITY_CLASS`. **Dangerous, use with caution:**
`REALTIME` outranks the OS's own input/audio/UI threads — a busy realtime-priority
process can make the entire desktop (mouse, keyboard, everything) stop responding,
which is the exact failure mode this project otherwise exists to prevent. It also
needs the `SeIncreaseBasePriorityPrivilege` privilege (elevated/admin processes
have it by default); without it, Windows doesn't error out, it silently downgrades
the request to `HIGH_PRIORITY_CLASS` instead — confirmed empirically. Same
single-process-only caveat as `abovenormal`/`high` applies on top of all that.

### `cap <percent> <command> [args...]`
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
parent already lets through, so equal caps **multiply**: `cap 50 cap 50 ...`
yields roughly 25% of total system CPU, not 50% (see "Chaining these tools
together" below for the full picture across limit types).

Blocks until the command exits, propagates its exit code.

```
cap 50 npm run build
```

### `pint <thread-count> <command> [args...]`
Short for **pin threads**. Restricts the whole process tree to the first
`<thread-count>` logical processors via Windows process affinity
(`JOBOBJECT_BASIC_LIMIT_INFORMATION`, `JOB_OBJECT_LIMIT_AFFINITY`) — same
suspend-then-assign-then-resume Job Object mechanism as `cap`, so the same
"covers the whole subtree from the first instruction" and "breakaway fails
closed" guarantees apply.

Deliberately *threads*, not *cores*, in both the name and the semantics:
Windows affinity masks address logical processors (hardware threads), not
physical cores. On a machine with Hyper-Threading/SMT, `pint 4` pins to 4
*logical processors* — depending on which ones, that could be 2 fully-used
physical cores or 4 half-used ones; the affinity API has no concept of "whole
core" grouping on its own. `<thread-count>` must be between 1 and the number
of logical processors on the machine (`[Environment]::ProcessorCount`, capped
at 63 — a single affinity mask can't address more).

```
pint 4 npm run build
```

### `capm <size> <command> [args...]`
Hard memory ceiling for the whole process tree, enforced by a Windows Job
Object (`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, `JOB_OBJECT_LIMIT_JOB_MEMORY`)
— same suspend-then-assign-then-resume mechanism as `cap`/`pint`, so the same
"covers the whole subtree from the first instruction" and "breakaway fails
closed" guarantees apply. Caps the whole job's *aggregate* committed memory,
not any single process — like `cap`'s CPU% and `pint`'s affinity, it's one
ceiling for the whole tree, not a per-process limit.

`<size>` accepts three forms:

| Form | Meaning |
| --- | --- |
| `50` (bare integer, `1`-`100`) | percent of total physical RAM (`GlobalMemoryStatusEx`), not of whatever's currently free — the cap means the same thing regardless of what else is running on the machine at invocation time. Same convention as `cap`'s own `<percent 1-100>` — deliberately no `%` character; see "Chaining these tools together" below for why |
| `512m` / `512M` | megabytes |
| `2g` / `2G` | gigabytes |

```
capm 50 npm run build
capm 512m npm run build
capm 2g npm run build
```

**Unlike `cap`, exceeding the limit doesn't throttle — it fails the
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

**A `cap`/`pint`/`capm` limit sticks to any daemon the wrapped command leaves
running**, for that daemon's entire lifetime — not just for the wrapped
command's own run. Job Object membership is permanent for a process once
assigned (short of an explicit, disallowed breakaway); a background process
the command spawns and detaches from is still in the same job, still capped,
for as long as it stays alive. This bites build tools that reuse a persistent
process across invocations to skip cold-start cost: `dotnet build`'s
`VBCSCompiler`/MSBuild node reuse, a Gradle daemon, file-watcher processes
left running by `npm run watch`-style scripts. A follow-up **uncapped**
`dotnet build` (or `gradle`) can end up running inside the *previous* `cap`
call's Job Object without a new `cap`/`pint`/`capm` invocation of its own,
capped because a stale daemon from an earlier call is doing the work. Either
don't leave the daemon running across a call whose limit shouldn't persist
(`dotnet build -p:UseSharedCompilation=false`, `gradle --no-daemon`), or
accept that the limit is now effectively attached to the daemon until it's
killed.

## Chaining these tools together

These tools can be stacked by passing one as another's `<command>`:

```
capm 50 cap 50 idle npm run build
```

Each wrapper wraps everything after its own arguments, so the outermost
wrapper is whichever one you type first. A few things to know before relying
on a combination:

**Bare tool names resolve through the same `cmd.exe`/`PATHEXT` fallback
documented above.** None of these ship a `.exe`, so e.g. `capm`'s attempt to
launch `cap` directly always fails and falls back to `cmd.exe`, which finds
`cap.bat` via `PATHEXT` — meaning chaining only works when the tools'
install directory is actually on `PATH`, and any `%` elsewhere on that
command line trips the same fail-closed check described above. `capm`'s own
`<size>` deliberately has no `%` form for exactly this reason: an earlier
version accepted `25%`, and `cap 50 capm 25% ...` failed the fail-closed
check while `capm 25% cap 50 ...` worked fine — an order-dependent foot-gun.
A bare percent (`capm 50 cap 50 ...`) sidesteps it entirely, in any order.

**Nested Job Object limits do not follow one universal "smaller wins" rule —
each limit type combines differently:**

- **CPU (`cap`)**: a nested job's CPU rate is relative to what its parent
  already lets through, so equal caps *multiply* rather than take the
  minimum — `cap 50 cap 50 ...` yields roughly 25% of total system CPU, not
  50%. This is documented Windows behavior for
  [`JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information),
  not a bug here.
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
- **Priority (`idle`/`belownormal`/`abovenormal`/`high`/`realtime`)**: not a
  Job Object limit — the *last* one applied to a given process simply wins,
  same as invoking any one of them alone.
- **Affinity (`pint`)**: nested affinity is an *effective-limits* chain — a
  child job's mask can be as tight as it wants but is clamped to whatever
  the parent already allows, never wider
  ([Nested Jobs — Job Limits](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs)).
  Confirmed empirically: `pint 2 pint 3 ...` (inner asking for *more*
  processors than the outer allows) still comes back pinned to the outer's
  2, not the inner's 3 — no error, just silently clamped to the tighter mask.

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
own `.bat`/`.cmd` auto-relaunch), but any other target launches directly via
`-FilePath`, never touching cmd.exe — same as the already-elevated branch, a
literal `%` in any argument is only refused when the `.bat`/`.cmd` fallback is
actually needed, since a direct launch is never exposed to `%` expansion at all.

```
admin npm install -g some-package
```

## AI CLI launchers

Not part of the priority/CPU-limiting toolset above — these two are unrelated
one-line convenience wrappers that happened to live alongside win-nice's own
scripts and got folded into the same install/PATH mechanism.

### `cy [args...]`
Runs `claude --dangerously-skip-permissions [args...]`.

### `cx [args...]`
Runs `codex --dangerously-bypass-approvals-and-sandbox [args...]`.

**Both bypass the tool's own permission/approval/sandbox prompts.** Only use them
in a context where you'd already accept running that AI agent unattended (e.g.
inside an already-sandboxed/disposable environment). They do not add any sandboxing
of their own — the flag names describe exactly what they do.

## Install

```
npm install -g win-nice
```

This copies every tool above into `%LOCALAPPDATA%\win-nice\bin` and adds that
directory to your user `PATH` (via `postinstall`). Restart your terminal
afterwards so the new `PATH` takes effect.

`npm uninstall -g win-nice` does **not** reverse this — npm's `uninstall`
lifecycle script was removed (npm ≥ 7 never runs it at all; there is no
supported npm version where a `preuninstall` script would fire). Run
`npx win-nice uninstall` (see below) before or after the `npm uninstall`,
either order — `npx` re-fetches the package to run it, so it still works even
after the global package itself is gone.

The commands themselves are never registered through npm's own global `bin`
shimming — `idle`/`cap`/etc. are too generic a name to risk colliding with
someone else's global npm package. npm here is only the delivery mechanism for
a dedicated, PATH-managed install directory.

Without npm: copy the contents of `bin/` into any directory on your `PATH`.

### Managing an existing install

```
npx win-nice status      # what's installed, where, which version
npx win-nice reinstall   # re-copy from the current package version
npx win-nice uninstall   # remove files + PATH entry
```

`uninstall`/`reinstall` treat every file recorded in the install manifest
(`%LOCALAPPDATA%\win-nice\install-manifest.json`) as owned by the package and
remove it regardless of local edits — these are managed files, not a
customization point. If the manifest itself is missing or corrupt, uninstall
falls back to scanning the install directory and only removes files that still
carry the `win-nice: managed-file` marker comment, so that scan doesn't delete
unrelated files sitting in the same directory.

### Claude Code / Codex skill

```
npx win-nice skill install     # add the win-nice reference skill
npx win-nice skill uninstall   # remove it
```

Separate, opt-in install step — not run automatically by `postinstall`. Copies
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

Windows 8 / Server 2012 or newer (Job Object CPU rate control). PowerShell is
bundled with Windows — no separate install needed to run the tools. Node.js is
only needed for the npm-based installer/tests, not for the tools themselves.
`cy`/`cx` additionally need `claude`/`codex` installed and on `PATH`.

**PowerShell execution policy:** Windows client editions default to
`Restricted`, which blocks a bare `.ps1` invoked directly by PowerShell itself
(`... cannot be loaded because running scripts is disabled on this system`).
The `.bat` files are unaffected (they pass `-ExecutionPolicy Bypass`
explicitly), and so are the Git Bash shims — each one runs
`powershell -NoProfile -ExecutionPolicy Bypass -File ...` itself, so only
invoking a bare `name.ps1` from PowerShell needs the one-time fix below.
Caveat: Group Policy can still override `-ExecutionPolicy Bypass` in some
managed environments. Run once, as the user who'll run these tools:

```
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

### Environment variables

- `WIN_NICE_HOME` — overrides the install root (default
  `%LOCALAPPDATA%\win-nice`). Used by the test suite; also useful for a
  non-default install location.
- `WIN_NICE_SKILL_HOME` — overrides the home directory `skill install`/
  `skill uninstall` resolve `~/.claude/skills/...` and `~/.agents/skills/...`
  against (default: the real user home). Mirrors `WIN_NICE_HOME`, for the
  skill files instead of `bin/`.
- `WIN_NICE_NO_PATH` — if set (to anything), `install`/`uninstall`/
  `reinstall` skip the user `PATH` update/removal entirely, only managing
  files under the install directory.

## Testing

```
npm test                                       # installer logic (fast; isolated scratch registry key, cleaned up automatically)
powershell -Command "Invoke-Pester -Path test\win-nice.Tests.ps1"   # real tool behavior
```

Pin the Pester version before running the second command by hand: Windows
ships Pester 3.4.0 built in, but a system with a newer Pester also installed
(GitHub-hosted runners have both 3.4.0 and 5.x side by side) auto-loads the
newer one, and this suite uses Pester 3's legacy assertion syntax (`Should
Be`), which 5.x removed entirely. If bare `Invoke-Pester` fails immediately
on every `It`, import the right version first:

```
powershell -Command "Import-Module Pester -MaximumVersion 3.99; Invoke-Pester -Path test\win-nice.Tests.ps1"
```

(this is what CI/publish do; see `.github/workflows/ci.yml`.)

The Pester suite is an integration suite: it spawns real processes, checks
actual `PriorityClass`, `ProcessorAffinity`, and Job Object CPU throttling
across every tool, and takes roughly 1-2 minutes (more under system load - the
CPU-cap test retries a few times if the machine is too busy to get a clean
baseline). It never touches the real system `PATH`/registry; the installer
tests use `WIN_NICE_HOME` to redirect installs into a temp directory instead.

3 of its cases only exercise `admin.ps1`'s already-elevated branch, which
needs the whole test-runner process to already be elevated (not just
`admin.ps1` itself) - a normal, unelevated run reports them `Skipped`, which
is expected, not a failure. `npm run test:elevated` (`test/run-elevated.ps1`)
is a single entry point for that: one UAC prompt elevates the runner once,
then the full suite - all cases, not just those 3 - runs inside that one
elevated session, with the result relayed back to the original console.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache License, Version 2.0](LICENSE-APACHE),
at your option.
