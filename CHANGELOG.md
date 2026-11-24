# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `wnice --audit` reports job accounting and same-user process suspects after
  the command ends. Legacy tools can opt in with `WNICE_AUDIT=1`; suspects are
  based on validated parent chains and are not proven leaks.

- `wnice --no-servers`: opt-in server-reuse settings for the launched command.
  It preserves user-set environment values, disables MSBuild node reuse,
  Roslyn shared compilation and Gradle daemons, and assigns a free loopback
  port to sccache when unset. It does not affect servers already running.

- `wnice --jobs`: a read-only report of the wnice-family Job Objects the
  current user's wrappers hold right now (owner process and command line,
  CPU cap, memory and process limits, affinity mask, priority class, active
  processes) plus the busiest processes with the jobs they belong to. One
  row per kernel job object, found through the system handle table,
  duplicated with query-only rights and closed on every path; nothing is
  modified and no elevation is needed. The owner's user SID is checked
  before anything else is read from that process, so foreign owners are
  only counted. Every value a query could not establish is shown as `?`,
  and a chain verdict (`UNCAPPED`, `NOT-IN-WNICE-JOB`) is given only when
  the inventory is complete - otherwise the fact carries an explicit
  `- inventory incomplete` qualifier and a one-line legend explains it.
  A nested job (e.g. `capc 50 capm 50 ...`) shows its rate as a share of
  its parent job's allowance, never of the whole machine: the immediate
  parent is named when the answers prove it, otherwise the row shows it as
  `nested (parent ?)`. `--jobs` is decided before any other argument is
  interpreted and is accepted only as the whole command line: any other
  argument before the first `--` - a command, an option, a help or version
  flag, an invalid option value, or a repeated `--jobs` - is a usage error
  with exit 2 and a single line on stderr, so `wnice cmd --jobs` is that
  error too, while a `--jobs` at or after the first `--` is an ordinary
  argument of the wrapped command (`wnice -- cmd --jobs`). `wnice --help`
  is the only place it is documented; only invocations whose arguments
  contain `--jobs` change, and every other invocation behaves exactly as
  before.

- `admin --keep-limits <command> [args...]`: opt-in, keeps the limits of the
  job `admin` itself runs in when it has to elevate. A UAC-elevated process is
  created by the AppInfo service, not as a child of `admin`, so it never
  enters the caller's job: `capc 10 admin build.bat` used to run the elevated
  command with no CPU cap, memory limit or process limit. With the flag,
  `admin` reads the limits of the caller's immediate job
  (`QueryInformationJobObject` with a NULL job handle) and elevates this
  executable's `wnice` front instead of the command, as
  `wnice <options> -- <command> [args...]`: `-c` CPU rate percent, `-m` job
  memory limit, `-n` active-process limit, `-t` affinity (only when the mask
  is a contiguous run from CPU 0) and `-p` priority class. The elevated
  `wnice` creates its own new job, so the limits live in a separate pool,
  never in the caller's job. A hard cap with a fractional percent is rounded
  down to a whole percent (never below 1), with one stderr note naming both values; every
  limit that cannot be kept (a CPU rate control that is not a hard cap, a
  sparse affinity mask, an unknown priority class) gets its own stderr line
  and is left out, and the rest is still kept. A job with no such limit - or
  no job at all - prints one note and falls back to plain `admin`. If the
  limits cannot be read, or the sibling `wnice.exe` is not this executable's
  front (the same file, or the installer's byte-identical copy), `admin`
  refuses to elevate with exit 1 before the UAC prompt instead of running
  unlimited. Not kept by design: the wall-clock timeout (it lives
  in the wrapper process, not in the job) and the caps of outer jobs when the
  caller's job is nested. Already elevated, the flag changes nothing.
  `admin <command>` without the flag, the `admin` usage text and every exit
  code are unchanged; only the `admin --help` text grew, which is the
  differential allowlist's second entry.

- `WNICE_AGENT_LIMITS` (optional): limits for the agents `cy` and `cx`
  start. When set, its value is whitespace-separated `wnice` limit options
  (`-p/--priority`, `-c/--cpu`, `-t/--threads`, `-m/--memory`, `-n/--procs`,
  `-s/--timeout`, `--boost`) and the agent runs under exactly the job
  `wnice <those options> -- claude ...` (resp. `codex ...`) would create: one
  job with those limits around the agent and everything it starts, e.g.
  `set WNICE_AGENT_LIMITS=-p idle -c 10 -m 10g` once per shell instead of
  editing every `cy` command. No quoting rules beyond whitespace separation,
  and the value goes through the same parser and validation as the `wnice`
  command line, so values and error wording cannot drift. A command or any
  other non-option token, `--`, `--wsl`, `--jobs`, `--help` and `--version`
  are refused: one stderr line naming the variable with the reason, exit 1,
  nothing started. Unset, empty or whitespace-only is exactly the 0.4.0
  behaviour (`cy`/`cx` stdout, stderr, exit code and the job they create are
  unchanged), and `wnice`, the legacy tools, `admin` and `uiup` never read
  the variable.

### Changed

- Internal `wnice --jobs` edge cases now flag partial process enumeration, avoid naming unproven nested parents, and count failed owners once in footer summaries.

- Documentation only: README.md and skills/win-nice/SKILL.md now state what
  `-c` measures (N percent of all logical processors, one hard cap per job -
  not a share of one core and not a machine-wide pool), list what Job Object
  limits do not cover (reused daemons, service-created processes, VM work,
  work done by other processes, deliberate handle duplication) and document
  `wnice --jobs`, `admin --keep-limits` and `WNICE_AGENT_LIMITS`. No code, CLI
  or version change.

### Internal

- Backward-compatibility safety net for the published CLI contract: a `cargo test` suite with committed `--help`/usage goldens, exact message and exit-code checks and the Job Object lifecycle (`tests/compat.rs`), plus a CI differential gate that replays a fixed command set against the published `win-nice@0.4.0` binary (`scripts/compat-differential.js`; its allowlist gained its first entry with `--jobs`). No runtime, CLI or version change.
- Job Object containment regression suite (`tests/job_containment.rs`): no breakaway from the tree's job, a nested job cannot loosen the outer one, and the wrapper's job handle is not inheritable; plus strict probe-report parsing, atomic scratch-dir creation and a kill-on-drop survivor guard in `tests/compat.rs`. No runtime, CLI or version change.
- Job Object pool-probe coverage (`tests/job_containment.rs`): named-job membership and queried CPU-cap observations. Pinned finding: a process in an unrelated job tree cannot join a named pool job that is nested in another tree (`ERROR_ACCESS_DENIED`, measured on windows-2022 and windows-2025), so one shared CPU pool across independent terminal tabs is not possible with Job Objects alone; processes of one job chain do share a pool. No runtime, CLI or version change.

## [0.4.0] - 2026-11-15

Two feature additions - a new `--wsl` mode and the priority booster - plus
repository hygiene and release engineering. There are no breaking CLI
changes: every tool name, option and exit code of 0.3.0 behaves exactly as
before, and both new modes are opt-in per invocation (`--wsl`, `--boost`).
The installer logic is untouched and the runtime dependency count stays
zero. Upgrading from 0.3.0 is an ordinary `npm install -g win-nice`; the
only observable interface change besides the new flags is the wording of
the un-elevated above-normal stderr note (see Changed below - scripts
matching the old `root process only` wording verbatim need a one-line
update).

### Added

- `wnice --wsl[=<distro>] [options] -- <command> [args...]`: run the command
  inside a WSL2 distro as its default user, under a transient systemd scope
  started by root. The wnice flags map to cgroup-v2 properties in the guest
  (`-c` CPUQuota, `-p` nice/CPUWeight, `-m` MemoryMax, `-t` AllowedCPUs,
  `-n` TasksMax, `-s` RuntimeMaxSec + cgroup-wide SIGKILL, exit 124).
  Every requirement is prechecked up front with a clear error (WSL2,
  systemd as PID 1, cgroup v2, systemd-run, default user, guest-visible
  cwd); the Windows cwd is translated verbatim to `/mnt/<drive>/...`
  (UNC paths are refused); `-p realtime` is refused loudly (the Linux
  nice/weight model has no realtime band). Windows mode is unchanged;
  arguments travel as a raw argv
  array with no shell on either side, so the cmd.exe `%` refusal does not
  apply.
- `wnice --boost` (requires `-p/--priority`): skip the job priority-limit
  probe and apply the priority class to every process of the tree via the
  booster. Documented in `wnice --help`, README.md ("Priority above NORMAL
  without elevation") and `skills/win-nice/SKILL.md`.
- The booster itself: the job is associated with an I/O completion port and a
  watcher thread consumes its notifications, opening each new process
  (`PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION`) and
  calling `SetPriorityClass` on it. The new PID arrives in the completion
  packet's `lpOverlapped` value on `JOB_OBJECT_MSG_NEW_PROCESS` — verified in
  the W1 no-UAC spike (Windows 10 19045, un-elevated: 92/92 processes
  boosted, median 0.004 ms per process), which found it differs from the old
  MSDN wording. Per-process booster errors (process already exited, access
  denied) are ignored and never change the exit code.
- CI runs the build/test matrix on explicit `windows-2022` and `windows-2025`
  runner images (`fail-fast: false`) instead of the floating `windows-latest`
  image only.
- An MSRV gate: `Cargo.toml` now declares `rust-version = "1.87"` and CI runs
  `cargo check --locked --all-targets` with exactly that toolchain (floor:
  `u32::is_multiple_of` needs 1.87, `#[expect]` needs 1.81, Cargo.lock's v4
  format needs cargo >= 1.78, and windows-sys declares 1.60).
- `cargo-deny` (advisories, license allow-list, duplicate/source bans) via
  `deny.toml` and a CI job.
- Dependabot for cargo, npm and GitHub Actions (weekly, grouped updates;
  SHA-pinned actions keep their pins).
- OpenSSF Scorecard analysis (weekly + push, SARIF to code scanning) and a
  zizmor-based GitHub Actions security audit.
- `cargo doc` with `RUSTDOCFLAGS=-D warnings` as a CI job.
- CODEOWNERS and SUPPORT.md.
- The publish pipeline (`.github/workflows/publish.yml`) covers the whole
  release chain with ONE build: `wnice.exe` is built once (pinned release
  toolchain), the npm tarball is packed once (`npm pack`),
  `rust-deps.cdx.json` (Rust dependencies from `Cargo.lock`, via
  cargo-cyclonedx) and `npm-deps.cdx.json` (npm package, via built-in
  `npm sbom`) are written, the four build outputs are covered by
  `SHA256SUMS`, all five files are attested as GitHub artifact attestations,
  a draft GitHub Release is created with the text extracted from the
  matching `CHANGELOG.md` section (re-runs recreate a stale draft), and
  `npm publish --provenance` runs LAST with that exact tarball - so the
  registry package and the Release assets are the same bytes, and a failure
  before the publish step publishes nothing. `workflow_dispatch` rehearses
  the pipeline on a branch through `npm publish --dry-run`.
- docs/VERIFYING.md, a user-facing guide for tracing every release artifact
  and the npm package back to a git commit (artifact attestations,
  `SHA256SUMS`, npm provenance, the signed release tag), and docs/RELEASING.md,
  the maintainer release procedure including signed tags and the
  draft-release verification checklist.
- A CI `repro` job: two recipe variants, each built twice from clean trees
  at different absolute paths - a control without `/Brepro` and the release
  recipe with `-C link-arg=/Brepro` - compared by SHA-256, with differing-byte
  counts and offsets printed on a mismatch. Measured on one CI run (commit
  bd4e365): the control diverged, the `/Brepro` recipe was bit-identical.
  Informative (non-blocking); the verdict lands in the job summary.

### Changed

- Un-elevated above-normal priorities (`-p abovenormal|high|realtime` and the
  `abovenormal`/`high`/`realtime` tools) now start the booster when the job
  refuses the tree-wide priority limit (Win32 error 1314) instead of setting
  the class on the root process only. The root-only fallback remains for
  below-normal priorities (job limits need no privilege there) and as
  graceful degradation when booster setup itself fails; only `--boost` turns
  a setup failure into a hard error (exit 1). The stderr note changed from
  `wnice: note: high priority applies to the root process only ...` to
  `wnice: note: high priority applies to every process in the tree via the
  booster (job completion port + SetPriorityClass, not a job limit); boost is
  usually sub-millisecond (up to a few ms in spawn bursts), so very
  short-lived children may exit before it lands`.
- CI workflows declare least-privilege `permissions` (`contents: read`,
  raised only where code-scanning uploads need it), cancel outdated
  pull-request runs via `concurrency`, cap every job with `timeout-minutes`,
  and drop the checkouts' persisted git credentials
  (`persist-credentials: false` - nothing in CI pushes).
- The publish workflow builds `wnice.exe` with a pinned toolchain (1.97.0),
  `--remap-path-prefix` and `-C link-arg=/Brepro` - the recipe ci.yml's
  `repro` job mirrors and measured (commit bd4e365): without `/Brepro` the
  builds differed, with it they were bit-identical within one CI run;
  bit-identity is not claimed beyond that scope. ci.yml's other jobs keep
  tracking floating stable on purpose.

## [0.3.0] - 2026-10-25

All 14 tools are now one native Rust executable, `wnice.exe`, built in CI
and shipped in the npm package. Nothing is compiled on the user's machine, and
the tools no longer need PowerShell, .NET or Node.js to run.

### Breaking changes

- The `bin/<tool>.ps1`, `bin/<tool>.bat`, `load-launcher.ps1` and the 14 prebuilt
  helper DLLs are gone. Every tool is `<tool>.exe`, a hardlink of
  `wnice.exe` that picks its behavior from its own name; Git Bash gets a
  generated extensionless shim next to it. Scripts that called `<tool>.ps1`
  or `<tool>.bat` by full name must call `<tool>` (or `<tool>.exe`).
- Bare command names are resolved from absolute `PATH` entries only, never
  from the current directory. A name found nowhere exits `127` instead of
  falling back to `cmd.exe`, so cmd.exe builtins must be wrapped explicitly
  (`idle cmd /c dir`).
- Priority is now a Job Object limit for the whole process tree:
  `abovenormal`, `high` and `realtime` now also apply to the wrapped
  command's children. Without `SeIncreaseBasePriorityPrivilege` the job
  refuses `high`/`realtime` (Win32 error 1314); the tool then sets `HIGH` on
  the root process only and says so on stderr.
- The timeout message reads `wnice: timed out after <seconds>s - ...`
  (was `caps: ...`). Exit code `124` is unchanged.
- `cy`, `cx` and an already-elevated `admin` run the target inside a Job Object too (no limits,
  kill-on-close released on a normal exit).

- The npm installer requires Node.js 24 or newer (`engines.node` is now
  `>=24`, the only version CI tests). The tools themselves need no Node.js.

### Added

- `wnice [options] [--] <command> [args...]`: every limit at once
  (`-p` priority, `-c` CPU cap, `-t` threads, `-m` memory, `-n` process count,
  `-s` timeout) in one wrapper process and one Job Object, so the process
  tree is `wnice.exe -> command` with no `cmd.exe`/`powershell.exe` layers.
- Built-in help with examples: `wnice --help` and `<tool> --help` (also `-h`,
  `/?`) for every tool except `cy`/`cx`, which pass it on to `claude`/`codex`;
  usage errors point to it. `win-nice help` lists the installer commands and
  environment variables.
- Upgrades work while a tool is running: in-use executables are renamed aside
  (`<tool>.exe.old-<n>`) and removed later; the whole upgrade rolls back if
  any step fails.

### Changed

- The installer writes `wnice.exe`, 14 hardlinks and 15 shims (a copy where
  hardlinks are unsupported); the published package contains 14 files and one
  binary.
- Upgrading over a 0.2.x install (`npm install -g win-nice`) replaces its
  shims and removes its `.ps1`/`.bat`/DLL files, with or without the old
  install manifest; files without the `win-nice: managed-file` marker are
  never touched.
- PATH change notifications no longer need a helper assembly.
- Tests run inside a developer sandbox (`scripts/lib/guarded.ps1`: hidden
  desktop, process cap, Idle priority, timeout), so they never open windows;
  the Pester suite now drives the installed exes and takes seconds instead of
  minutes. A structure linter caps code files at 1000 lines and code
  directories at 7 entries.

### Removed

- `scripts/build-launcher-assemblies.ps1`, `scripts/check-assemblies.js`,
  `scripts/EnvironmentNotifier.cs` and the C# fault-injection Pester harness
  (replaced by fault injection through the Rust `Sys` trait).
- The `PowerShell execution policy` requirement: there are no scripts to run.

## [0.2.2] - 2026-09-27

### Changed

- `npm test`, `npm pack`, and `npm publish` no longer invoke the C# compiler
  or rewrite the committed helper assemblies. Each `bin/<Class>.dll.managed`
  sidecar now records the SHA-256 of the C# source the DLL was built from;
  `scripts/check-assemblies.js` (run by `npm test` and as `prepack`)
  recomputes it without a compiler and fails, naming the assembly, when a
  committed DLL is stale or missing. The published tarball therefore ships
  exactly the committed DLL bytes. `scripts/build-launcher-assemblies.ps1` is
  incremental (`-Force` rebuilds everything).
- The list of the 14 helper assemblies is defined once in `install/install.js`
  and used by the installer, `release-check`, and the freshness check; a test
  keeps the build script's own table in sync with it.
- After an upgrade, a PowerShell session that had already loaded a helper
  assembly prints one warning that the DLL changed on disk and keeps using the
  loaded version until a new session starts.
- CI runs `idle`, `caps`, and `capc` under PowerShell 7 (`pwsh`) as a smoke
  test of the prebuilt .NET Framework assemblies there.

### Removed

- Unused `scripts/build-idle-launcher.ps1`.

## [0.2.1] - 2026-09-25

### Changed

- Replaced per-launch C# compilation with prebuilt .NET helper assemblies and a shared PowerShell loader. `npm test` and `npm pack` regenerate the assemblies from the embedded launcher sources.
- The installer now manages the helper assemblies and marker sidecars; PATH notifications use the prebuilt `EnvironmentNotifier` helper.
- Expanded the release artifact allowlist to cover the loader, helper assemblies, and their marker sidecars.
- Made the Pester CPU-saturation test opt-in with `WIN_NICE_RUN_CPU_STRESS_TEST=1` so routine runs do not saturate every logical processor.

### Fixed

- Prevented Git line-ending conversion of helper DLLs and made fallback cleanup remove assemblies before their marker sidecars.

## [0.2.0] - 2026-09-04

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

- Fixed executable search-order hardening for Windows system helpers: all
  PowerShell-delegating `.bat` wrappers, the installer, and the `admin`/`uiup`
  UAC paths now launch Windows PowerShell or `cmd.exe` by an absolute system
  path. `admin` also carries the absolute path returned by `Get-Command` into
  its direct elevated launch instead of resolving the original bare name a
  second time through ShellExecute. This prevents a same-named executable in
  the caller's current directory from replacing the intended process,
  including at the elevation boundary.
- `capt` under 32-bit Windows PowerShell now rejects thread counts above the
  32-bit affinity-mask width with an actionable usage error instead of
  reaching a `UIntPtr` conversion and reporting a raw overflow exception.
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

[Unreleased]: https://github.com/PHPCraftdream/win-nice/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/PHPCraftdream/win-nice/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/PHPCraftdream/win-nice/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/PHPCraftdream/win-nice/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/PHPCraftdream/win-nice/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/PHPCraftdream/win-nice/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/PHPCraftdream/win-nice/releases/tag/v0.1.0
