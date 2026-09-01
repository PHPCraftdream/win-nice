# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
param(
    [string]$Percent,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Command
)

$percentValue = 0
if (-not [int]::TryParse($Percent, [ref]$percentValue) -or $percentValue -lt 1 -or $percentValue -gt 100) {
    Write-Error "usage: cap <percent 1-100> <command> [args...]"
    exit 1
}
if (-not $Command -or $Command.Count -eq 0) {
    Write-Error "usage: cap <percent> <command> [args...]"
    exit 1
}

$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '\s') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class Capper
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

    public static int Run(int percent, string commandLine)
    {
        IntPtr hJob = CreateJobObject(IntPtr.Zero, null);
        if (hJob == IntPtr.Zero)
            throw new InvalidOperationException("CreateJobObject failed: " + Marshal.GetLastWin32Error());

        var cpuInfo = new JOBOBJECT_CPU_RATE_CONTROL_INFORMATION
        {
            ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            CpuRate = (uint)(percent * 100)
        };
        int size = Marshal.SizeOf(cpuInfo);
        IntPtr ptr = Marshal.AllocHGlobal(size);
        Marshal.StructureToPtr(cpuInfo, ptr, false);
        bool ok = SetInformationJobObject(hJob, JobObjectCpuRateControlInformation, ptr, (uint)size);
        Marshal.FreeHGlobal(ptr);
        if (!ok)
        {
            CloseHandle(hJob);
            throw new InvalidOperationException("SetInformationJobObject failed: " + Marshal.GetLastWin32Error());
        }

        string cmdExe = Environment.SystemDirectory + "\\cmd.exe";
        var fullCommandLine = new StringBuilder("\"" + cmdExe + "\" /c " + commandLine);

        var si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(si);
        PROCESS_INFORMATION pi;

        bool created = CreateProcess(null, fullCommandLine, IntPtr.Zero, IntPtr.Zero, true,
            CREATE_SUSPENDED, IntPtr.Zero, null, ref si, out pi);
        if (!created)
        {
            CloseHandle(hJob);
            throw new InvalidOperationException("CreateProcess failed: " + Marshal.GetLastWin32Error());
        }

        if (!AssignProcessToJobObject(hJob, pi.hProcess))
        {
            // Can't guarantee the cap - kill instead of letting it run uncapped and orphaned.
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

$exitCode = [Capper]::Run($percentValue, $commandLine)
exit $exitCode
