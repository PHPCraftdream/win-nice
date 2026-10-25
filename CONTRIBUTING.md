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

## Reporting bugs

Open a GitHub issue. Include your Windows version, `wnice -V`, the shell you
called it from, and the exact command that misbehaved. For anything
resembling a security issue (privilege escalation, argument injection beyond
what's documented), see [SECURITY.md](SECURITY.md) instead of a public issue.
