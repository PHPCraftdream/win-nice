# win-nice

Windows equivalent of the unix `nice` / `renice` / `cpulimit` family, built for a
specific pain point: running several parallel AI coding agents (Claude Code, etc.)
on one Windows box without their builds/tests pegging every core and freezing the
desktop — mouse, keyboard, window dragging, audio, all of it.

No compiled binaries, no runtime dependencies. Just `.bat` + `.ps1` files that
call the relevant Win32 APIs directly (Job Objects, process priority classes).
npm is used only as a distribution/version channel and for the install CLI —
none of the tools themselves need Node.js to run.

## Tools

### `idle <command> [args...]`
Runs `command` at `IDLE_PRIORITY_CLASS`, waits for it to exit, propagates its exit
code. Priority applies to the whole spawned process tree automatically — Windows'
`CreateProcess` inherits `IDLE`/`BELOW_NORMAL` priority by default when a child
process doesn't request a priority of its own.

### `belownormal <command> [args...]`
Same as `idle`, at `BELOW_NORMAL_PRIORITY_CLASS` — a lighter touch than idle.

### `cap <percent> <command> [args...]`
Hard CPU quota (1-100) for the whole process tree, enforced by a Windows Job
Object (`JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`, hard cap). Unlike `idle`/
`belownormal`, this is a real ceiling on total CPU%, not just a scheduling
priority — it holds even when nothing else on the machine is contending for CPU.
Child processes are captured too (Windows nests job objects automatically since
Windows 8). Blocks until the command exits, propagates its exit code.

```
cap 50 npm run build
```

### `uiup`
One-shot priority boost (`HIGH`) for the live shell/UI/audio processes so the
desktop stays responsive while heavy background work runs underneath:
`explorer`, `dwm`, `sihost`, `ShellExperienceHost`, `StartMenuExperienceHost`,
`StartMenu`, `SearchApp`, `audiodg`.

Self-elevates via UAC — `dwm`/`sihost` run under a separate account
(`Window Manager\DWM-1`), so raising their priority needs admin rights.

The boost does **not** propagate to apps you launch from Explorer afterwards:
`HIGH` priority isn't inherited by child processes under Windows' default
`CreateProcess` rules (only `IDLE`/`BELOW_NORMAL` are). Confirmed empirically —
see the project history for the test.

## Install

```
npm install -g win-nice
```

This copies `idle`/`belownormal`/`cap`/`uiup` into `%LOCALAPPDATA%\win-nice\bin`
and adds that directory to your user `PATH` (via `postinstall`). Restart your
terminal afterwards so the new `PATH` takes effect. `npm uninstall -g win-nice`
reverses it (via `preuninstall`).

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

`uninstall`/`reinstall` only remove files that still carry the
`win-nice: managed-file` marker comment — if you've edited one of the installed
scripts yourself, it's left alone (reported as skipped) instead of being
silently overwritten. File tracking is manifest-first
(`%LOCALAPPDATA%\win-nice\install-manifest.json`), with a marker-comment scan
of the install directory as a fallback if the manifest is missing or corrupt.

## Requirements

Windows 8 / Server 2012 or newer (Job Object CPU rate control). PowerShell is
bundled with Windows — no separate install needed to run the tools. Node.js is
only needed for the npm-based installer/tests, not for `idle`/`cap`/`uiup`
themselves.

## Testing

```
npm test                                       # installer logic (fast, no side effects)
powershell -Command "Invoke-Pester -Path test\win-nice.Tests.ps1"   # real tool behavior
```

The Pester suite is an integration suite: it spawns real processes, checks
actual `PriorityClass` and Job Object CPU throttling, and takes ~20-30s. It
never touches the real system `PATH`/registry; the installer tests use
`WIN_NICE_HOME` to redirect installs into a temp directory instead.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache License, Version 2.0](LICENSE-APACHE),
at your option.
