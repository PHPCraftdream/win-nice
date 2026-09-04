# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
# Deliberately no param()/[CmdletBinding()]: a declared parameter name (even
# without a [Parameter()] attribute) can still be ambiguously prefix-matched by
# flags meant for the wrapped command (e.g. "-s" matching "-Size"). Reading
# everything from $args sidesteps PowerShell's parameter binder entirely.
$usage = "usage: capm <size> <command> [args...]  (size: plain integer 1-100 " +
    "= percent of total physical RAM, e.g. 50 - same convention as capc's " +
    "<percent 1-100>; number+m/M = MB, e.g. 512m; number+g/G = GB, e.g. 2g)"

if ($args.Count -lt 2) {
    Write-Error $usage
    exit 1
}

$sizeArg = $args[0]
# No "%" suffix on purpose, unlike capc/capt's own numeric-only args this one
# could otherwise carry a unit character - but capm is meant to be chainable
# with the other tools by bare name (e.g. "capc 50 capm 50 <command>"), and a
# "%" in an argument trips every tool's fail-closed check the moment a chain
# hop needs the cmd.exe fallback (which bare-name resolution always does,
# since none of these ship a .exe) - so "capc 50 capm 25% ..." used to fail
# while "capm 25% capc 50 ..." worked, an order-dependent foot-gun. A bare
# integer (capc's own convention) sidesteps that entirely.
if ($sizeArg -notmatch '^(?<num>\d+(\.\d+)?)(?<unit>[mMgG]?)$') {
    Write-Error $usage
    exit 1
}
# TryParse, not a raw [double] cast: an arbitrarily long digit string (the
# regex above has no length limit) overflows a plain [double] cast with a
# raw, unhandled PowerShell conversion error (path/line number and all) -
# TryParse fails cleanly instead, so every invalid <size> hits the same
# single usage message regardless of why it's invalid.
$sizeNum = 0.0
$numOk = [double]::TryParse($Matches['num'], [System.Globalization.NumberStyles]::Float,
    [System.Globalization.CultureInfo]::InvariantCulture, [ref]$sizeNum)
if (-not $numOk -or [double]::IsNaN($sizeNum) -or [double]::IsInfinity($sizeNum)) {
    Write-Error "capm: <size> is out of range. $usage"
    exit 1
}
$sizeUnit = $Matches['unit']
if ($sizeUnit -eq '') {
    # No suffix: percent of total RAM, matching capc's own <percent 1-100>
    # exactly - a plain integer only (TryParse rejects "50.5"), folded into
    # the internal "%" conversion path below ("%" is never a valid *input*
    # character here - see above - only an internal marker for that path).
    $percentValue = 0
    if (-not [int]::TryParse($sizeArg, [ref]$percentValue) -or $percentValue -lt 1 -or $percentValue -gt 100) {
        Write-Error $usage
        exit 1
    }
    $sizeUnit = '%'
    $sizeNum = $percentValue
} elseif ($sizeNum -le 0) {
    Write-Error $usage
    exit 1
}
$Command = @($args[1..($args.Count - 1)])

# Fallback command line for when the target isn't a directly-launchable .exe (see
# CapmLauncher.Run below) - re-parsed by cmd.exe (via "cmd.exe /c"), so quoting must
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

public static class CapmLauncher
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

    [StructLayout(LayoutKind.Sequential)]
    struct MEMORYSTATUSEX
    {
        public uint dwLength;
        public uint dwMemoryLoad;
        public ulong ullTotalPhys;
        public ulong ullAvailPhys;
        public ulong ullTotalPageFile;
        public ulong ullAvailPageFile;
        public ulong ullTotalVirtual;
        public ulong ullAvailVirtual;
        public ulong ullAvailExtendedVirtual;
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

    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr hObject);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX lpBuffer);

    const uint CREATE_SUSPENDED = 0x00000004;
    const int JobObjectExtendedLimitInformation = 9;
    const uint JOB_OBJECT_LIMIT_JOB_MEMORY = 0x00000200;
    const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;

    // Used by the PowerShell side to resolve a "<N>%" size argument into bytes
    // before Run() is ever called - the percentage is relative to total physical
    // RAM, not whatever's currently free, so the cap means the same thing
    // regardless of what else is running on the machine at invocation time.
    public static ulong GetTotalPhysicalMemoryBytes()
    {
        var status = new MEMORYSTATUSEX();
        status.dwLength = (uint)Marshal.SizeOf(typeof(MEMORYSTATUSEX));
        if (!GlobalMemoryStatusEx(ref status))
            throw new InvalidOperationException("GlobalMemoryStatusEx failed: " + Marshal.GetLastWin32Error());
        return status.ullTotalPhys;
    }

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

    public static int Run(ulong memoryLimitBytes, string[] argv, string cmdExeCommandLine)
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
            // JOB_OBJECT_LIMIT_JOB_MEMORY caps the whole job's aggregate committed memory,
            // not any single process - same "whole subtree, one ceiling" semantics as
            // capc's CPU% and capt's affinity, not a per-process limit. Exceeding it fails
            // the allocation call that would have breached it (VirtualAlloc-family APIs
            // return an error / .NET throws OutOfMemoryException) - Windows doesn't kill
            // the process outright, it just refuses to hand out more committed memory;
            // most programs don't handle that gracefully, so it usually looks like a
            // crash in practice, but it's the allocation failing, not an OS-issued kill.
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
                    // A backstop only, never the normal exit mechanism: the last
                    // handle closing terminates every process still assigned to
                    // the job FOR ANY reason, including this wrapper's own
                    // orderly close in the finally - and the wait below only
                    // waits on the directly wrapped root process, so a daemon it
                    // spawned and left running can still be in the job at that
                    // point, the root long gone. Killing a daemon on a SUCCESSFUL
                    // exit would break the documented daemon-survival contract
                    // (README: the memory ceiling sticks to any daemon the
                    // wrapped command leaves running, for that daemon's whole
                    // lifetime), so the success path below clears this flag
                    // first - see capc.ps1 for the full write-up.
                    LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                },
                JobMemoryLimit = (UIntPtr)memoryLimitBytes
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

            if (WaitForSingleObject(hProcess, 0xFFFFFFFF) == 0xFFFFFFFF)
            {
                // The child's actual state is unknown here - don't just report
                // failure and potentially leave it running unmanaged in the
                // background. Best-effort kill before giving up.
                int waitErr = Marshal.GetLastWin32Error();
                string message = "WaitForSingleObject failed: " + waitErr;
                // Report if the best-effort kill itself also failed.
                if (!TerminateProcess(hProcess, 1))
                    message += "; TerminateProcess also failed: " + Marshal.GetLastWin32Error();
                throw new InvalidOperationException(message);
            }

            uint exitCode;
            if (!GetExitCodeProcess(hProcess, out exitCode))
                throw new InvalidOperationException("GetExitCodeProcess failed: " + Marshal.GetLastWin32Error());

            // Normal success: the root process finished - release the
            // kill-on-close backstop so the finally below closes hJob WITHOUT
            // terminating anything still assigned to the job (the documented
            // daemon-survival contract: the memory ceiling keeps applying to
            // whatever the command left running, it just outlives this
            // wrapper's handle). Deliberately best-effort, not a throw: the
            // wrapped command succeeded, and a failed cleanup syscall must not
            // turn its real exit code below into a wrapper error - warn on
            // stderr instead. Same struct and field values as the original
            // set above, minus the kill-on-close bit, so the daemon keeps its
            // memory ceiling.
            var releaseInfo = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION
            {
                BasicLimitInformation = new JOBOBJECT_BASIC_LIMIT_INFORMATION
                {
                    LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY
                },
                JobMemoryLimit = (UIntPtr)memoryLimitBytes
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
            // Same order as the code this replaces: thread handle, process handle, job handle.
            if (hThread != IntPtr.Zero) CloseHandle(hThread);
            if (hProcess != IntPtr.Zero) CloseHandle(hProcess);
            if (hJob != IntPtr.Zero) CloseHandle(hJob);
        }
    }
}
"@

Add-Type -TypeDefinition $source -Language CSharp

# $sizeUnit is always 'g'/'G'/'m'/'M'/'%' by this point - the validation above
# either matched an m/M/g/G suffix or resolved a bare integer to '%'.
try {
    switch ($sizeUnit) {
        { $_ -in 'g', 'G' } { $memoryLimitBytes = [uint64]($sizeNum * 1GB) }
        { $_ -in 'm', 'M' } { $memoryLimitBytes = [uint64]($sizeNum * 1MB) }
        '%' {
            $totalPhysBytes = [CapmLauncher]::GetTotalPhysicalMemoryBytes()
            $memoryLimitBytes = [uint64]([double]$totalPhysBytes * ($sizeNum / 100.0))
        }
    }
} catch {
    # An absurdly large <size> can overflow the double->uint64 cast above
    # (e.g. a huge decimal string) - report it as a usage error, not a crash.
    Write-Error "capm: <size> is out of range. $usage"
    exit 1
}
if ($memoryLimitBytes -eq 0) {
    Write-Error "capm: <size> is too small - rounds to 0 bytes. $usage"
    exit 1
}
# SIZE_T/UIntPtr is process-width: 32-bit PowerShell can't address a limit
# above 4GiB regardless of the machine's actual RAM or architecture. Computed
# from [UIntPtr]::Size rather than [UIntPtr]::MaxValue - the latter doesn't
# exist on .NET Framework (Windows PowerShell 5.1) and silently reads as $null
# there, which would make this check compare against 0 and always trip.
$sizeTMaxBytes = if ([UIntPtr]::Size -eq 4) { [uint64][uint32]::MaxValue } else { [uint64]::MaxValue }
if ($memoryLimitBytes -gt $sizeTMaxBytes) {
    Write-Error ("capm: <size> ($memoryLimitBytes bytes) exceeds the addressable limit " +
        "for this PowerShell process ($sizeTMaxBytes bytes, $([UIntPtr]::Size * 8)-bit) - " +
        "use 64-bit PowerShell for larger caps, or lower <size>.")
    exit 1
}

try {
    exit ([CapmLauncher]::Run($memoryLimitBytes, [string[]]$Command, $commandLine))
} catch {
    Write-Error $_.Exception.InnerException.Message
    exit 1
}
