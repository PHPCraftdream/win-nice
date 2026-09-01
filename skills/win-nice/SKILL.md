<!-- win-nice: managed-skill -->
---
name: win-nice
description: Reference for win-nice's Windows CLI tools for process priority, hard CPU quotas, and CPU affinity (idle, belownormal, abovenormal, high, realtime, cap, pint, uiup, admin). Use when the user asks how to limit CPU usage, priority, or thread/core affinity for a command on Windows, wants to avoid a build/test freezing the desktop, or mentions any of these tool names.
---

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
no race window), including anything it spawns, recursively.

- `cap <percent 1-100> <command> [args...]` — hard CPU quota
  (`JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`, hard cap). A real ceiling on total
  CPU%, not just scheduling priority — holds even when nothing else is
  contending for CPU. Example: `cap 50 npm run build`.
- `pint <thread-count> <command> [args...]` — short for "pin threads": restricts
  the whole tree to the first N *logical processors* via process affinity
  (`JOB_OBJECT_LIMIT_AFFINITY`). Threads, not physical cores — on
  Hyper-Threading/SMT hardware, N logical processors can be fewer physical
  cores. `<thread-count>` must be between 1 and
  `min([Environment]::ProcessorCount, 63)`. Example: `pint 4 npm run build`.

## Elevation / desktop responsiveness

- `admin <command> [args...]` — runs elevated (as Administrator); triggers the
  standard UAC consent prompt if the calling shell isn't already elevated, runs
  inline with no extra prompt if it is. The elevated equivalent of `idle`.
- `uiup` (no arguments) — one-shot `HIGH` priority boost for the live
  shell/UI/audio processes (`explorer`, `dwm`, `sihost`,
  `ShellExperienceHost`, `StartMenuExperienceHost`, `StartMenu`, `SearchApp`,
  `audiodg`) so the desktop stays responsive while heavy background work runs
  underneath. Self-elevates via UAC. Does **not** affect apps launched from
  Explorer afterward (`HIGH` isn't inherited by default).

## Argument safety

Every tool tries to launch the wrapped command directly first (no shell
involved) and only falls back to `cmd.exe /c` when the target is a `.bat`/
`.cmd` file or a cmd.exe builtin that genuinely needs one. On the direct path,
`&`, `|`, `<`, `>`, `^`, `%`, quotes, spaces, and empty strings all pass through
exactly as given. On the `cmd.exe /c` fallback path, `&|<>^`/quotes/spaces are
still fully protected, but a literal `%` can still trigger environment-variable
expansion — there's no reliable per-character escape for that at the
`cmd.exe /c` level (a limitation shared by anything that shells through
cmd.exe, Node's own `child_process` included).

One more gotcha, independent of the above: each tool ships as a `name.bat` /
`name.ps1` pair. Invoking the bare name from an actual PowerShell session
resolves to the `.ps1` and gets full argument safety. Invoking it from
`cmd.exe`, or via PATHEXT-based resolution the way Node's `child_process` (and
most non-PowerShell launchers) resolve a bare command on Windows — PATHEXT
doesn't include `.PS1` by default — lands on the `.bat` file instead, which
corrupts any literal `%` in its arguments before the command ever runs at all
(cmd.exe's own batch-parameter substitution rescanning for `%...%` patterns;
not fixable from inside a `.bat`). Every other special character survives that
hop untouched.

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
