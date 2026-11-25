# win-nice — workspace decomposition and affected-test CI plan

Date: 2026-11-15. Status: CANCELLED by the owner on 2026-11-15, nothing was started. The goal for win-nice was only to add the missing capabilities (`--wsl`, tree-wide HIGH without elevation) and to publish 0.4.0; both were done in the single crate. Kept for reference only; the sections below describe a plan that is not being executed.

## Why

Today `wnice` is one crate (lib + bin, about 4.8k lines of Rust) plus the npm
installer (about 3k lines of Node). The planned 0.4.0 work adds a WSL backend,
a job-notification watcher for tree-wide HIGH priority, `MIN_MAX_RATE` CPU
reservation and possibly a priority broker. These have different test needs
(pure logic, real Win32 processes, real WSL, an elevated session), so keeping
everything in one crate makes every change pay for every test and spreads
`unsafe` and security-relevant code across the tree.

Current pressure points:

- `src/limits.rs` is 945 lines against the 1000-line cap; about 650 of them are inline tests.
- `src/run/tests.rs` is 763 lines; `tests/integration.rs` is 702 lines in one test binary.
- Win32/`unsafe` code lives in three places: `src/lib.rs` (`RealSys`), `src/front/native.rs`, `src/limits.rs`.

## Target workspace

All internal crates are `publish = false`; the shipped artifact stays one
`wnice.exe` distributed through npm. No third-party dependency is added (the
"no dependencies" property is preserved; internal crates do not change it).

| Crate | Contents | Win32 / `unsafe` | Where it is tested |
|---|---|---|---|
| `wnice-core` | `Config`, `PriorityClass`, `MemSpec`, CLI and legacy-tool parsing, argv quoting, limit composition and the field-by-field byte layout of Job Object structs | none | ubuntu, unit tests |
| `wnice-sys` | `Sys` trait, `RealSys`, `Handle`, `native` (elevation, ShellExecuteEx, toolhelp); `fakesys` behind a `fake` feature | all `unsafe` of the project | windows |
| `wnice-run` | the `run()` engine (job, launch, wait, timeout, release); the future job-notification watcher | through `Sys` | windows: fakesys plus real processes |
| `wnice-wsl` | pure flag → `systemd-run` translation, POSIX quoting, path translation; a thin `wsl.exe` caller | none | ubuntu unit tests; real WSL only locally behind an env flag |
| `wnice-broker` (only if task W4 is done) | protocol (framing, HMAC via Windows CNG — no hand-written crypto, no new dependency); pipe server with an owner-only ACL | yes | protocol on ubuntu; server in the elevated suite, local only |
| `wnice` (bin) | thin multicall dispatcher and frontends (`help`, `admin`, `uiup`, legacy tools) | none | windows: Pester and smoke |

Finer splitting (a crate for argv quoting, a crate for help texts) is not
planned: it adds noise without isolation or test-selection benefit.

## In-crate splits

- `limits.rs`: move tests to `limits/tests.rs`; separate limit composition (moves to core) from the Job Object byte layout.
- `tests/integration.rs`: split into test binaries by behavior (`limits_*`, `timeout`, `priority`, `launch_quoting`) so test selection picks a binary, not the whole suite; keep the self-re-executing probe (`tests/probe_support.rs`) as a shared module.
- `run/tests.rs`: split by engine phase (job creation, launch, wait/timeout, release).

## Affected-test matrix

Rule: changed paths → changed crates → reverse dependencies from the
`cargo metadata` graph → jobs.

| Change | Jobs |
|---|---|
| `crates/wnice-core/**` | everything (all crates depend on core) |
| `crates/wnice-wsl/**` | wsl unit tests on ubuntu + exe build + smoke; no Pester, no elevated suite |
| `crates/wnice-broker/**` | broker unit tests on ubuntu + exe build; elevated suite flagged "manual run required" |
| `crates/wnice-run/**`, `crates/wnice-sys/**` | windows integration tests + Pester |
| `install/**`, `test/install/**` | `npm test` + `release-check` only; no cargo (exe from the master artifact cache) |
| `docs/**`, `*.md` | `docs-sync` + `lint:structure` |
| `.github/**`, `Cargo.lock`, toolchain files | full run |
| push to master, tags, nightly | always the full run (safety net against planner bugs) |

Requirements:

- One stable aggregate check (`ci-ok`) that branch protection can require; a dynamic matrix cannot provide a fixed check name.
- The planner is a small Node script with its own tests on synthetic diffs.

Expected benefit, honestly: win-nice CI is already short. Savings come mainly
from installer-only and docs-only PRs skipping the Rust build, and from pure
crates running on cheaper ubuntu runners. The main value of the split is
isolation: all `unsafe` in one crate, the broker (a security boundary) small
and separately auditable, and no Win32 code in the WSL path.

## Tasks and order

The W3 (`--wsl`) and R1 (CI hardening) agents are already working inside the
current single crate; starting the move now would only create conflicts.

1. Let W1 (no-UAC spike), W3 and R1 finish. W3 adds new modules; moving them later is mechanical.
2. **S1 — workspace split.** Pure move with a hard "behavior does not change" boundary: the same tests are green before and after, CLI output and exit codes are unchanged, `wnice.exe` passes Pester unchanged. Includes the in-crate splits above.
3. **S2 — affected-test planner.** Dynamic matrix, `ci-ok` aggregate, ubuntu jobs for pure crates, full run on master/tags/nightly.
4. W2 (job-notification watcher, `MIN_MAX_RATE`) and W4 (broker, only if W1 shows it is needed) are implemented directly in the new crates.
5. Then R2 (supply chain), R3 (docs and release prep), R4 (release — only on the owner's explicit decision).
