# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
# Deliberately no param()/[CmdletBinding()]: a declared parameter name (even
# without a [Parameter()] attribute) can still be ambiguously prefix-matched by
# flags meant for the wrapped command (e.g. "-p" matching "-Percent"). Reading
# everything from $args sidesteps PowerShell's parameter binder entirely.
if ($args.Count -lt 2) {
    Write-Error "usage: capc <percent 1-100> <command> [args...]"
    exit 1
}
$percentValue = 0
if (-not [int]::TryParse($args[0], [ref]$percentValue) -or $percentValue -lt 1 -or $percentValue -gt 100) {
    Write-Error "usage: capc <percent 1-100> <command> [args...]"
    exit 1
}
$Command = @($args[1..($args.Count - 1)])

# Fallback command line for when the target isn't a directly-launchable .exe (see
# CapcLauncher.Run below) - re-parsed by cmd.exe (via "cmd.exe /c"), so quoting must
# neutralize its operators (&|<>^) and not just whitespace, or e.g. "A&B" gets split
# into two commands. NOTE: a literal "%" in an argument can still trigger cmd.exe
# environment-variable expansion (e.g. "%PATH%") even when quoted, and cmd.exe pairs
# up "%" characters across argument/quote boundaries - two unrelated arguments that
# each contain one "%" can corrupt each other. There is no reliable per-character
# escape for this at the cmd.exe /c level; it's a known, inherent limitation shared
# by anything that shells out through cmd.exe (Node's own child_process included).
# This fallback path only runs for .bat/.cmd/builtin targets - a direct .exe target
# never goes through cmd.exe at all, so it isn't exposed to this limitation.
$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '[\s"&|<>^]') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class CapcLauncher
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
    struct JOBOBJECT_CPU_RATE_CONTROL_INFORMATION
    {
        public uint ControlFlags;
        public uint CpuRate;
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

    const uint CREATE_SUSPENDED = 0x00000004;
    const int JobObjectCpuRateControlInformation = 15;
    const uint JOB_OBJECT_CPU_RATE_CONTROL_ENABLE = 0x1;
    const uint JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP = 0x4;
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

            // KILL_ON_JOB_CLOSE: the cleanup in the finally below only runs if this
            // launcher process survives to execute it. Killed from outside (taskkill
            // without /T, a crash), nothing in-process ever runs - without this flag
            // the last job handle dying with the process would leave every process
            // still assigned to the job running on, untracked and unmanaged. With
            // it, Windows itself terminates the whole job at that moment.
            //
            // A backstop only, never the normal exit mechanism: the last handle
            // closing terminates every process still assigned to the job FOR ANY
            // reason, including this wrapper's own orderly close in the finally -
            // and the wait below only waits on the directly wrapped root process,
            // so a daemon it spawned and left running (build server, watcher) can
            // still be in the job at that point, the root long gone. Killing a
            // daemon on a SUCCESSFUL exit would break the documented
            // daemon-survival contract (README: the limit sticks to any daemon
            // the wrapped command leaves running, for that daemon's whole
            // lifetime; see also
            // https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
            // so the success path below clears this flag before returning. Every
            // other field stays zero - Windows only reads a struct field when its
            // own LimitFlags bit is set.
            // Set via the EXTENDED info class: JobObjectBasicLimitInformation
            // rejects this flag with ERROR_INVALID_PARAMETER.
            var extInfo = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION
            {
                BasicLimitInformation = new JOBOBJECT_BASIC_LIMIT_INFORMATION
                {
                    LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                }
            };
            int extSize = Marshal.SizeOf(extInfo);
            IntPtr extPtr = Marshal.AllocHGlobal(extSize);
            bool extOk;
            try
            {
                Marshal.StructureToPtr(extInfo, extPtr, false);
                extOk = SetInformationJobObject(hJob, JobObjectExtendedLimitInformation, extPtr, (uint)extSize);
            }
            finally
            {
                Marshal.FreeHGlobal(extPtr);
            }
            if (!extOk)
                throw new InvalidOperationException("SetInformationJobObject failed: " + Marshal.GetLastWin32Error());

            var si = new STARTUPINFO();
            si.cb = Marshal.SizeOf(si);
            PROCESS_INFORMATION pi = new PROCESS_INFORMATION();

            // Try launching the target directly first (no shell at all) - unless it's a
            // .bat/.cmd file. CreateProcess has an undocumented-but-real fallback of its
            // own for those: instead of failing, it silently re-invokes them through
            // cmd.exe using OUR unescaped argv text (ArgvQuote only protects CRT argv
            // parsing, not cmd.exe's operators), reopening the exact "A&B" splits this
            // whole file exists to prevent. A bare name with no extension is safe either
            // way: CreateProcess only ever auto-appends ".exe" to it, never ".bat/.cmd",
            // so it fails cleanly (ERROR_FILE_NOT_FOUND) when only a same-named .bat/.cmd
            // exists, and falls through to the escaped path below.
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

            // Normal success: the root process finished and reported its exit
            // code - release the kill-on-close backstop so the finally below
            // closes hJob WITHOUT terminating anything still assigned to the
            // job. This is the documented daemon-survival contract, not a leak:
            // whatever the command left running detached stays in the job and
            // stays CPU-capped, it just outlives this wrapper's handle.
            // Deliberately best-effort, not a throw: the wrapped command
            // succeeded, and a failed cleanup syscall must not turn its real
            // exit code below into a wrapper error - warn on stderr instead.
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
    exit ([CapcLauncher]::Run($percentValue, [string[]]$Command, $commandLine))
} catch {
    Write-Error $_.Exception.InnerException.Message
    exit 1
}
