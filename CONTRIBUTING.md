# Contributing

Thanks for considering a contribution. This is a small, dependency-free Windows
utility - keep changes proportionate to that scope.

## Setup

The tools are one Rust binary (`src/`, `Cargo.toml`; `windows-sys` is the only
dependency). The npm side (`install/`) is only the installer. You need a Rust
toolchain and Node.js:

```
cargo test            # Rust unit tests + tests/integration.rs (real processes, Job Objects)
npm run build:exe     # cargo build --release --locked, then stages bin/wnice.exe
npm install           # no dependencies - postinstall detects a source checkout and no-ops
npm test              # installer/skill-installer logic + Git Bash shims, fast
npm run test:pester   # the installed tools driven from PowerShell, a few seconds
```

`bin/wnice.exe` is never committed: CI builds it, and `npm pack` refuses to
run without it. End users never compile anything.

### WSL integration tests (manual, real WSL2 required)

`tests/wsl_integration.rs` launches a real distro, so it is skipped unless
`WNICE_TEST_WSL=1` is set — CI (the windows-2022/windows-2025 matrix)
stays green without it. The flag-to-systemd-property mapping itself
(`src/run/wsl`) is pure logic covered by ordinary `cargo test` without
WSL - only the probe/integration flow needs a real distro.
Run it manually on a WSL2-capable machine:

```
set WNICE_TEST_WSL=1
cargo test --locked --test wsl_integration
```

`WNICE_TEST_WSL_DISTRO=<name>` optionally selects which distro to use
(otherwise the default one). The machine needs: WSL2 (WSL1 refused),
systemd as PID 1 (`[boot] systemd=true` in `/etc/wsl.conf`), cgroup v2,
`systemd-run`, `runuser`/`nice`, and the checkout on a drive visible in the
guest (`/mnt/<drive>/...`), since the tests run from the translated current
directory.

Both npm test scripts run inside `scripts/lib/guarded.ps1`: a private hidden desktop
(no test window ever reaches the screen), one Job Object with a live-process cap
(100 for `npm test`, 60 for Pester), Idle priority for the whole tree, a
wall-clock timeout (exit 124) and kill-on-exit for leftover processes. Output is
printed when the run ends, not streamed. A "Not enough quota" failure means a
test spawned more processes than the cap - treat it as a bug, not a reason to
raise the cap. Run `cargo test` the same way when you can:
`powershell -File scripts\lib\guarded.ps1 25 900 cargo test --locked`.

Tests never open windows and never prompt for UAC. Rust test spawns use
`CREATE_NO_WINDOW` (or `DETACHED_PROCESS` for an intentional daemon); Node
child processes get `windowsHide: true` from `scripts/lib/hide-windows.js`.
The integration harness (`tests/integration.rs`, `harness = false`) re-executes
itself as a probe; `WNICE_PROBE_DEPTH` caps the probe generations so a bug
cannot turn into a process flood.

The guard pins every process to Idle, so the priority cases skip there; the
integration harness treats any skip as a failure when `CI=true`, so they must
pass in CI. One Pester case covers `admin`'s already-elevated branch and needs
the whole runner elevated: `npm run test:elevated`
(`test/pester/run-elevated.ps1`) asks for elevation once and runs the suite in
that session. Run both a normal and an elevated pass for full coverage.
The booster's own unit tests (fake `Sys` handles, real child processes under a
plain un-elevated run) are part of ordinary `cargo test` and skip nowhere.

Neither suite touches your real system PATH/registry by default; the Node
tests redirect installs via `WIN_NICE_HOME` and registry tests use a scratch
key under `HKCU:\Software\WinNiceTest`.

## Before opening a PR

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, `npm test` and `npm run test:pester` pass.
- New behavior has a test. A bug fix should include a regression test that
  fails without the fix.
- Doc changes: if you touch the `%`-argument-safety behavior, keep
  `README.md`'s "Argument handling" section and `skills/win-nice/SKILL.md`'s
  "Argument safety" section in sync - `test/docs-sync.test.js` checks this and
  will fail if they drift.
- No new runtime dependencies without a strong reason - "no dependencies" is a
  deliberate project property, not an oversight.

## Compatibility contract

Everything a released version exposes is contract: the tool names and their
argument syntax, the `wnice` options (including `--wsl` and `--boost`), the
exit codes (`<n>` passed through unchanged, `124` timeout, `127` command not
found, `1` usage or wrapper error, negative NTSTATUS values bit for bit), the
exact stderr text, and the `--help` output of every tool. New features must be
strictly additive; changing any of the above is a breaking change and needs a
deliberate decision, never a CI-green silent diff. Not contract: the installer
(covered by `npm test`), timing, README/SKILL wording, and anything that only
happens inside a WSL guest.

Two gates hold it:

1. `tests/compat.rs` (part of `cargo test`) compares the running binary with
   the goldens in `tests/compat/help/` and `tests/compat/usage/`, and pins the
   exit codes, the error messages and the Job Object lifecycle (the limits are
   on the tree's job while it runs; a child left running by a normal exit
   survives AND still reports the CPU cap itself, queried after the wrapper is
   gone, while KILL_ON_JOB_CLOSE is released; a timeout kills the whole tree
   and exits 124).
   To change a golden: run the tool, diff its output against the file, and
   commit the new file in the same commit as the intended change. `tests/compat/**`
   is pinned to LF in `.gitattributes` - do not let an editor rewrite the line
   endings.

2. `scripts/compat-differential.js` (a CI step) replays the fixed command set in
   `tests/compat/differential-cases.json` against the published binary of the
   version named in `tests/compat/reference-version.txt` and against the fresh
   build, and fails on any difference in stdout, stderr or exit code. An
   intentional difference needs an entry in
   `tests/compat/differential-allowlist.json` (`{ "case", "field", "reason" }`);
   the list is empty by default, and an entry that stops differing fails the
   run, so it cannot rot.
   - Local run: `npm run build:exe`, then
     `npm pack win-nice@$(cat tests/compat/reference-version.txt) --pack-destination <dir>`
     and
     `node --require ./scripts/lib/hide-windows.js scripts/compat-differential.js --built bin/wnice.exe --tarball <dir>/win-nice-<version>.tgz`.
   - After a release, move the reference: change the single line in
     `tests/compat/reference-version.txt` to the new version (first PR after the
     release) and drop any allowlist entry that no longer differs. Never point
     it at "latest".
   - New coverage: append one object to `differential-cases.json`. `${CMD}` and
     `${WORKDIR}` are expanded by the runner, and an optional `env` object
     overrides single environment variables for both sides (the cy/cx cases pin
     `PATH` and `PATHEXT` to a fake agent CLI), so a case never depends on the
     caller's PATH, `PATHEXT`, or a temp path. An optional `expect` block
     (`exit`, `stdoutIncludes`, `stderrIncludes`) is checked against BOTH
     builds: a case that silently took another path - a stand-in that was not
     found, say - fails instead of matching.

## Design conventions worth knowing before you dig in

- One multicall binary: `src/main.rs` dispatches on the argv[0] file stem.
  Help texts live in `src/front/help.rs`; keep them in step with README when
  an option or default changes.
  `wnice` is the combined frontend (`src/cli.rs`); `src/front/` holds the
  single-purpose ones. They all build a `Config` and call `run::run`, the one
  place that creates the Job Object, launches the command and waits.
- Every Win32 call `run` makes goes through the `Sys` trait (`src/lib.rs`), so
  `src/run/tests.rs` can inject a failure into any call and check that every
  handle is closed exactly once.
- Job Object structs are serialized field by field (`limits::extended_bytes`),
  never by viewing a padded `repr(C)` struct as raw bytes.
- Bare command names are resolved by `launch::resolve_bare_name` from absolute
  PATH entries only - never from the current directory.
- The WSL engine (`wnice --wsl`) lives in `src/run/wsl` as pure mapping
  logic (`compose`) plus a `WslExec` seam, so the flag → systemd-property
  mapping is unit-tested without WSL; only the probe flow touches real
  `wsl.exe`, and the integration tests exercising it are gated behind
  `WNICE_TEST_WSL=1`.
- The installer writes `wnice.exe`, one hardlink per tool name and the Git
  Bash shims; it only ever deletes or replaces files it can prove are its own
  (exact binary bytes, generated shim text, or the `wnice.exe.managed`
  sidecar). Running exes are renamed aside (`.old-<n>`), never overwritten.

## Structure rules

`npm run lint:structure` (also run by `npm test` and CI) enforces two limits:

- no code file (`.js`, `.ts`, `.ps1`, `.cs`, `.rs`, `.bat`, ...) longer than 1000 lines;
- no directory that directly contains code holds more than 7 entries (files and
  subdirectories together; untracked files that are not ignored count too).

Directories without code of their own (docs, assets) are exempt. Violations that
predate the rules are recorded in `BASELINE` in `scripts/lint/lint-structure.js`: those
sizes may shrink but never grow, and the linter prints a note when an entry can be
tightened or removed. Fix a violation by splitting the file or grouping files into
subdirectories, not by raising a limit.

## Releasing

Releases are maintainer-only: a signed `v*.*.*` tag push runs the whole
pipeline in publish.yml - one build of `wnice.exe`, attestations, a draft
GitHub Release, then `npm publish --provenance` as the last step, so the npm
package and the Release assets are the same bytes. The draft is reviewed and
published manually. The full procedure lives in
[docs/RELEASING.md](docs/RELEASING.md).

## Reporting bugs

Open a GitHub issue. Include your Windows version, `wnice -V`, the shell you
called it from, and the exact command that misbehaved. For anything
resembling a security issue (privilege escalation, argument injection beyond
what's documented), see [SECURITY.md](SECURITY.md) instead of a public issue.
