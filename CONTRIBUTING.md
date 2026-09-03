# Contributing

Thanks for considering a contribution. This is a small, dependency-free Windows
utility - keep changes proportionate to that scope.

## Setup

No build step. Clone, then:

```
npm install    # no dependencies - triggers postinstall, which detects a
                 source checkout and no-ops (nothing to set up)
npm test       # Node test suite - installer/skill-installer logic, fast
powershell -Command "Invoke-Pester -Path test\win-nice.Tests.ps1"   # real tool behavior, ~1-2 min
```

3 cases in the Pester suite only exercise `admin.ps1`'s already-elevated branch,
which needs the whole test-runner process (not just `admin.ps1` itself) to
already be running elevated - an unelevated run reports them `Skipped`, not
failed. A different, disjoint 4 cases only make sense when NOT elevated and
`Skip` under elevation instead. `npm run test:elevated`
(`test/run-elevated.ps1`) asks for elevation once via the standard UAC prompt,
then runs the suite inside that elevated session, activating the first group
(and skipping the second) - relaying the result back to this console. Run both
a normal and an elevated pass for full coverage; neither alone exercises every
case.

The Pester suite spawns real processes and checks actual priority class, Job
Object CPU throttling, and affinity - it needs Windows and only uses the
Pester 3.4.0 that ships with Windows PowerShell 5.1 (no install needed; if a
newer Pester is also installed, pin the version - see the Testing section in
README.md for the exact command CI uses).

Neither suite touches your real system PATH/registry by default; the Node
tests redirect installs via `WIN_NICE_HOME` and registry tests use a scratch
key under `HKCU:\Software\WinNiceTest`.

## Before opening a PR

- Both test suites pass.
- New behavior has a test. A bug fix should include a regression test that
  fails without the fix.
- Doc changes: if you touch the `%`-argument-safety behavior, keep
  `README.md`'s "Argument handling" section and `skills/win-nice/SKILL.md`'s
  "Argument safety" section in sync - `test/docs-sync.test.js` checks this and
  will fail if they drift.
- No new runtime dependencies without a strong reason - "no dependencies" is a
  deliberate project property, not an oversight.

## Design conventions worth knowing before you dig in

- Every `bin/<tool>` ships as up to three entry points: `.bat` (cmd.exe/PATHEXT
  callers), `.ps1` (PowerShell bare-name resolution, the fully argument-safe
  path), and an extensionless POSIX shim (Git Bash). See README's "Argument
  handling" section for why there are three and what each one guarantees.
- Each `.ps1`'s embedded C# class needs a name unique across the whole `bin/`
  directory (`Add-Type` throws "type already exists" if two tools share a
  class name and both run bare-name in the same PowerShell session - this bit
  us once, see the sequential-invocation regression test).
- Managed files (everything under `bin/`, plus the installed skill) carry a
  `win-nice: managed-file` / `win-nice: managed-skill` marker comment - the
  installer relies on it to know what it's allowed to overwrite/remove.

## Reporting bugs

Open a GitHub issue. Include your Windows version, PowerShell version
(`$PSVersionTable`), and the exact command that misbehaved. For anything
resembling a security issue (privilege escalation, argument injection beyond
what's documented), see [SECURITY.md](SECURITY.md) instead of a public issue.
