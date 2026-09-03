# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
# Deliberately no param()/[CmdletBinding()]: a declared parameter name (even
# without a [Parameter()] attribute) can still be ambiguously prefix-matched by
# flags meant for the wrapped command (e.g. "-s" matching "-Size"). Reading
# everything from $args sidesteps PowerShell's parameter binder entirely.
#
# Why caps exists: capc/capt/capm deliberately impose no timeout - a quota tool
# shouldn't unilaterally decide a legitimate long build is stuck (same precedent
# as nice/cpulimit, which bound resource use but never wall-clock time). caps is
# the complement: the caller has already decided "kill this after N seconds no
# matter what", and the Job Object (not process walking) is what makes "and
# nothing survives it" true for the whole spawned tree.
$usage = "usage: caps <seconds> <command> [args...]  (seconds: positive whole or " +
    "decimal number, e.g. 2 or 2.5 - converted to whole milliseconds; minimum " +
    "1 ms, maximum 4294967294 ms (~49.7 days), because 0xFFFFFFFF is the " +
    "wait-forever sentinel, not a deadline)"

if ($args.Count -lt 2) {
    Write-Error $usage
    exit 1
}

$secondsArg = $args[0]
if ($secondsArg -notmatch '^(?<num>\d+(\.\d+)?)$') {
    Write-Error $usage
    exit 1
}
# TryParse, not a raw [double] cast: an arbitrarily long digit string (the
# regex above has no length limit) overflows a plain [double] cast with a
# raw, unhandled PowerShell conversion error (path/line number and all) -
# TryParse fails cleanly instead, so every invalid <seconds> hits the same
# single usage message regardless of why it's invalid. (Same reason capm.ps1
# documents for its own <size>.)
$secondsNum = 0.0
$numOk = [double]::TryParse($Matches['num'], [System.Globalization.NumberStyles]::Float,
    [System.Globalization.CultureInfo]::InvariantCulture, [ref]$secondsNum)
if (-not $numOk -or [double]::IsNaN($secondsNum) -or [double]::IsInfinity($secondsNum) -or $secondsNum -le 0) {
    Write-Error "caps: <seconds> is out of range. $usage"
    exit 1
}
# WaitForSingleObject's dwMilliseconds is a uint32 whose 0xFFFFFFFF value is
# reserved as INFINITE - so this tool's deadline cap is 0xFFFFFFFE ms (~49.7
# days). Anything larger would silently wrap/truncate into a different
# deadline or collide with the wait-forever sentinel; reject it as the usage
# error it is instead. Floor to whole milliseconds so a sub-millisecond value
# can neither round up nor truncate to a meaningless 0 unnoticed.
$timeoutMsDouble = $secondsNum * 1000.0
if ($timeoutMsDouble -lt 1 -or $timeoutMsDouble -gt 4294967294) {
    Write-Error "caps: <seconds> is out of range. $usage"
    exit 1
}
$timeoutMs = [uint32][math]::Floor($timeoutMsDouble)
$Command = @($args[1..($args.Count - 1)])

# Fallback command line for when the target isn't a directly-launchable .exe (see
# CapsLauncher.Run below) - re-parsed by cmd.exe (via "cmd.exe /c"), so quoting must
# neutralize its operators (&|<>^) and not just whitespace - see capc.ps1 for the
# same logic and its documented "%" limitation.
$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '[\s"&|<>^]') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class CapsLauncher
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO
    {
        public int cb;
        public string lpReserved;
        public string lpDesktop;
        public string lpTitle;
        public int dwX;
        public int dwY;
        public int dwXSize;
        public int dwYSize;
        public int dwXCountChars;
        public int dwYCountChars;
        public int dwFillAttribute;
        public int dwFlags;
        public short wShowWindow;
        public short cbReserved2;
        public IntPtr lpReserved2;
        public IntPtr hStdInput;
        public IntPtr hStdOutput;
        public IntPtr hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION
    {
        public IntPtr hProcess;
        public IntPtr hThread;
        public int dwProcessId;
        public int dwThreadId;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_BASIC_LIMIT_INFORMATION
    {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct IO_COUNTERS
    {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong OtherOperationCount;
        public ulong ReadTransferCount;
        public ulong WriteTransferCount;
        public ulong OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcess(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr CreateJobObject(IntPtr lpJobAttributes, string lpName);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint ResumeThread(IntPtr hThread);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);

    // The timeout path's whole-tree kill: TerminateJobObject terminates every
    // process still assigned to the job in one atomic kernel call - the direct
    // child and everything it spawned, no process-tree walking, no window
    // where a descendant outlives the child. (TerminateProcess above stays for
    // the failure paths, where the best-effort kill can only ever target the
    // one process handle in hand.)
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateJobObject(IntPtr hJob, uint uExitCode);

    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr hObject);

    const uint CREATE_SUSPENDED = 0x00000004;
    const int JobObjectExtendedLimitInformation = 9;
    const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;

    // Poll-slice length for the absolute-deadline wait loop in Run(): long
    // enough that a bounded run costs only one kernel call per second, short
    // enough that timeout detection after an unexpected wake is prompt.
    const uint DeadlinePollSliceMs = 1000;

    // Standard MSVCRT/CommandLineToArgvW quoting: safe for a directly-launched .exe's
    // own argv parsing. No cmd.exe involved on this path, so none of its operator or
    // "%" expansion semantics apply - this is the safe path, used whenever possible.
    static string ArgvQuote(string arg)
    {
        if (arg.Length > 0 && arg.IndexOfAny(new char[] { ' ', '\t', '\n', '\v', '"' }) < 0)
            return arg;

        var result = new StringBuilder();
        result.Append('"');
        int backslashes = 0;
        foreach (char c in arg)
        {
            if (c == '\\')
            {
                backslashes++;
            }
            else if (c == '"')
            {
                result.Append('\\', backslashes * 2 + 1);
                result.Append('"');
                backslashes = 0;
            }
            else
            {
                if (backslashes > 0) { result.Append('\\', backslashes); backslashes = 0; }
                result.Append(c);
            }
        }
        if (backslashes > 0) result.Append('\\', backslashes * 2);
        result.Append('"');
        return result.ToString();
    }

    static string BuildArgvCommandLine(string[] argv)
    {
        var parts = new string[argv.Length];
        for (int i = 0; i < argv.Length; i++) parts[i] = ArgvQuote(argv[i]);
        return string.Join(" ", parts);
    }

    // Pure deadline arithmetic for the wait loop in Run(), public and with both
    // timestamps injectable so tests can pin the sleep/suspend semantics
    // deterministically (a CI runner can't be genuinely suspended on demand):
    // returns the milliseconds to pass to the next WaitForSingleObject slice -
    // min(maxSliceMs, time left until deadlineUtc as of nowUtc) - or 0 when the
    // deadline has passed, which the caller must treat as its timeout path
    // without waiting again. The partial final slice is ceilinged so the last
    // wait lands on the deadline instead of a fraction short of it.
    public static uint RemainingWaitMs(DateTime deadlineUtc, DateTime nowUtc, uint maxSliceMs)
    {
        double remainingMs = (deadlineUtc - nowUtc).TotalMilliseconds;
        if (remainingMs <= 0.0)
            return 0;
        if (remainingMs >= maxSliceMs)
            return maxSliceMs;
        return (uint)Math.Ceiling(remainingMs);
    }

    public static int Run(uint timeoutMs, string[] argv, string cmdExeCommandLine)
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
            var extInfo = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION
            {
                BasicLimitInformation = new JOBOBJECT_BASIC_LIMIT_INFORMATION
                {
                    // KILL_ON_JOB_CLOSE: the cleanup in the finally below only runs
                    // if this launcher process survives to execute it. Killed from
                    // outside (taskkill without /T, a crash), nothing in-process
                    // ever runs - without this flag the last job handle dying with
                    // the process would leave every process still assigned to the
                    // job running on, untracked and unmanaged. With it, Windows
                    // itself terminates the whole job at that moment.
                    //
                    // A backstop only, never a normal exit mechanism: the last
                    // handle closing terminates every process still assigned to
                    // the job FOR ANY reason, including this wrapper's own orderly
                    // close in the finally - and the bounded wait below only waits
                    // on the directly wrapped root process, so a daemon it spawned
                    // and left running can still be in the job then. Killing a
                    // daemon on a SUCCESSFUL exit would break the documented
                    // daemon-survival contract (README: a limit sticks to any
                    // daemon the wrapped command leaves running, for that daemon's
                    // whole lifetime), so the success path clears this flag before
                    // returning; the timeout path keeps it (it kills the job
                    // itself). See capc.ps1 for the full write-up.
                    // caps sets no other limit flag - the job exists purely so the
                    // timeout kill (and that close-of-business kill) covers the
                    // whole process tree, not to impose any resource ceiling.
                    LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                }
            };
            int size = Marshal.SizeOf(extInfo);
            IntPtr ptr = Marshal.AllocHGlobal(size);
            bool ok;
            try
            {
                Marshal.StructureToPtr(extInfo, ptr, false);
                ok = SetInformationJobObject(hJob, JobObjectExtendedLimitInformation, ptr, (uint)size);
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

            // See capc.ps1 for why .bat/.cmd targets skip the direct attempt entirely:
            // CreateProcess silently re-invokes them through cmd.exe on its own, using
            // unescaped text, instead of failing the way a genuinely missing exe would.
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
                // Falling back to cmd.exe /c: a literal "%" in any argument could now
                // trigger environment-variable expansion (cmd.exe pairs up "%" characters
                // across the whole command line, even across separate arguments) and
                // change what actually runs. Fail loudly here instead of silently risking
                // that - there's no reliable per-character escape for "%" at this level.
                foreach (var a in argv)
                {
                    if (a.IndexOf('%') >= 0)
                        throw new InvalidOperationException(
                            "Refusing to run: argument contains '%' and the target needs the cmd.exe " +
                            "fallback (not a directly-launchable .exe), where '%' can trigger unintended " +
                            "environment-variable expansion. See README's Argument handling section.");
                }

                string cmdExe = Environment.SystemDirectory + "\\cmd.exe";
                // /d: skip HKCU AutoRun (user-writable registry key). /v:off: disable delayed
                // expansion so "!var!" in an argument can't be expanded. /s plus the extra outer
                // quote pair: cmd's /S rule strips exactly that outer pair and leaves the rest of
                // the string untouched - without /S, cmd strips the first and last quote of the
                // whole line instead, which breaks quoting whenever the target path itself needs
                // quotes AND another argument is also quoted.
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
                string message = "AssignProcessToJobObject failed: " + err;
                // Report if the best-effort kill itself also failed.
                if (!TerminateProcess(hProcess, 1))
                    message += "; TerminateProcess also failed: " + Marshal.GetLastWin32Error();
                throw new InvalidOperationException(message);
            }

            if (ResumeThread(hThread) == 0xFFFFFFFF)
            {
                // Still suspended - an unbounded wait below would hang forever. Kill
                // it instead of leaving an orphaned, permanently-suspended process.
                int resumeErr = Marshal.GetLastWin32Error();
                string message = "ResumeThread failed: " + resumeErr;
                // Report if the best-effort kill itself also failed.
                if (!TerminateProcess(hProcess, 1))
                    message += "; TerminateProcess also failed: " + Marshal.GetLastWin32Error();
                throw new InvalidOperationException(message);
            }

            // caps' one behavioral difference from every other launcher in this
            // repo: the wait is bounded. The PowerShell side validated the
            // deadline into [1, 0xFFFFFFFE] ms before the cast, so the value can
            // never collide with the 0xFFFFFFFF INFINITE sentinel.
            //
            // The deadline is computed once as an ABSOLUTE UTC timestamp and
            // polled in short slices - NOT passed as one long relative wait.
            // WaitForSingleObject's relative dwMilliseconds does not count time
            // spent in low-power sleep/suspend on Windows 8+ (see its Microsoft
            // docs page), so a laptop suspended mid-wait would otherwise resume
            // with most of its original countdown still ahead of it instead of
            // noticing the wall-clock deadline passed while it slept.
            // DateTime.UtcNow keeps advancing across a suspend (the RTC keeps
            // running), so re-deriving the remaining time from it before every
            // slice makes the deadline genuinely absolute: right after waking
            // from an overslept suspend, the very next iteration sees no time
            // left and takes the timeout path immediately.
            // https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject
            DateTime deadlineUtc = DateTime.UtcNow.AddMilliseconds(timeoutMs);
            uint waitResult;
            while (true)
            {
                // Capped by the real remaining time (see RemainingWaitMs). A
                // fast-exiting child is still detected instantly via
                // WAIT_OBJECT_0 - the polling never delays a normal run.
                uint sliceMs = RemainingWaitMs(deadlineUtc, DateTime.UtcNow, DeadlinePollSliceMs);
                if (sliceMs == 0)
                {
                    // No time left before the deadline - indistinguishable from
                    // the single-wait WAIT_TIMEOUT result handled below.
                    waitResult = 0x00000102; // WAIT_TIMEOUT
                    break;
                }
                waitResult = WaitForSingleObject(hProcess, sliceMs);
                if (waitResult != 0x00000102)
                    break; // WAIT_OBJECT_0 (finished) or WAIT_FAILED (handled below)
            }
            if (waitResult == 0xFFFFFFFF)
            {
                // WAIT_FAILED - same handling as every other launcher: the
                // child's actual state is unknown here - don't just report
                // failure and potentially leave it running unmanaged in the
                // background. Best-effort kill before giving up.
                int waitErr = Marshal.GetLastWin32Error();
                string message = "WaitForSingleObject failed: " + waitErr;
                // Report if the best-effort kill itself also failed.
                if (!TerminateProcess(hProcess, 1))
                    message += "; TerminateProcess also failed: " + Marshal.GetLastWin32Error();
                throw new InvalidOperationException(message);
            }
            if (waitResult == 0x00000102) // WAIT_TIMEOUT
            {
                // The deadline passed - the whole point of this tool. Kill the
                // entire job now (see TerminateJobObject above for why one
                // kernel call is the right primitive). KILL_ON_JOB_CLOSE is
                // only the backstop for THIS wrapper dying non-cooperatively;
                // on this path the wrapper is alive and kills the job itself.
                //
                // 124: the unix timeout(1) convention, reported by the
                // PowerShell handler below. Deliberately NOT
                // GetExitCodeProcess here - the reason for exiting is already
                // known, and the process may still be mid-death when asked.
                if (!TerminateJobObject(hJob, 124))
                    throw new InvalidOperationException("TerminateJobObject failed: " + Marshal.GetLastWin32Error());
                throw new TimeoutException("caps: timed out after " + timeoutMs + " ms");
            }
            // WAIT_OBJECT_0: the child finished inside the deadline - fall
            // through to the exact same GetExitCodeProcess/propagate path as
            // every other launcher.

            uint exitCode;
            if (!GetExitCodeProcess(hProcess, out exitCode))
                throw new InvalidOperationException("GetExitCodeProcess failed: " + Marshal.GetLastWin32Error());

            // WAIT_OBJECT_0 path only: the root process finished inside the
            // deadline - release the kill-on-close backstop so the finally
            // below closes hJob WITHOUT terminating anything still assigned
            // to the job (the documented daemon-survival contract: whatever
            // the command left running detached outlives this wrapper's
            // handle, uncapped by design - caps imposes no resource limit).
            // NOT reached by the timeout path above: that one kills the job
            // itself via TerminateJobObject and keeps the flag as the
            // non-cooperative-death backstop. Deliberately best-effort, not a
            // throw: the wrapped command succeeded, and a failed cleanup
            // syscall must not turn its real exit code below into a wrapper
            // error - warn on stderr instead.
            var releaseInfo = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION
            {
                BasicLimitInformation = new JOBOBJECT_BASIC_LIMIT_INFORMATION
                {
                    LimitFlags = 0
                }
            };
            int releaseSize = Marshal.SizeOf(releaseInfo);
            IntPtr releasePtr = Marshal.AllocHGlobal(releaseSize);
            bool releaseOk;
            try
            {
                Marshal.StructureToPtr(releaseInfo, releasePtr, false);
                releaseOk = SetInformationJobObject(hJob, JobObjectExtendedLimitInformation, releasePtr, (uint)releaseSize);
            }
            finally
            {
                Marshal.FreeHGlobal(releasePtr);
            }
            if (!releaseOk)
                Console.Error.WriteLine("warning: could not release the job's kill-on-close guard - a still-running background process left by the wrapped command may be terminated when this wrapper exits");

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
}
"@

Add-Type -TypeDefinition $source -Language CSharp

try {
    exit ([CapsLauncher]::Run($timeoutMs, [string[]]$Command, $commandLine))
} catch [System.TimeoutException] {
    # $secondsArg is the user's own spelling of the deadline ("2", "2.5") -
    # echo that, not a re-derived number, and exit with timeout(1)'s 124.
    Write-Error "caps: timed out after ${secondsArg}s - job and every process in it were force-killed"
    exit 124
} catch {
    Write-Error $_.Exception.InnerException.Message
    exit 1
}
