# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.2.0] - 2026-09-02

### Changed

- `cap` renamed to `capc`; `pint` renamed to `capt` (breaking - the old names
  no longer exist).
- Every launcher's best-effort `TerminateProcess` kill (triggered when
  `WaitForSingleObject`/`ResumeThread`/`AssignProcessToJobObject` fails) now
  checks its own result too. If the kill itself also fails, the thrown error
  says so explicitly instead of silently treating a failed kill the same as a
  successful one.

## [0.1.1] - 2026-09-02

### Added

- `capm` - hard memory ceiling via Job Objects; `<size>` accepts a bare
  integer `1`-`100` (percent of total physical RAM, same convention as
  `cap`'s own `<percent 1-100>`), or `m`/`M` (MB), or `g`/`G` (GB).
- These tools can be chained by name (e.g. `capm 50 cap 50 idle <command>`)
  - each wrapper's Job Object nests inside the outer one (Windows 8+). Nested
  limits do *not* uniformly take the smaller value: CPU rate (`cap`)
  multiplies relative to its parent (`cap 50 cap 50` ≈ 25%, not 50%); memory
  (`capm`) ceilings apply independently to accounting scopes that aren't the
  same size (a parent job's accounting includes every child job's committed
  memory plus its own process, so nested `capm` ceilings don't reduce to a
  simple minimum). See README's "Chaining these tools together" section for
  the full picture.

### Changed

- Every launcher's embedded native-process primitive now checks `ResumeThread`/
  `WaitForSingleObject`/`GetExitCodeProcess` return values instead of assuming
  success, and `AllocHGlobal`/`FreeHGlobal` around each Job Object limit
  struct is wrapped in `try`/`finally`. On a `ResumeThread` or
  `WaitForSingleObject` failure, the launcher now attempts to terminate the
  child instead of either waiting on a still-suspended process forever or
  reporting failure while it may still be running unmanaged in the
  background. Hardening for a class of rare Win32 failures - not a fix for
  an observed regression.

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
[0.2.0]: https://github.com/PHPCraftdream/win-nice/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/PHPCraftdream/win-nice/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/PHPCraftdream/win-nice/releases/tag/v0.1.0
