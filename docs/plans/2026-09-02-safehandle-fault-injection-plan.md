# Implementation plan: exception-safe native handle lifetime + fault-injection tests

Date: 2026-09-02
Source: `docs/reviews/2026-09-02-1745-708cb53-release-review.md`, P3 section
("native cleanup всё ещё не exception-safe на уровне всех handles",
"новые native failure branches не имеют fault-injection tests").

Audience: an implementing agent with no prior context on this repository.
Everything needed to execute is in this file; no further design decisions are
required.

Status of the two parts: **design complete, not implemented.** No file under
`bin/` or `test/` has been touched.

---

## 0. Verification already done for this plan

Every non-obvious claim below was checked against the real environment
(Windows 10 Pro 19045, Windows PowerShell **5.1.19041.7548**, .NET Framework
4.x, `Add-Type -Language CSharp` = CodeDom C# 5 compiler) with throwaway
prototypes in `%TEMP%`. Results are quoted inline where they matter. In
particular:

- the Part 1 replacement templates in section 2 were compiled and executed
  against real child processes on both templates (success path, injected wait
  failure, injected resume failure, injected `GetExitCodeProcess` failure,
  `%`-fail-closed path, `.bat` cmd.exe-fallback path) — all produced identical
  exception messages and exit codes to today's code;
- the Part 2 harness in section 3 was prototyped end to end and produced
  `WaitForSingleObject failed: 6`, `TerminateProcess` called once, 3 distinct
  handles closed, 0 close failures.

---

## 1. Current state (verified, with exact references)

### 1.1 Two templates, twelve files

`bin/` ships 12 `.ps1` launchers. `bin/uiup.ps1` (41 lines) has no embedded C#
and is **out of scope**. The other 11 each embed one `Add-Type -Language
CSharp` class with a unique name, and fall into exactly two shapes:

| Shape | Files | Embedded class | `Run()` signature | `dwCreationFlags` |
|---|---|---|---|---|
| **A — no Job Object** | `idle`, `belownormal`, `abovenormal`, `high`, `realtime` | `IdleLauncher`, `BelowNormalLauncher`, `AboveNormalLauncher`, `HighLauncher`, `RealtimeLauncher` | `Run(uint priorityClass, string[] argv, string cmdExeCommandLine)` | `priorityClass` |
| **A — no Job Object** | `cy`, `cx`, `admin` | `CyLauncher`, `CxLauncher`, `AdminLauncher` | `Run(string[] argv, string cmdExeCommandLine)` | `0` |
| **B — Job Object** | `cap`, `pint`, `capm` | `CapLauncher`, `PintLauncher`, `CapmLauncher` | `Run(int percent, …)` / `Run(ulong affinityMask, …)` / `Run(ulong memoryLimitBytes, …)` | `CREATE_SUSPENDED` |

The 8 shape-A `Run()` bodies were diffed after normalizing the priority
parameter name: `idle`, `belownormal`, `abovenormal`, `high`, `realtime` are
**byte-identical**. `cy`/`cx` differ only by three comment lines and
`priorityClass` → `0`. `admin` differs only by the same `0`, a missing
"See cap.ps1 for why .bat/.cmd targets…" comment, a longer cmd.exe comment
block, and the flag order `/d /s /v:off` (vs `/d /v:off /s`) on a two-line
`StringBuilder` (`bin/admin.ps1:172-173`).

The 3 shape-B `Run()` bodies differ only by the limit struct/info-class/message
noun, plus one real inconsistency documented in 1.3.

**Conclusion: this is 2 templates × mechanical per-file application, not 11
bespoke changes.** Producing this as anything other than 2 templates would be
wrong — the files are already near-identical by design and drift between the
11 copies is itself a stated risk in the review.

### 1.2 How handles are managed today

Shape A (`bin/idle.ps1:123-199`): `CreateProcess` fills `PROCESS_INFORMATION
pi`; there are three exits, each repeating the same two-line cleanup —
`WaitForSingleObject` failure (`bin/idle.ps1:174-184`, `CloseHandle` at
`181-182`), `GetExitCodeProcess` failure (`bin/idle.ps1:187-193`, `CloseHandle`
at `190-191`), success (`bin/idle.ps1:195-198`).

Shape B (`bin/cap.ps1:156-294`): `hJob` is acquired at `bin/cap.ps1:158-161`,
then **seven** exits each repeat the cleanup —
`SetInformationJobObject` failure (`179-183`),
`%`-fail-closed (`217-224`),
`CreateProcess` failure (`236-240`),
`AssignProcessToJobObject` failure (`243-252`),
`ResumeThread` failure (`254-264`),
`WaitForSingleObject` failure (`266-277`),
`GetExitCodeProcess` failure (`279-287`),
success (`289-291`).

Occurrence counts of the literal `CloseHandle(` per file today
(1 of them is the `[DllImport]` declaration):

```
idle/belownormal/abovenormal/high/realtime/cy/cx/admin  =  7
cap                                                     = 18
pint / capm                                             = 19
```

`TerminateProcess` is called before the throw in exactly the branches where
the child may still be alive: shape A — wait failure only (`bin/idle.ps1:180`);
shape B — assign (`bin/cap.ps1:247`), resume (`bin/cap.ps1:259`), wait
(`bin/cap.ps1:272`). This is deliberate and recent; **do not touch it.**

`CloseHandle` is declared **without** `SetLastError = true`
(`bin/cap.ps1:107-108`); `TerminateProcess` **with**
(`bin/cap.ps1:104-105`). That asymmetry is what makes the refactor in section 2
message-preserving — see 2.4.

The managed exception surfaces to the user through each file's outer
PowerShell `try/catch`, e.g. `bin/idle.ps1:206-211`, `bin/cap.ps1:300-305`:

```powershell
} catch {
    Write-Error $_.Exception.InnerException.Message
    exit 1
}
```

### 1.3 One real inconsistency found while reading

`bin/cap.ps1:217-224` (the `%`-fail-closed branch) throws **without**
`CloseHandle(hJob)`, while the same branch in `bin/pint.ps1:212-222` (close at
`216`) and `bin/capm.ps1:316-326` (close at `320`) does close it. This is
exactly the class of drift the review predicted. It is not user-visible (the
PowerShell host exits immediately afterwards and the OS reclaims the handle),
but Part 1 fixes it as a natural consequence of a single `finally` — see the
expectation delta in section 4.2.

### 1.4 Constraints that bound both parts

- **Class-name uniqueness.** `Add-Type` throws "Cannot add type. The type name
  … already exists." if two launchers load a same-named class into one
  PowerShell session, which happens on bare-name resolution (`idle args…` runs
  `idle.ps1` in the *current* process). Documented at `CONTRIBUTING.md:45-48`;
  regression test at `test/win-nice.Tests.ps1:1347-1398`.
- **Line endings.** `.gitattributes:1-2` pins `*.bat` and `*.ps1` to
  `text eol=crlf`; `git ls-files --eol -- bin` shows `w/crlf` for every
  `.bat`/`.ps1` and `w/lf` for the extensionless shims. The working tree must
  stay that way.
- **C# 5 only.** `Add-Type -Language CSharp` on PowerShell 5.1 uses the .NET
  Framework CodeDom provider: no string interpolation, no `nameof`, no `?.`,
  no expression-bodied members. Object initializers and `var` are fine (already
  used).
- **No dependencies, minimal surface** (`CONTRIBUTING.md:36-37`,
  `README.md` throughout). Nothing may be added to the shipped `files` list in
  `package.json:24-32`.
- **Test framework.** Pester **3.4.0** syntax only: `-TestCases`, `Should Be`,
  `Should Match`, `Should Not Be`, `-Skip:`; **no** `BeforeAll`/`AfterAll`
  anywhere in `test/win-nice.Tests.ps1` by established convention.

---

## 2. Part 1 — exception-safe native handle lifetime

### 2.1 Decision: single outer `try/finally` with ownership locals. Not SafeHandle.

**SafeHandle is available** — this was verified, not assumed. Under Windows
PowerShell 5.1, `Add-Type -Language CSharp` successfully compiled and ran a
class deriving from `Microsoft.Win32.SafeHandles.SafeHandleZeroOrMinusOneIsInvalid`
with an overridden `ReleaseHandle()`; `Microsoft.Win32.SafeHandles.SafeProcessHandle`
and `SafeWaitHandle` both resolve as public types. (The transparency concern —
a security-transparent assembly overriding the `SecurityCritical`
`ReleaseHandle` — does not bite: an `Add-Type` assembly is fully trusted with no
security attributes, so its code is treated as `SecurityCritical` by default.)
So there is no framework-availability reason to avoid it.

It is still the wrong tool here, for four reasons:

1. **It buys no additional guarantee at the point that matters.** `CreateProcess`
   returns its handles inside a blittable `PROCESS_INFORMATION` struct as raw
   `IntPtr` (`bin/cap.ps1:64-71, 80-84`). Any `SafeHandle` would have to be
   constructed *after* the P/Invoke returns — leaving exactly the same
   acquisition-to-ownership window a `try/finally` leaves. Closing that window
   properly means changing the `CreateProcess` marshalling signature and adding
   a constrained-execution region, which is a far larger and riskier change to
   the one primitive every tool in the package depends on.
2. **It adds a type per file.** A `SafeHandle` subclass is a new type; either it
   gets a per-file-unique name (11 more names to keep unique), or it is nested
   inside the launcher class (`CapLauncher.OwnedHandle` — collision-free,
   because the outer name already disambiguates), which is workable but costs
   ~15 lines per file for zero behavioral gain.
3. **It hides `CloseHandle`.** With `SafeHandle`, closing happens inside
   `ReleaseHandle()`. Part 2's harness instruments the `[DllImport]`
   declarations; keeping `CloseHandle` as a directly-called P/Invoke is what
   makes "closed exactly once" externally observable and testable.
4. **A shared helper type is impossible anyway.** Each `.ps1` runs its own
   `Add-Type` — sometimes in its own process, sometimes (bare-name resolution)
   in the caller's session. There is no place a shared type could be compiled
   once. Any shared `SafeHandle` wrapper would need a per-file-unique name too,
   i.e. it is not actually shared. This is why the recommended design introduces
   **no new type at all**.

**Recommended design:** one `try/finally` per `Run()`, ownership tracked in
plain `IntPtr` locals that stay `IntPtr.Zero` until the handle is really
acquired. No new types, no framework-specific APIs, no shared code between
files, per-file rollback stays possible.

Fallback, if the implementer hits an unexpected blocker: none needed — the
recommended design *is* the framework-independent fallback the review asked
for.

### 2.2 Template B — Job Object launchers (`cap`, `pint`, `capm`)

Shown with `cap`'s tool-specific content. **Preserve every existing comment
verbatim** (elided below as `// [unchanged: …]` only to keep this document
readable — in the real edit they stay exactly as they are today). Per-file
substitutions are in the table after the code.

```csharp
    public static int Run(int percent, string[] argv, string cmdExeCommandLine)
    {
        IntPtr hJob = CreateJobObject(IntPtr.Zero, null);
        if (hJob == IntPtr.Zero)
            throw new InvalidOperationException("CreateJobObject failed: " + Marshal.GetLastWin32Error());

        // Single owner for every handle this method acquires. The finally below closes
        // hThread/hProcess/hJob - in that order - on EVERY way out: normal return, any
        // of the InvalidOperationExceptions thrown here, and an unexpected managed
        // exception (allocation/marshalling failure) between acquisition and use.
        // hProcess/hThread stay IntPtr.Zero until CreateProcess has actually succeeded,
        // so each handle is closed exactly once and only if it was really acquired.
        IntPtr hProcess = IntPtr.Zero;
        IntPtr hThread = IntPtr.Zero;
        try
        {
            var cpuInfo = new JOBOBJECT_CPU_RATE_CONTROL_INFORMATION
            {
                ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
                CpuRate = (uint)(percent * 100)
            };
            int size = Marshal.SizeOf(cpuInfo);
            IntPtr ptr = Marshal.AllocHGlobal(size);
            bool ok;
            try
            {
                Marshal.StructureToPtr(cpuInfo, ptr, false);
                ok = SetInformationJobObject(hJob, JobObjectCpuRateControlInformation, ptr, (uint)size);
            }
            finally
            {
                Marshal.FreeHGlobal(ptr);
            }
            if (!ok)
                throw new InvalidOperationException("SetInformationJobObject failed: " + Marshal.GetLastWin32Error());

            var si = new STARTUPINFO();
            si.cb = Marshal.SizeOf(si);
            PROCESS_INFORMATION pi = new PROCESS_INFORMATION();

            // [unchanged: the "Try launching the target directly first" comment block -
            //  cap.ps1 has the long version, pint.ps1/capm.ps1 the short "See cap.ps1" one]
            bool isBatOrCmd = argv.Length > 0 && (
                argv[0].EndsWith(".bat", StringComparison.OrdinalIgnoreCase) ||
                argv[0].EndsWith(".cmd", StringComparison.OrdinalIgnoreCase));

            bool created = false;
            if (!isBatOrCmd)
            {
                var directCommandLine = new StringBuilder(BuildArgvCommandLine(argv));
                created = CreateProcess(null, directCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                    CREATE_SUSPENDED, IntPtr.Zero, null, ref si, out pi);
            }

            if (!created)
            {
                // [unchanged: the "Falling back to cmd.exe /c" comment block]
                foreach (var a in argv)
                {
                    if (a.IndexOf('%') >= 0)
                        throw new InvalidOperationException(
                            "Refusing to run: argument contains '%' and the target needs the cmd.exe " +
                            "fallback (not a directly-launchable .exe), where '%' can trigger unintended " +
                            "environment-variable expansion. See README's Argument handling section.");
                }

                string cmdExe = Environment.SystemDirectory + "\\cmd.exe";
                // [unchanged: the "/d: skip HKCU AutoRun …" comment block]
                var shellCommandLine = new StringBuilder("\"" + cmdExe + "\" /d /v:off /s /c \"" + cmdExeCommandLine + "\"");
                created = CreateProcess(null, shellCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                    CREATE_SUSPENDED, IntPtr.Zero, null, ref si, out pi);
                if (!created)
                    throw new InvalidOperationException("CreateProcess failed: " + Marshal.GetLastWin32Error());
            }

            // Ownership of the child's handles transfers here, once CreateProcess has
            // actually succeeded - from this point the finally below is what closes them.
            hProcess = pi.hProcess;
            hThread = pi.hThread;

            if (!AssignProcessToJobObject(hJob, hProcess))
            {
                // Can't guarantee the cap - kill instead of letting it run uncapped and orphaned.
                int err = Marshal.GetLastWin32Error();
                TerminateProcess(hProcess, 1);
                throw new InvalidOperationException("AssignProcessToJobObject failed: " + err);
            }

            if (ResumeThread(hThread) == 0xFFFFFFFF)
            {
                // Still suspended - an unbounded wait below would hang forever. Kill
                // it instead of leaving an orphaned, permanently-suspended process.
                int resumeErr = Marshal.GetLastWin32Error();
                TerminateProcess(hProcess, 1);
                throw new InvalidOperationException("ResumeThread failed: " + resumeErr);
            }

            if (WaitForSingleObject(hProcess, 0xFFFFFFFF) == 0xFFFFFFFF)
            {
                // The child's actual state is unknown here - don't just report
                // failure and potentially leave it running unmanaged in the
                // background. Best-effort kill before giving up.
                int waitErr = Marshal.GetLastWin32Error();
                TerminateProcess(hProcess, 1);
                throw new InvalidOperationException("WaitForSingleObject failed: " + waitErr);
            }

            uint exitCode;
            if (!GetExitCodeProcess(hProcess, out exitCode))
                throw new InvalidOperationException("GetExitCodeProcess failed: " + Marshal.GetLastWin32Error());

            return (int)exitCode;
        }
        finally
        {
            // Same order as the code this replaces: thread handle, process handle, job handle.
            if (hThread != IntPtr.Zero) CloseHandle(hThread);
            if (hProcess != IntPtr.Zero) CloseHandle(hProcess);
            if (hJob != IntPtr.Zero) CloseHandle(hJob);
        }
    }
```

Per-file substitutions for template B:

| | `cap.ps1` | `pint.ps1` | `capm.ps1` |
|---|---|---|---|
| signature | `Run(int percent, …)` | `Run(ulong affinityMask, …)` | `Run(ulong memoryLimitBytes, …)` |
| local + struct | `cpuInfo` / `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` | `limitInfo` / `JOBOBJECT_BASIC_LIMIT_INFORMATION` | `extInfo` / `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` |
| initializer | `ControlFlags = …ENABLE \| …HARD_CAP, CpuRate = (uint)(percent * 100)` | `LimitFlags = JOB_OBJECT_LIMIT_AFFINITY, Affinity = (UIntPtr)affinityMask` | `BasicLimitInformation = new …{ LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY }, JobMemoryLimit = (UIntPtr)memoryLimitBytes` |
| info class | `JobObjectCpuRateControlInformation` | `JobObjectBasicLimitInformation` | `JobObjectExtendedLimitInformation` |
| assign-failure comment noun | "the cap … uncapped" | "the pin … unpinned" | "the cap … uncapped" |
| extra comment above initializer | — | — | keep the `JOB_OBJECT_LIMIT_JOB_MEMORY` paragraph (`bin/capm.ps1:256-263`) |
| direct-launch comment | long version (`bin/cap.ps1:189-197`) | short version | short version |

### 2.3 Template A — non-Job launchers (8 files)

```csharp
    public static int Run(uint priorityClass, string[] argv, string cmdExeCommandLine)
    {
        var si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(si);
        PROCESS_INFORMATION pi = new PROCESS_INFORMATION();

        // [unchanged: the ".bat/.cmd targets skip the direct attempt" comment block]
        bool isBatOrCmd = argv.Length > 0 && (
            argv[0].EndsWith(".bat", StringComparison.OrdinalIgnoreCase) ||
            argv[0].EndsWith(".cmd", StringComparison.OrdinalIgnoreCase));

        bool created = false;
        if (!isBatOrCmd)
        {
            var directCommandLine = new StringBuilder(BuildArgvCommandLine(argv));
            created = CreateProcess(null, directCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                priorityClass, IntPtr.Zero, null, ref si, out pi);
        }

        if (!created)
        {
            // [unchanged: the "Falling back to cmd.exe /c" comment block]
            // No handle is held at this point, so this throw has nothing to clean up.
            foreach (var a in argv)
            {
                if (a.IndexOf('%') >= 0)
                    throw new InvalidOperationException(
                        "Refusing to run: argument contains '%' and the target needs the cmd.exe " +
                        "fallback (not a directly-launchable .exe), where '%' can trigger unintended " +
                        "environment-variable expansion. See README's Argument handling section.");
            }

            string cmdExe = Environment.SystemDirectory + "\\cmd.exe";
            // [unchanged: the "/d: skip HKCU AutoRun …" comment block]
            var shellCommandLine = new StringBuilder("\"" + cmdExe + "\" /d /v:off /s /c \"" + cmdExeCommandLine + "\"");
            created = CreateProcess(null, shellCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                priorityClass, IntPtr.Zero, null, ref si, out pi);
            if (!created)
                throw new InvalidOperationException("CreateProcess failed: " + Marshal.GetLastWin32Error());
        }

        // Ownership of the child's handles starts here - CreateProcess has succeeded, so
        // both are valid, and the finally below closes each of them exactly once on every
        // way out: normal return, a thrown InvalidOperationException, or an unexpected
        // managed exception.
        IntPtr hProcess = pi.hProcess;
        IntPtr hThread = pi.hThread;
        try
        {
            if (WaitForSingleObject(hProcess, 0xFFFFFFFF) == 0xFFFFFFFF)
            {
                // The child's actual state is unknown here - don't just report
                // failure and potentially leave it running unmanaged in the
                // background. Best-effort kill before giving up.
                int waitErr = Marshal.GetLastWin32Error();
                TerminateProcess(hProcess, 1);
                throw new InvalidOperationException("WaitForSingleObject failed: " + waitErr);
            }

            uint exitCode;
            if (!GetExitCodeProcess(hProcess, out exitCode))
                throw new InvalidOperationException("GetExitCodeProcess failed: " + Marshal.GetLastWin32Error());

            return (int)exitCode;
        }
        finally
        {
            // Same order as the code this replaces: thread handle, then process handle.
            CloseHandle(hThread);
            CloseHandle(hProcess);
        }
    }
```

No `IntPtr.Zero` guards here on purpose: the `try` begins *after* a
guaranteed-successful `CreateProcess`, so both handles are always valid.
(Template B needs the guards because `hJob` is acquired before the process
exists.) State this in the review note if asked — it is a deliberate asymmetry,
not an oversight.

Per-file substitutions for template A:

| File | `Run` signature | flags argument | notes |
|---|---|---|---|
| `idle.ps1` | `Run(uint priorityClass, …)` | `priorityClass` | reference copy |
| `belownormal.ps1` | same | `priorityClass` | identical to `idle` |
| `abovenormal.ps1` | same | `priorityClass` | identical to `idle` |
| `high.ps1` | same | `priorityClass` | identical to `idle` |
| `realtime.ps1` | same | `priorityClass` | identical to `idle` |
| `cy.ps1` | `Run(string[] argv, string cmdExeCommandLine)` | `0` | keep the extra `"A bare name like \"claude\"…"` comment lines |
| `cx.ps1` | same as `cy` | `0` | same, with `"codex"` |
| `admin.ps1` | same as `cy` | `0` | **keep** `/d /s /v:off` order and the two-line `StringBuilder` at `bin/admin.ps1:172-173`; **keep** the longer cmd.exe comment block; `admin.ps1` has no "See cap.ps1 for why .bat/.cmd" comment — do not add one |

### 2.4 What must NOT change

- **Exception message text.** All seven strings stay byte-identical:
  `"CreateJobObject failed: " + …`, `"SetInformationJobObject failed: " + …`,
  `"Refusing to run: argument contains '%' …"`, `"CreateProcess failed: " + …`,
  `"AssignProcessToJobObject failed: " + err`, `"ResumeThread failed: " + resumeErr`,
  `"WaitForSingleObject failed: " + waitErr`, `"GetExitCodeProcess failed: " + …`.
- **The captured Win32 error value.** Keep the explicit `int err/resumeErr/waitErr =
  Marshal.GetLastWin32Error();` locals in the three (shape B) / one (shape A)
  branches that call `TerminateProcess` before throwing — `TerminateProcess` is
  declared `SetLastError = true` (`bin/cap.ps1:104-105`) and would overwrite the
  value. Inlining `Marshal.GetLastWin32Error()` into the `GetExitCodeProcess`
  message is safe **only because** `CloseHandle` is declared *without*
  `SetLastError = true` (`bin/cap.ps1:107-108`) and no longer runs between the
  failure and the message. Verified: the prototype produced
  `GetExitCodeProcess failed: 6` and `WaitForSingleObject failed: 6` exactly.
- **Ordering.** suspend → assign → resume → wait, and `TerminateProcess`
  *before* the throw in the assign/resume/wait branches. Handle close order
  stays thread → process → job.
- **Exit codes.** `(int)exitCode` from `GetExitCodeProcess`; `exit 1` from the
  outer PowerShell `catch`.
- **Direct-launch-then-cmd.exe-fallback logic**, including the `.bat`/`.cmd`
  skip and the `/d /v:off /s` (or `/d /s /v:off` for `admin`) + outer-quote-wrap
  command line.
- **The `%` fail-closed check** — same position (only on the fallback path),
  same message, same "never launches the target" guarantee.
- **Class names.** `IdleLauncher`, …, `CapmLauncher`, `AdminLauncher` — unchanged.
  No new type is introduced anywhere, so the `Add-Type` collision regression
  (`test/win-nice.Tests.ps1:1347-1398`) cannot be re-opened.
- **Everything outside `Run()`**: structs, `[DllImport]` declarations, `const`s,
  `ArgvQuote`, `BuildArgvCommandLine`, `CapmLauncher.GetTotalPhysicalMemoryBytes`,
  `AdminLauncher.BuildArgvCommandLine` (public, called from PowerShell at
  `bin/admin.ps1:269`), and every line of PowerShell above/below the here-string.
- **CRLF** in the working tree for all 11 `.ps1` files.

The single **intentional** behavior delta is `bin/cap.ps1`'s `%`-fail-closed
branch, which starts closing `hJob` (see 1.3). It is invisible externally.
Mention it in the commit message.

### 2.5 Per-file checklist (11 files)

Apply template A or B; for each file confirm the four checkpoints.

| # | File | Template | Checkpoints |
|---|---|---|---|
| 1 | `bin/idle.ps1` | A | `CloseHandle(` count 7 → **3**; `TerminateProcess(` stays 2; comments intact; CRLF |
| 2 | `bin/belownormal.ps1` | A | same |
| 3 | `bin/abovenormal.ps1` | A | same |
| 4 | `bin/high.ps1` | A | same |
| 5 | `bin/realtime.ps1` | A | same |
| 6 | `bin/cy.ps1` | A | same, flags `0` |
| 7 | `bin/cx.ps1` | A | same, flags `0` |
| 8 | `bin/admin.ps1` | A | same, flags `0`, `/d /s /v:off` preserved |
| 9 | `bin/cap.ps1` | B | `CloseHandle(` count 18 → **4**; `TerminateProcess(` stays 4; CRLF |
| 10 | `bin/pint.ps1` | B | 19 → **4**; `TerminateProcess(` stays 4 |
| 11 | `bin/capm.ps1` | B | 19 → **4**; `TerminateProcess(` stays 4; `GetTotalPhysicalMemoryBytes` untouched |

Not in scope: `bin/uiup.ps1`, all `bin/*.bat`, all extensionless shims,
`install/*`, `README.md`, `CHANGELOG.md`, `skills/`.

### 2.6 How to verify Part 1

```
:: from the repo root
npm test
```
Expected: **47 passed, 0 failed, 7 skipped** (unchanged; `package.json:35`).

```
powershell -Command "Import-Module Pester -MaximumVersion 3.99; Invoke-Pester -Path test\win-nice.Tests.ps1"
```
Expected: **124 passed, 0 failed, 3 skipped** baseline (plus whatever Part 2
adds). The 3 skips are the already-elevated `admin` cases — expected, unrelated,
out of scope. This is the same invocation CI uses
(`.github/workflows/ci.yml:33-49`, which additionally sets
`$ErrorActionPreference = 'Continue'` — do that too if running the suite from a
step/shell where it defaults to `Stop`) and the same one documented at
`README.md:436-450` and `CONTRIBUTING.md:14`.

Structural checks:

```
powershell -NoProfile -Command "Get-ChildItem bin\*.ps1 | ForEach-Object { $e=$null; [System.Management.Automation.Language.Parser]::ParseFile($_.FullName,[ref]$null,[ref]$e) | Out-Null; if ($e.Count) { $_.Name + ': ' + $e.Count + ' parse errors' } }"
```
Expected: no output.

```
git ls-files --eol -- "bin/*.ps1" "bin/*.bat"
```
Expected: every row `w/crlf`. Any `w/lf` row is a regression — fix before
committing.

```
powershell -NoProfile -Command "Get-ChildItem bin\*.ps1 | ForEach-Object { $n=([regex]::Matches((Get-Content -Raw $_.FullName),'CloseHandle\(')).Count; '{0}: {1}' -f $_.Name,$n }"
```
Expected after Part 1: 3 for the eight shape-A files, 4 for `cap`/`pint`/`capm`,
0 for `uiup`.

Behavioral spot checks (each must print the exact code shown):

```
powershell -NoProfile -File bin\idle.ps1 cmd /c "exit 7"   & echo %ERRORLEVEL%   :: 7
powershell -NoProfile -File bin\cap.ps1 50 cmd /c "exit 7" & echo %ERRORLEVEL%   :: 7
powershell -NoProfile -File bin\pint.ps1 1 cmd /c "exit 7" & echo %ERRORLEVEL%   :: 7
powershell -NoProfile -File bin\capm.ps1 100m cmd /c "exit 7" & echo %ERRORLEVEL% :: 7
```

---

## 3. Part 2 — fault-injection tests for the native failure branches

### 3.1 Why the obvious approaches don't work

Researched concretely, not assumed:

- **Real faults from the CLI: impossible for 4 of the 6 calls.**
  `WaitForSingleObject`, `GetExitCodeProcess`, `ResumeThread` and
  `AssignProcessToJobObject` take only handles the launcher itself just created;
  nothing a caller passes on the command line reaches those arguments. There is
  no crafted handle value a *test* could smuggle in, because the test never gets
  to touch `pi`. (`AssignProcessToJobObject` does fail for real on pre-Windows-8
  when nesting jobs — irrelevant: Windows 8+ supports nested jobs, and
  `test/win-nice.Tests.ps1:889-1090` already relies on nesting working.)
- **`SetInformationJobObject`** fails for real on an out-of-range `CpuRate`
  (0 or >10000) — but `cap.ps1` validates `1..100` in PowerShell first
  (`bin/cap.ps1:12-15`), so the branch is unreachable without changing
  validation.
- **`CreateProcess` failure** is unreachable too: a missing target falls through
  to the cmd.exe fallback, and `Environment.SystemDirectory + "\\cmd.exe"` always
  exists. (The existing PATH-isolation test at `test/win-nice.Tests.ps1:1319-1345`
  accepts `CreateProcess failed` as one of three possible messages precisely
  because it cannot force it.)

So some form of injection is required.

### 3.2 Options considered

**Option A — delegate-typed static fields in the production classes.**
Each P/Invoke becomes `public static Func<…>` defaulting to the real import;
tests swap it. *Rejected.* It puts a publicly writable function pointer for
`WaitForSingleObject`/`TerminateProcess` into every shipped launcher. Bare-name
resolution runs these classes **in the caller's own PowerShell session**
(`test/win-nice.Tests.ps1:1349-1352`), so any script in that session could
neuter `admin`'s or `cap`'s fail-closed behavior at runtime. That is a real
weakening of a security-relevant guarantee, plus ~25 lines × 11 files of
production surface in a project whose stated property is minimalism
(`CONTRIBUTING.md:36-37`).

**Option B — environment-variable-gated fault code compiled into the class.**
*Rejected outright.* This is precisely the "toggle that could ship on" the
requirements forbid, and it makes an env var part of the security surface of
`admin`.

**Option C — separate harness that re-declares the same P/Invoke signatures
against crafted handles.** *Rejected.* It would test a hand-written copy of the
signatures, not the launcher's own control flow — it proves nothing about
`Run()`'s cleanup ordering, `TerminateProcess`-before-throw, or message text.
And the crafted-handle idea does not actually work for the interesting calls,
per 3.1.

**Option D — extract the C# into a shared `.cs` template compiled by both
production and tests** (the review's own first suggestion). *Rejected.* Each
`.ps1` must stay self-contained: `install/install.js` copies `bin/*.bat` and
`bin/*.ps1` (`test/install-uninstall.test.js:33`) and `package.json:24-32` ships
`bin` as-is. A shared `.cs` becomes a new installed artifact, a new manifest
entry, a new uninstall case, and a per-launch file read — and, because of the
class-name-uniqueness rule, it could not even define one shared class; it would
have to be a template with the class name substituted at load time. Large
change, no gain over Option E.

**Option E (recommended) — test-only source-transform fault harness.**
The test reads the embedded C# out of `bin/<tool>.ps1` via the PowerShell AST,
replaces specific `[DllImport]` *declarations* with instrumented managed stubs
that forward to the real entry point (`EntryPoint = "…"`) or force a failure on
demand, wraps the result in a unique `namespace`, `Add-Type`s it once, and calls
`Run()` directly.

Why this one:

- **Zero production change.** `bin/` is not touched by Part 2 at all. No toggle
  can ship on, because there is no toggle in shipped code. This is the decisive
  argument.
- It exercises the launcher's **own** control flow, comments and message
  strings — the code under test comes verbatim out of the shipped file.
- It reuses an established convention: `test/win-nice.Tests.ps1:1186-1188`
  already parses `admin.ps1` with `[…]::Parser]::ParseFile` and materializes a
  function out of it.
- Verified working: the prototype produced `WaitForSingleObject failed: 6`,
  `TerminateProcess` called once, 3 distinct handles closed, 0 close failures.

Honest weakness and its mitigation: the tests run a *transformed copy*, so a
transform whose anchor stops matching could silently test nothing. Mitigated by
(a) `Edit-SourceOnce` throwing when an anchor is missing **or non-unique**, and
(b) the source-shape guard test in 3.5 which asserts the invariant across all 11
files.

### 3.3 Harness code (goes in `test/win-nice.Tests.ps1`)

Placement: the **existing** suite file, near the bottom, before the final
cleanup at `test/win-nice.Tests.ps1:1413-1421`. A separate `.Tests.ps1` file
would force edits to `.github/workflows/ci.yml:48`, `README.md:436,447` and
`CONTRIBUTING.md:14`, which all hardcode `test\win-nice.Tests.ps1` — not worth
it.

```powershell
# ---------------------------------------------------------------------------
# Fault-injection harness. bin/ is NOT modified by any of this.
#
# ResumeThread / WaitForSingleObject / GetExitCodeProcess /
# AssignProcessToJobObject only ever fail on handles the launcher itself just
# created, so no command line can reach those branches - the integration tests
# above can only ever exercise the success paths. Instead of adding a toggle to
# production code, these tests take the SAME embedded C# out of the .ps1 (via
# the AST, like the admin routing tests above), swap individual [DllImport]
# declarations for instrumented managed stubs that forward to the real entry
# point (or force a documented failure on demand), compile that copy into its
# own namespace, and call Run() directly.
function Get-LauncherCSharp {
    param([Parameter(Mandatory = $true)][string]$Ps1Path)
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Ps1Path, [ref]$null, [ref]$null)
    $node = $ast.Find({
        param($a)
        $a -is [System.Management.Automation.Language.StringConstantExpressionAst] -and
        $a.StringConstantType -eq 'DoubleQuotedHereString' -and
        $a.Value -match 'public static class \w+Launcher'
    }, $true)
    if (-not $node) { throw "no embedded C# here-string found in $Ps1Path" }
    # LF-normalized so the anchors below don't have to care about CRLF.
    return ($node.Value -replace "`r`n", "`n")
}

# Anchored single-occurrence replacement. Throws when the anchor is missing OR
# ambiguous, so a launcher edit that moves it fails the test loudly instead of
# quietly producing an uninstrumented (always-green) probe.
function Edit-SourceOnce {
    param(
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][string]$Find,
        [Parameter(Mandatory = $true)][string]$Replace,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $needle = $Find -replace "`r`n", "`n"
    $i = $Text.IndexOf($needle, [StringComparison]::Ordinal)
    if ($i -lt 0) { throw "fault-probe anchor '$Label' not found - launcher source changed shape" }
    if ($Text.IndexOf($needle, $i + 1, [StringComparison]::Ordinal) -ge 0) { throw "fault-probe anchor '$Label' is not unique" }
    return $Text.Substring(0, $i) + ($Replace -replace "`r`n", "`n") + $Text.Substring($i + $needle.Length)
}

# Builds (once) an instrumented copy of a launcher's class and returns its Type.
# -JobObject additionally instruments the three Job-Object-only imports.
function New-LauncherFaultProbe {
    param(
        [Parameter(Mandatory = $true)][string]$Ps1Path,
        [Parameter(Mandatory = $true)][string]$Namespace,
        [Parameter(Mandatory = $true)][string]$ClassName,
        [switch]$JobObject
    )
    $already = [System.Management.Automation.PSTypeName]"$Namespace.$ClassName"
    if ($already.Type) { return $already.Type }

    $src = Get-LauncherCSharp -Ps1Path $Ps1Path

    # CloseHandle: record every close, forward to the real one, count failures.
    # A double close would make the second CloseHandle return false, so
    # "CloseHandleFailures -eq 0 and every logged handle distinct" IS the
    # closed-exactly-once oracle. All shared probe state lives here.
    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr hObject);
'@ @'
    [DllImport("kernel32.dll", EntryPoint = "CloseHandle")]
    static extern bool CloseHandleReal(IntPtr hObject);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern void SetLastError(uint dwErrCode);

    public static bool FailWait;
    public static bool FailResume;
    public static bool FailAssign;
    public static bool FailSetInfo;
    public static bool FailGetExitCode;
    public static int TerminateCalls;
    public static int CloseHandleFailures;
    public static int LastProcessId;
    public static System.Collections.Generic.List<IntPtr> ClosedHandles = new System.Collections.Generic.List<IntPtr>();

    public static void ResetProbe()
    {
        FailWait = false; FailResume = false; FailAssign = false;
        FailSetInfo = false; FailGetExitCode = false;
        TerminateCalls = 0; CloseHandleFailures = 0; LastProcessId = 0;
        ClosedHandles.Clear();
    }

    static bool CloseHandle(IntPtr hObject)
    {
        ClosedHandles.Add(hObject);
        bool ok = CloseHandleReal(hObject);
        if (!ok) CloseHandleFailures++;
        return ok;
    }
'@ 'CloseHandle'

    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "TerminateProcess")]
    static extern bool TerminateProcessReal(IntPtr hProcess, uint uExitCode);

    static bool TerminateProcess(IntPtr hProcess, uint uExitCode)
    {
        TerminateCalls++;
        return TerminateProcessReal(hProcess, uExitCode);
    }
'@ 'TerminateProcess'

    # ERROR_INVALID_HANDLE (6) via a real SetLastError P/Invoke, so
    # Marshal.GetLastWin32Error() returns a deterministic value - asserting on
    # it proves the launcher captures the error BEFORE calling TerminateProcess.
    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "WaitForSingleObject")]
    static extern uint WaitForSingleObjectReal(IntPtr hHandle, uint dwMilliseconds);

    static uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds)
    {
        if (FailWait) { SetLastError(6); return 0xFFFFFFFF; }
        return WaitForSingleObjectReal(hHandle, dwMilliseconds);
    }
'@ 'WaitForSingleObject'

    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "GetExitCodeProcess")]
    static extern bool GetExitCodeProcessReal(IntPtr hProcess, out uint lpExitCode);

    static bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode)
    {
        if (FailGetExitCode) { lpExitCode = 0; SetLastError(6); return false; }
        return GetExitCodeProcessReal(hProcess, out lpExitCode);
    }
'@ 'GetExitCodeProcess'

    # Records the child's PID so a test can assert the process is really gone
    # after an injected failure (the orphan-child concern from the release review).
    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcess(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode, EntryPoint = "CreateProcess")]
    static extern bool CreateProcessReal(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);

    static bool CreateProcess(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation)
    {
        bool ok = CreateProcessReal(lpApplicationName, lpCommandLine, lpProcessAttributes,
            lpThreadAttributes, bInheritHandles, dwCreationFlags, lpEnvironment,
            lpCurrentDirectory, ref lpStartupInfo, out lpProcessInformation);
        if (ok) LastProcessId = lpProcessInformation.dwProcessId;
        return ok;
    }
'@ 'CreateProcess'

    if ($JobObject) {
        $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint ResumeThread(IntPtr hThread);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "ResumeThread")]
    static extern uint ResumeThreadReal(IntPtr hThread);

    static uint ResumeThread(IntPtr hThread)
    {
        if (FailResume) { SetLastError(5); return 0xFFFFFFFF; }
        return ResumeThreadReal(hThread);
    }
'@ 'ResumeThread'

        $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "AssignProcessToJobObject")]
    static extern bool AssignProcessToJobObjectReal(IntPtr hJob, IntPtr hProcess);

    static bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess)
    {
        if (FailAssign) { SetLastError(5); return false; }
        return AssignProcessToJobObjectReal(hJob, hProcess);
    }
'@ 'AssignProcessToJobObject'

        $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "SetInformationJobObject")]
    static extern bool SetInformationJobObjectReal(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength);

    static bool SetInformationJobObject(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength)
    {
        if (FailSetInfo) { SetLastError(87); return false; }
        return SetInformationJobObjectReal(hJob, JobObjectInfoClass, lpJobObjectInfo, cbJobObjectInfoLength);
    }
'@ 'SetInformationJobObject'
    }

    # Wrapping in a namespace keeps the ORIGINAL class name (so the copy really is
    # the shipped code) while guaranteeing no Add-Type collision with a production
    # class - the "using" lines end up inside the namespace, which is legal C#.
    Add-Type -TypeDefinition ("namespace $Namespace`n{`n" + $src + "`n}`n") -Language CSharp
    return ([System.Management.Automation.PSTypeName]"$Namespace.$ClassName").Type
}

# One probe per template shape. The 8 non-Job launchers share a byte-identical
# Run() body (verified), and so do the 3 Job-Object ones, so two compiled probes
# cover every failure branch; the source-shape test below is what guarantees the
# other 9 files still match the template these two stand in for.
$script:probePriority = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'idle.ps1') `
    -Namespace 'WinNiceFaultProbePriority' -ClassName 'IdleLauncher'
$script:probeJob = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'cap.ps1') `
    -Namespace 'WinNiceFaultProbeJob' -ClassName 'CapLauncher' -JobObject

# Kills a probe child that survived a failed assertion (a successful test's
# injected TerminateProcess has already killed it).
function Remove-ProbeChild {
    param([int]$ProcessId)
    if ($ProcessId -gt 0) {
        $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if ($p) { Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue }
    }
}
```

### 3.4 Illustrative test cases

Three concrete `It` blocks, in this file's conventions (Pester 3, `Should Be`,
`New-TempScript`, `Join-Path $bin '…'`, no `BeforeAll`):

```powershell
Describe 'native failure branches (fault-injected copy of the embedded C#)' {
    It 'kills the child, reports the wait error, and closes every handle exactly once when WaitForSingleObject fails (Job Object template)' {
        $t = $script:probeJob
        $t::ResetProbe()
        $t::FailWait = $true
        $message = $null
        try {
            $t::Run(50, [string[]]@('ping', '-n', '30', '127.0.0.1'), 'ping -n 30 127.0.0.1') | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        # "failed: 6" (not 0) proves the Win32 error is captured BEFORE the
        # TerminateProcess call, which would otherwise overwrite it.
        $message | Should Be 'WaitForSingleObject failed: 6'
        $t::TerminateCalls | Should Be 1
        # hThread, hProcess, hJob - each closed once, each close succeeded
        # (a double close would return false and bump CloseHandleFailures).
        $t::ClosedHandles.Count | Should Be 3
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be 3
        $t::CloseHandleFailures | Should Be 0
        # Fail-closed: the wrapper reported failure, so the child must not still
        # be running in the background.
        Get-Process -Id $t::LastProcessId -ErrorAction SilentlyContinue | Should Be $null
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'kills the still-suspended child and reports the resume error when ResumeThread fails (Job Object template)' {
        $t = $script:probeJob
        $t::ResetProbe()
        $t::FailResume = $true
        $message = $null
        try {
            $t::Run(50, [string[]]@('ping', '-n', '30', '127.0.0.1'), 'ping -n 30 127.0.0.1') | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        # ERROR_ACCESS_DENIED (5) - deterministic, injected by the probe.
        $message | Should Be 'ResumeThread failed: 5'
        # Without this kill the child stays suspended forever: CREATE_SUSPENDED
        # was never undone and nobody else holds a handle to it.
        $t::TerminateCalls | Should Be 1
        $t::ClosedHandles.Count | Should Be 3
        $t::CloseHandleFailures | Should Be 0
        Get-Process -Id $t::LastProcessId -ErrorAction SilentlyContinue | Should Be $null
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'closes the job handle - and only it, exactly once - on the "%" fail-closed branch (Job Object template)' {
        $t = $script:probeJob
        $t::ResetProbe()
        $targetBat = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $targetBat -Value "@echo off`r`nexit /b 0`r`n"
        $message = $null
        try {
            $t::Run(50, [string[]]@($targetBat, '100%OFF'), 'x') | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        ($message -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
        # No process was ever created on this branch, so hJob is the only handle
        # in flight - and it must still be released.
        $t::ClosedHandles.Count | Should Be 1
        $t::CloseHandleFailures | Should Be 0
        $t::LastProcessId | Should Be 0
        Remove-Item $targetBat -ErrorAction SilentlyContinue
        $t::ResetProbe()
    }
}
```

Further cases the same harness enables (add them the same way; listed rather
than written out):

| Case | Probe | Flag | Expected message | `TerminateCalls` | `ClosedHandles.Count` |
|---|---|---|---|---|---|
| `AssignProcessToJobObject` fails | job | `FailAssign` | `AssignProcessToJobObject failed: 5` | 1 | 3 |
| `SetInformationJobObject` fails | job | `FailSetInfo` | `SetInformationJobObject failed: 87` | 0 | 1 (`hJob` only) |
| `GetExitCodeProcess` fails (job) | job | `FailGetExitCode` | `GetExitCodeProcess failed: 6` | 0 | 3 |
| `WaitForSingleObject` fails (priority) | priority | `FailWait` | `WaitForSingleObject failed: 6` | 1 | 2 |
| `GetExitCodeProcess` fails (priority) | priority | `FailGetExitCode` | `GetExitCodeProcess failed: 6` | 0 | 2 |
| success path still works | either | none | — (returns the child's code) | 0 | 3 / 2 |

For the priority probe, call `$script:probePriority::Run(64, [string[]]@('cmd','/c','exit 7'), 'cmd /c "exit 7"')`
(`64` = `IDLE_PRIORITY_CLASS`, `bin/idle.ps1:205`).

### 3.5 Source-shape guard (keeps the 2 probes honest for all 11 files)

The two probes stand in for 11 files. One extra test makes that substitution
safe, and simultaneously locks in Part 1's invariant:

```powershell
$launcherSourceFiles = @(
    @{ Name = 'idle';        Shape = 'Priority' }
    @{ Name = 'belownormal'; Shape = 'Priority' }
    @{ Name = 'abovenormal'; Shape = 'Priority' }
    @{ Name = 'high';        Shape = 'Priority' }
    @{ Name = 'realtime';    Shape = 'Priority' }
    @{ Name = 'cy';          Shape = 'Priority' }
    @{ Name = 'cx';          Shape = 'Priority' }
    @{ Name = 'admin';       Shape = 'Priority' }
    @{ Name = 'cap';         Shape = 'Job' }
    @{ Name = 'pint';        Shape = 'Job' }
    @{ Name = 'capm';        Shape = 'Job' }
)

Describe 'embedded launcher C# keeps the single-owner cleanup shape (<Name>)' {
    It 'closes each handle exactly once, only from the ownership finally (<Name>)' -TestCases $launcherSourceFiles {
        param($Name, $Shape)
        $src = Get-LauncherCSharp -Ps1Path (Join-Path $bin "$Name.ps1")
        # 1 [DllImport] declaration + one close per owned handle, all inside the
        # single finally. Any per-branch CloseHandle coming back bumps this count.
        $expected = if ($Shape -eq 'Job') { 4 } else { 3 }
        ([regex]::Matches($src, 'CloseHandle\(')).Count | Should Be $expected
        # The probe transform in this file anchors on these exact declarations.
        $src | Should Match ([regex]::Escape('static extern bool CloseHandle(IntPtr hObject);'))
        $src | Should Match ([regex]::Escape('static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);'))
        $src | Should Match ([regex]::Escape('static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);'))
    }
}
```

This test only becomes true after Part 1 — see the ordering in section 4.

### 3.6 Part 2 checklist

- [ ] `test/win-nice.Tests.ps1` only. Nothing under `bin/`, `install/`,
      `.github/`, `package.json`, `README.md`, `CONTRIBUTING.md` changes.
- [ ] Add `Get-LauncherCSharp`, `Edit-SourceOnce`, `New-LauncherFaultProbe`,
      `Remove-ProbeChild` and the two `$script:probe*` variables (3.3), placed
      after the `cy.ps1 / cx.ps1 PATH isolation` Describe
      (`test/win-nice.Tests.ps1:1319-1345`) and before the sequential-invocation
      Describe.
- [ ] Add the `native failure branches …` Describe (3.4) plus the remaining
      cases from the table.
- [ ] Add the source-shape guard Describe (3.5) — **with Part 1's commit**, not
      before (see 4.2).
- [ ] Pester 3 syntax only; no `BeforeAll`/`AfterAll`; every temp artifact goes
      through `New-TempFile`/`New-TempScript` so the run-scoped cleanup at
      `test/win-nice.Tests.ps1:1413-1421` catches leaks.
- [ ] `test/win-nice.Tests.ps1` is not covered by `.gitattributes` (`*.ps1`
      is — so it **is** `text eol=crlf`; keep CRLF here too).
- [ ] Every fault `It` ends by resetting the probe (static state is shared
      across `It` blocks in one Pester run).
- [ ] Never point a probe at `admin.ps1`'s elevation path — `New-LauncherFaultProbe`
      compiles `AdminLauncher` only if someone explicitly asks for it, and the
      recommended set is `idle` + `cap` only.

---

## 4. Execution order

### 4.1 Recommendation: Part 2 first, then Part 1. Sequential, not parallel.

Part 1's acceptance criterion is "100% of observable behavior preserved" across
11 files × up to 8 exit paths. Today **none** of those failure exit paths is
executed by any test — which is precisely why the refactor was deferred as too
risky. Part 2 is the instrument that makes Part 1 verifiable; landing it first
is the standard characterization-test-before-refactor order and turns Part 1
from "read the diff carefully" into "the suite proves it".

Part 2 is also risk-free to land on its own: it touches no shipped file, so it
cannot regress any user-visible behavior, and it is immediately useful even if
Part 1 is never done.

They are *file-disjoint* (`bin/**` vs `test/win-nice.Tests.ps1`), so two agents
**could** run in parallel — but should not, for one concrete reason: the
expectation values in Part 2's tests are exactly what Part 1 changes for one
branch (4.2). Running them concurrently means one agent writing assertions
against code the other is rewriting underneath. The saving is minutes; the cost
is a merge with a silently wrong oracle.

If parallelism is unavoidable, the only safe split is: agent A does Part 2
**excluding** the `%`-branch case and the source-shape guard; agent B does
Part 1; then a third pass adds those two.

### 4.2 The exact expectation delta Part 1 introduces

Only one, and it is invisible outside the process:

| Test | Before Part 1 | After Part 1 |
|---|---|---|
| `%` fail-closed, `cap` (3.4, third `It`) | `ClosedHandles.Count` = **0** (`bin/cap.ps1:217-224` never closes `hJob`) | **1** |
| same case for `pint`/`capm` | 1 (`bin/pint.ps1:216`, `bin/capm.ps1:320`) | 1 |
| source-shape guard (3.5) | fails (counts are 7/18/19) | passes (3/4) |

Concrete handling: land Part 2 with the `%` case asserting only
`$t::CloseHandleFailures | Should Be 0` and `$t::ClosedHandles.Count | Should BeLessThan 2`
(Pester 3 has `Should BeLessThan`), then in Part 1's commit tighten it to
`Should Be 1` and add the source-shape Describe. Note the tightening in the
commit message.

Everything else — messages, exit codes, `TerminateCalls`, ordering, handle
counts on all other branches — is identical before and after. This was verified
by running both templates against the real APIs before writing this plan.

### 4.3 Step-by-step

1. **Part 2, step 1.** Add the harness (3.3) + the fault Describe (3.4) with
   the `%` case in its loose form. Run both suites. Expect
   `124 + N passed, 0 failed, 3 skipped`.
2. **Part 2, step 2.** Commit. This commit alone is a shippable improvement.
3. **Part 1, step 1.** Apply template A to the 8 shape-A files, one file at a
   time, running the Pester suite after each (each file is independent — a
   mistake is isolated).
4. **Part 1, step 2.** Apply template B to `cap.ps1`, `pint.ps1`, `capm.ps1`.
5. **Part 1, step 3.** Tighten the `%` assertion to `Should Be 1`, add the
   source-shape guard Describe (3.5).
6. **Part 1, step 4.** Run the full verification list from 2.6 including the
   EOL and `CloseHandle(`-count checks. Commit.

---

## 5. Risk and rollback

### 5.1 Blast radius

`Run()` **is** the process-launching primitive for every tool in the package.
A defect here does not degrade one feature — it breaks `idle`, `belownormal`,
`abovenormal`, `high`, `realtime`, `cap`, `pint`, `capm`, `admin`, `cy` and `cx`
simultaneously. Realistic failure modes of a botched Part 1:

- **Handle closed too early** (e.g. the `finally` running before
  `GetExitCodeProcess`): every tool returns a wrong or zero exit code. Caught by
  the many `propagates the exit code` tests.
- **Handle not closed / closed twice**: a leak invisible in a short-lived
  process, or an `ERROR_INVALID_HANDLE` on the second close. Only Part 2's
  `CloseHandleFailures`/distinct-handle assertions catch this — another reason
  Part 2 goes first.
- **`TerminateProcess` accidentally dropped or moved after the throw**: the
  fail-closed guarantee silently regresses to "wrapper reports failure, child
  keeps running" — the exact P2 the review raised. Caught by
  `TerminateCalls | Should Be 1` and the `Get-Process -Id … | Should Be $null`
  assertions.
- **Win32 error captured after `TerminateProcess`**: messages degrade to
  `… failed: 0`. Caught by the `failed: 6` / `failed: 5` assertions.
- **`Add-Type` compile error** (a stray C# 6 construct): the tool dies at
  startup with a compiler error instead of running. Caught by the very first
  Pester test that touches the file.
- **CRLF lost**: `git diff` shows the whole file rewritten; `.gitattributes`
  would renormalize on commit, but the working tree diverges and review becomes
  impossible. Caught by `git ls-files --eol`.

Part 2 in isolation has essentially no blast radius — worst case a flaky or
always-green test.

### 5.2 Rollback

The design deliberately introduces **no shared file and no shared type**, so
rollback granularity is one file:

- Whole change: `git revert <commit>`.
- One tool misbehaving: restore just that file from the previous commit
  (`git show <prev>:bin/cap.ps1`) — the other 10 are unaffected because nothing
  is shared between them.
- Part 2 can be reverted independently of Part 1 and vice versa (disjoint
  files), with the single exception of the tightened `%` assertion in
  `test/win-nice.Tests.ps1`, which must be reverted together with `bin/cap.ps1`.

### 5.3 Minimal "done" bar

Neither part is done until all of the following pass on the final working tree:

```
npm test
```
→ **47 passed, 0 failed, 7 skipped**.

```
powershell -Command "$ErrorActionPreference='Continue'; Import-Module Pester -MaximumVersion 3.99; Invoke-Pester -Path test\win-nice.Tests.ps1"
```
→ **124 + N passed, 0 failed, 3 skipped**, where N is the number of tests added
by Part 2. The **3 skipped** are the already-elevated `admin` cases
(`test/win-nice.Tests.ps1:1104,1109,578`) — a pre-existing, unrelated coverage
gap; do not try to fix it here.

Plus, from 2.6: the `.ps1` parse check (0 errors), `git ls-files --eol` (all
`w/crlf` for `bin/*.ps1` and `bin/*.bat`), the `CloseHandle(` count check
(3/3/3/3/3/3/3/3/4/4/4), and the four exit-code spot checks.

A green run of only one of the two suites is **not** sufficient: `npm test`
never executes any launcher, and the Pester suite never executes the installer.
