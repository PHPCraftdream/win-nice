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
    "1 ms, maximum 4294967294 ms (~49.7 days) - a deliberate ceiling, " +
    "not an API limit)"

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
# The upper bound is a deliberate usage ceiling, not an API limit: the deadline
# is an absolute 64-bit FILETIME armed via SetWaitableTimer, and the wait below
# passes dwMilliseconds = INFINITE unconditionally, so no uint32 boundary
# applies to it. ~49.7 days comfortably covers any real use; anything larger is
# rejected as the usage error it is instead, never silently truncated. Floor to
# whole milliseconds so a sub-millisecond value can neither round up nor
# truncate to a meaningless 0 unnoticed.
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
    static extern IntPtr CreateWaitableTimer(IntPtr lpTimerAttributes, bool bManualReset, string lpTimerName);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetWaitableTimer(IntPtr hTimer, ref long pDueTime, int lPeriod,
        IntPtr pfnCompletionRoutine, IntPtr lpArgToCompletionRoutine, bool fResume);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForMultipleObjects(uint nCount, IntPtr[] lpHandles, bool bWaitAll, uint dwMilliseconds);

    // Authoritative exit-timestamp source for the tie-break in Run(): when
    // both wait handles are already signaled, only the kernel's own record of
    // WHEN the process exited can say whether that happened before the
    // deadline. FILETIME fields combine into one 64-bit UTC FILETIME value
    // (high dword first), the same representation and epoch as the timer's
    // absolute due time.
    [StructLayout(LayoutKind.Sequential)]
    struct FILETIME
    {
        public uint dwLowDateTime;
        public uint dwHighDateTime;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetProcessTimes(IntPtr hProcess, out FILETIME lpCreationTime,
        out FILETIME lpExitTime, out FILETIME lpKernelTime, out FILETIME lpUserTime);

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

    public static int Run(uint timeoutMs, string[] argv, string cmdExeCommandLine)
    {
        IntPtr hJob = CreateJobObject(IntPtr.Zero, null);
        if (hJob == IntPtr.Zero)
            throw new InvalidOperationException("CreateJobObject failed: " + Marshal.GetLastWin32Error());

        // Single owner for every handle this method acquires. The finally below closes
        // hThread/hProcess/hJob/hTimer - in that order - on EVERY way out: normal
        // return, any of the InvalidOperationExceptions thrown here, and an
        // unexpected managed exception (allocation/marshalling failure) between
        // acquisition and use. hProcess/hThread stay IntPtr.Zero until CreateProcess
        // has actually succeeded, hTimer until the deadline timer is created just
        // before the wait, so each handle is closed exactly once and only if it
        // was really acquired.
        IntPtr hProcess = IntPtr.Zero;
        IntPtr hThread = IntPtr.Zero;
        IntPtr hTimer = IntPtr.Zero;
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
            // deadline into [1, 0xFFFFFFFE] ms - a deliberate usage ceiling -
            // before the cast; the INFINITE below is passed unconditionally
            // (the timer's absolute due time is what actually bounds the wait).
            //
            // The deadline is computed once as an ABSOLUTE UTC timestamp, armed
            // into a one-shot waitable timer, and waited on TOGETHER with the
            // process handle - it is never any kind of relative wait, not even a
            // sliced one. WaitForSingleObject's relative dwMilliseconds does not
            // count time spent in low-power sleep/suspend on Windows 8+, and the
            // problem is not just ONE long wait: a relative wait ALREADY IN
            // PROGRESS when the machine suspends keeps running down its
            // pre-sleep remainder after the wake, however far past the deadline
            // the clock now is - so the previous poll-slice design could still
            // overshoot the deadline by up to a slice and even accept a
            // post-deadline exit as on-time success. SetWaitableTimer's
            // ABSOLUTE due time has the opposite, documented property: a timer
            // whose due time has already passed comes up already-signaled,
            // whenever the system next looks at it - signaled-ness is a property
            // of the absolute clock, not of an in-progress wait. If the machine
            // sleeps past the deadline, the timer is therefore signaled the
            // moment anything waits on it after the wake: the deadline cannot be
            // postponed by a leftover wait remainder, and a child exiting after
            // the deadline can never be mistaken for an on-time success.
            // https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setwaitabletimer
            DateTime deadlineUtc = DateTime.UtcNow.AddMilliseconds(timeoutMs);

            // Manual reset, not auto-reset: once due the timer STAYS signaled,
            // so "deadline passed" is a stable state rather than a consumable
            // event - the process-finished vs deadline-hit outcome can't be
            // lost to a race against signal consumption. Anonymous (null name),
            // exactly like the job object above. On failure here the child is
            // already running; the throw falls through to the finally, whose
            // hJob close still carries KILL_ON_JOB_CLOSE (the release below
            // only happens on the success path), so the whole tree is
            // terminated by the same backstop as a non-cooperative wrapper
            // death - nothing is left running unmanaged.
            hTimer = CreateWaitableTimer(IntPtr.Zero, true, null);
            if (hTimer == IntPtr.Zero)
                throw new InvalidOperationException("CreateWaitableTimer failed: " + Marshal.GetLastWin32Error());

            // Positive due time = ABSOLUTE FILETIME (100ns units since
            // 1601-01-01 UTC), which ToFileTimeUtc() produces directly from the
            // deadline above. lPeriod 0 = one-shot. No completion routine: APC
            // delivery would require an alertable wait, which this never is.
            // fResume = false, deliberately: caps must never wake a sleeping
            // machine just to enforce a timeout - that would be a surprising
            // and hostile side effect for a tool whose whole point is to
            // coexist politely with the user's machine. If the machine is
            // asleep at the due time, the timer simply comes up
            // already-signaled on wake (the whole point of the absolute due
            // time) and the kill happens then.
            long timerDueTime = deadlineUtc.ToFileTimeUtc();
            if (!SetWaitableTimer(hTimer, ref timerDueTime, 0, IntPtr.Zero, IntPtr.Zero, false))
                throw new InvalidOperationException("SetWaitableTimer failed: " + Marshal.GetLastWin32Error());

            // Wait for EITHER the process to finish (index 0 - success path
            // below) or the deadline to pass (index 1 - timeout path below).
            // INFINITE is safe here: the timer itself is what bounds this wait,
            // and its due time is already validated into [1, 0xFFFFFFFE] ms.
            // If both handles are ALREADY signaled when the wait is serviced
            // (a child exit racing the deadline - or a deadline that passed
            // during sleep, after which the timer sits signaled while a woken
            // child races through its last instructions),
            // WaitForMultipleObjects reports the LOWEST signaled index: the
            // process. That alone proves nothing about which signal came
            // first, so the tie is resolved below against the timer's
            // absolute due time, not by array order.
            uint waitResult = WaitForMultipleObjects(2, new IntPtr[] { hProcess, hTimer }, false, 0xFFFFFFFF);
            if (waitResult == 0xFFFFFFFF)
            {
                // WAIT_FAILED - same handling as every other launcher: the
                // child's actual state is unknown here - don't just report
                // failure and potentially leave it running unmanaged in the
                // background. Best-effort kill before giving up.
                int waitErr = Marshal.GetLastWin32Error();
                string message = "WaitForMultipleObjects failed: " + waitErr;
                // Report if the best-effort kill itself also failed.
                if (!TerminateProcess(hProcess, 1))
                    message += "; TerminateProcess also failed: " + Marshal.GetLastWin32Error();
                throw new InvalidOperationException(message);
            }
            if (waitResult == 0x00000000) // WAIT_OBJECT_0: the process handle signaled
            {
                // The lowest-index rule makes "process reported first"
                // compatible with "exited AFTER the deadline": both objects
                // signaled, process listed first. The kernel's own record of
                // when the process actually exited is the authoritative
                // arbiter - GetProcessTimes returns a real exit time for a
                // terminated process, in the same absolute FILETIME clock and
                // epoch the timer's due time was computed in. At or before
                // the due time: genuinely finished in time, normal success
                // path below. Strictly after: the process only won the
                // array-order tie - treat it exactly like the timer signaling
                // (timeout path below), never as an on-time success.
                FILETIME creationTime, exitTime, kernelTime, userTime;
                if (!GetProcessTimes(hProcess, out creationTime, out exitTime, out kernelTime, out userTime))
                    throw new InvalidOperationException("GetProcessTimes failed: " + Marshal.GetLastWin32Error());
                long exitFileTime = ((long)exitTime.dwHighDateTime << 32) | (long)exitTime.dwLowDateTime;
                if (exitFileTime > timerDueTime)
                    waitResult = 0x00000001;
            }
            if (waitResult == 0x00000001) // deadline: timer signaled, or the tie-break demoted a post-deadline exit
            {
                // The deadline passed (timer signaled, or the process's real
                // exit time landed after the due time) - the whole point of
                // this tool. Kill the
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
            int releaseErr = 0;
            try
            {
                Marshal.StructureToPtr(releaseInfo, releasePtr, false);
                releaseOk = SetInformationJobObject(hJob, JobObjectExtendedLimitInformation, releasePtr, (uint)releaseSize);
                // Capture the Win32 error immediately, before any other call can
                // overwrite it - every other native failure branch in this file
                // reports the code for the same diagnosability reason.
                if (!releaseOk)
                    releaseErr = Marshal.GetLastWin32Error();
            }
            finally
            {
                Marshal.FreeHGlobal(releasePtr);
            }
            if (!releaseOk)
                Console.Error.WriteLine("warning: could not release the job's kill-on-close guard (SetInformationJobObject failed with Win32 error " + releaseErr + ") - a still-running background process left by the wrapped command may be terminated when this wrapper exits");

            return (int)exitCode;
        }
        finally
        {
            // Thread handle, process handle, job handle, then the timer. The
            // timer's position is functionally irrelevant - an anonymous kernel
            // object with no cascade semantics, unlike hJob whose close IS the
            // kill-on-close trigger - so it is appended last to leave the
            // long-established sequence untouched.
            if (hThread != IntPtr.Zero) CloseHandle(hThread);
            if (hProcess != IntPtr.Zero) CloseHandle(hProcess);
            if (hJob != IntPtr.Zero) CloseHandle(hJob);
            if (hTimer != IntPtr.Zero) CloseHandle(hTimer);
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
