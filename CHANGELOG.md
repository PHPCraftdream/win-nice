# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.2.0] - 2026-09-03

The first release since `0.1.0` - `0.1.1` was never tagged or published;
everything below shipped together as `0.2.0`.

### Added

- `capm` - hard memory ceiling via Job Objects; `<size>` accepts a bare
  integer `1`-`100` (percent of total physical RAM, same convention as
  `capc`'s own `<percent 1-100>`), or `m`/`M` (MB), or `g`/`G` (GB).
- `caps` - wall-clock timeout wrapper: runs `<command>` for up to `<seconds>`
  and if it's still running, force-terminates it and its whole process tree in
  one kernel call (`TerminateJobObject` on the tool's Job Object), then exits
  `124` (the unix `timeout` convention) with a message on stderr. Finishes
  inside the deadline: the wrapped exit code propagates like every other
  launcher. `<seconds>` accepts a positive whole or decimal number (e.g. `2`
  or `2.5`), converted to whole milliseconds, minimum 1 ms, maximum
  `4294967294` ms (~49.7 days) - a deliberate usage ceiling, not an API limit
  (the deadline is an absolute 64-bit FILETIME, so no uint32 boundary applies)
  - anything larger is a usage error, not a
  silently truncated deadline. The deadline is absolute, not a relative wait:
  it is computed from `DateTime.UtcNow`, armed as the absolute due time of a
  one-shot waitable timer, and waited on together with the process handle via
  `WaitForMultipleObjects` (a `GetProcessTimes` check rejects an exit that
  only won the simultaneous-signal race after the deadline), so time spent in
  sleep/suspend counts against it and the timeout fires on wake if the
  deadline passed during sleep (a relative wait doesn't count sleep time on
  Windows 8+). The due time is a system-clock value, so a manual or
  service-driven system-clock adjustment during the wait can shorten or
  lengthen the actual wait relative to `<seconds>`. Unlike
  `capc`/`capt`/`capm` it sets no resource
  limit - the Job Object (with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so a
  non-cooperatively killed wrapper still takes the tree down) exists purely to
  make the timeout kill cover the whole spawned tree.
- `capn` - process-count ceiling wrapper: caps the number of simultaneously
  active processes in the wrapped command's whole tree via
  `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`. The count includes the wrapped process
  itself (it is assigned to the still-empty job before it can spawn
  anything), so `capn 1 <command>` lets the command run but fails its first
  child-spawn attempt. Exceeding the limit refuses only the offending spawn -
  nothing already running is killed or throttled, and a command within budget
  is unaffected (all confirmed empirically). `<count>` accepts a positive
  whole number, minimum 1, maximum 4294967295 (`ActiveProcessLimit` is a
  uint32 field). Like the other Job Object launchers it carries
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so a non-cooperatively killed `capn`
  wrapper still takes the whole tree down.
- These tools can be chained by name (e.g. `capm 50 capc 50 idle <command>`)
  - each wrapper's Job Object nests inside the outer one (Windows 8+). Nested
  limits do *not* uniformly take the smaller value: CPU rate (`capc`)
  multiplies relative to its parent (`capc 50 capc 50` ≈ 25%, not 50%);
  memory (`capm`) ceilings apply independently to accounting scopes that
  aren't the same size (a parent job's accounting includes every child job's
  committed memory plus its own process, so nested `capm` ceilings don't
  reduce to a simple minimum). Process count (`capn`) limits are enforced
  independently per job, and an outer job's count already includes the inner
  wrapper process itself, so nested `capn` ceilings aren't a simple minimum
  either - leave the outer value headroom for the chain itself. See README's
  "Chaining these tools together" section for the full picture.

### Changed

- `cap` renamed to `capc`; `pint` renamed to `capt` (breaking - the old names
  no longer exist).
- Every launcher's embedded native-process primitive now checks
  `ResumeThread`/`WaitForSingleObject`/`GetExitCodeProcess`/`TerminateProcess`
  return values instead of assuming success, and `AllocHGlobal`/`FreeHGlobal`
  around each Job Object limit struct is wrapped in `try`/`finally`. On a
  `ResumeThread`, `AssignProcessToJobObject`, or `WaitForSingleObject`
  failure, the launcher now attempts to terminate the child instead of
  either waiting on a still-suspended process forever or reporting failure
  while it may still be running unmanaged in the background - and if that
  best-effort kill itself also fails, the thrown error says so explicitly.
  Hardening for a class of rare Win32 failures - not a fix for an observed
  regression.
- A plain `install`/upgrade (including `postinstall`) now also refreshes an
  already-installed, still-marked `win-nice skill install` copy
  (`~/.claude/skills/win-nice/SKILL.md`, `~/.agents/skills/win-nice/SKILL.md`)
  to the new version's content. Initial skill installation is still opt-in -
  this only ever touches a copy that's already there.

### Fixed

- `uninstall` and `reinstall` run from a source checkout of this repository
  without an explicit `WIN_NICE_HOME` set now refuse to run, matching what
  `install` already did - previously they deleted a real
  `%LOCALAPPDATA%\win-nice` installation and its PATH entry, then silently
  failed to restore it.
- `capc`/`capt`/`capm`/`caps`/`capn` now set `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
  on their Job Object as a backstop for non-cooperative termination: a wrapper
  killed from outside (`taskkill` without `/T`, a crash), or a `caps` deadline
  expiring, no longer leaves orphaned grandchild processes (e.g. a linker
  spawned by a build) running unbounded outside the resource limit - Windows
  itself terminates everything still in the job at that moment. A clean,
  successful exit releases this guard first, so a daemon/compiler-server/
  watcher the wrapped command legitimately left running still survives -
  matching this same section's existing "a limit sticks to any daemon ... for
  that daemon's whole lifetime" guarantee, which an earlier draft of this
  change would otherwise have silently broken.

## [0.1.0] - 2026-09-02

### Added

- `idle`, `belownormal`, `abovenormal`, `high`, `realtime` - Windows priority
  class launchers.
- `cap` - hard CPU quota via Job Objects.
- `pint` - CPU affinity ("pin threads") via Job Objects.
- `admin` - run a command elevated, with UAC prompt when needed.
- `uiup` - one-shot desktop-responsiveness priority boost.
- `cy` / `cx` - `claude`/`codex` launchers with permission/approval bypass
  flags, for use in already-sandboxed/disposable environments.
- Each tool ships a `.bat`, `.ps1`, and extensionless (Git Bash) entry point.
- npm-based installer (`postinstall`, `win-nice status|reinstall|uninstall`),
  PATH management via the Windows registry. Uninstall is an explicit `win-nice
  uninstall` command, not an npm `preuninstall` lifecycle hook - npm >= 7
  doesn't invoke `preuninstall` on a global `npm uninstall -g`.
- Optional `win-nice skill install|uninstall` - installs a reference skill
  for Claude Code and Codex CLI.
- Node test suite (installer logic) and Pester integration suite (real
  Windows process/priority/Job-Object behavior).

[Unreleased]: https://github.com/PHPCraftdream/win-nice/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/PHPCraftdream/win-nice/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/PHPCraftdream/win-nice/releases/tag/v0.1.0
