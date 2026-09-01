# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
# Deliberately no param()/[CmdletBinding()]: a declared parameter name (even
# without a [Parameter()] attribute) can still be ambiguously prefix-matched by
# flags meant for the wrapped command (e.g. "-c" matching "-Count"). Reading
# everything from $args sidesteps PowerShell's parameter binder entirely.
$processorCount = [Environment]::ProcessorCount
$maxCount = [Math]::Min($processorCount, 63)
if ($args.Count -lt 2) {
    Write-Error "usage: pint <thread-count 1-$maxCount> <command> [args...]"
    exit 1
}
$countValue = 0
if (-not [int]::TryParse($args[0], [ref]$countValue) -or $countValue -lt 1 -or $countValue -gt $maxCount) {
    Write-Error "usage: pint <thread-count 1-$maxCount> <command> [args...]"
    exit 1
}
$Command = @($args[1..($args.Count - 1)])

# Fallback command line for when the target isn't a directly-launchable .exe (see
# Pinner.Run below) - re-parsed by cmd.exe (via "cmd.exe /c"), so quoting must
# neutralize its operators (&|<>^) and not just whitespace - see cap.ps1 for the
# same logic and its documented "%" limitation.
$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '[\s"&|<>^]') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class Pinner
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
    const int JobObjectBasicLimitInformation = 2;
    const uint JOB_OBJECT_LIMIT_AFFINITY = 0x00000010;

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

    public static int Run(ulong affinityMask, string[] argv, string cmdExeCommandLine)
    {
        IntPtr hJob = CreateJobObject(IntPtr.Zero, null);
        if (hJob == IntPtr.Zero)
            throw new InvalidOperationException("CreateJobObject failed: " + Marshal.GetLastWin32Error());

        var limitInfo = new JOBOBJECT_BASIC_LIMIT_INFORMATION
        {
            LimitFlags = JOB_OBJECT_LIMIT_AFFINITY,
            Affinity = (UIntPtr)affinityMask
        };
        int size = Marshal.SizeOf(limitInfo);
        IntPtr ptr = Marshal.AllocHGlobal(size);
        Marshal.StructureToPtr(limitInfo, ptr, false);
        bool ok = SetInformationJobObject(hJob, JobObjectBasicLimitInformation, ptr, (uint)size);
        Marshal.FreeHGlobal(ptr);
        if (!ok)
        {
            CloseHandle(hJob);
            throw new InvalidOperationException("SetInformationJobObject failed: " + Marshal.GetLastWin32Error());
        }

        var si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(si);
        PROCESS_INFORMATION pi = new PROCESS_INFORMATION();

        // See cap.ps1 for why .bat/.cmd targets skip the direct attempt entirely:
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
            string cmdExe = Environment.SystemDirectory + "\\cmd.exe";
            var shellCommandLine = new StringBuilder("\"" + cmdExe + "\" /c " + cmdExeCommandLine);
            created = CreateProcess(null, shellCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                CREATE_SUSPENDED, IntPtr.Zero, null, ref si, out pi);
            if (!created)
            {
                CloseHandle(hJob);
                throw new InvalidOperationException("CreateProcess failed: " + Marshal.GetLastWin32Error());
            }
        }

        if (!AssignProcessToJobObject(hJob, pi.hProcess))
        {
            // Can't guarantee the pin - kill instead of letting it run unpinned and orphaned.
            int err = Marshal.GetLastWin32Error();
            TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            CloseHandle(hJob);
            throw new InvalidOperationException("AssignProcessToJobObject failed: " + err);
        }

        ResumeThread(pi.hThread);
        WaitForSingleObject(pi.hProcess, 0xFFFFFFFF);

        uint exitCode;
        GetExitCodeProcess(pi.hProcess, out exitCode);

        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        CloseHandle(hJob);

        return (int)exitCode;
    }
}
"@

Add-Type -TypeDefinition $source -Language CSharp

# First $countValue logical processors, i.e. threads - not physical cores. See
# README. Bit-shift, not [Math]::Pow: doubles can't exactly represent 2^63.
$affinityMask = ([uint64]1 -shl $countValue) - [uint64]1
$exitCode = [Pinner]::Run($affinityMask, [string[]]$Command, $commandLine)
exit $exitCode
