# Developer sandbox for anything that spawns many processes (cargo test, node --test,
# Pester). Usage: guarded.ps1 <maxProcesses> <timeoutSeconds> <command> [args...]
#  - the command runs on a PRIVATE HIDDEN DESKTOP: no window it (or its children) opens,
#    console or GUI, can ever appear on the user's screen;
#  - one Job Object: at most <maxProcesses> live processes (a fork bomb is refused, not
#    obeyed), Idle priority for the whole tree, everything killed on timeout (exit 124)
#    and when the command exits (no leaked probes/daemons);
#  - stdin is NUL, stdout+stderr are captured and printed after the command ends.
# Exit code: the command's (124 on timeout, 125 on a sandbox error).
$ErrorActionPreference = 'Stop'
if ($args.Count -lt 3 -or $args[0] -notmatch '^\d+$' -or $args[1] -notmatch '^\d+$') {
    [Console]::Error.WriteLine('usage: guarded.ps1 <maxProcesses> <timeoutSeconds> <command> [args...]')
    exit 125
}
$maxProcs = [uint32]$args[0]
$seconds = [uint32]$args[1]
$rest = @($args[2..($args.Count - 1)])

function ArgvQuote([string]$a) {
    if ($a -ne '' -and $a -notmatch '[\s"]') { return $a }
    $s = [regex]::Replace($a, '(\\*)"', '$1$1\"')
    $s = [regex]::Replace($s, '(\\+)$', '$1$1')
    return '"' + $s + '"'
}
$cmdline = ($rest | ForEach-Object { ArgvQuote ([string]$_) }) -join ' '

Add-Type -TypeDefinition @"
using System;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;

public static class Guard
{
    [StructLayout(LayoutKind.Sequential)] struct SA { public int n; public IntPtr sd; public int inherit; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct SI { public int cb; public string reserved, desktop, title; public int x, y, xs, ys, xc, yc, fill, flags;
                public short show, cb2; public IntPtr res2, hin, hout, herr; }
    [StructLayout(LayoutKind.Sequential)] struct PI { public IntPtr hp, ht; public int pid, tid; }
    [StructLayout(LayoutKind.Sequential)] struct BASIC { public long perProc, perJob; public uint flags; public UIntPtr minWs, maxWs;
                public uint active; public UIntPtr affinity; public uint prio, sched; }
    [StructLayout(LayoutKind.Sequential)] struct IOC { public ulong a, b, c, d, e, f; }
    [StructLayout(LayoutKind.Sequential)] struct EXT { public BASIC b; public IOC io; public UIntPtr pm, jm, ppm, pjm; }

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateDesktopW(string name, string dev, IntPtr mode, uint flags, uint access, IntPtr sa);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr d);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string f, uint access, uint share, ref SA sa, uint disp, uint attrs, IntPtr t);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateJobObjectW(IntPtr sa, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr j, int cls, ref EXT i, int len);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AssignProcessToJobObject(IntPtr j, IntPtr p);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit, uint flags,
        IntPtr env, string cwd, ref SI si, out PI pi);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint ResumeThread(IntPtr t);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateJobObject(IntPtr j, uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);

    const uint GENERIC_ALL = 0x10000000, GENERIC_READ = 0x80000000, GENERIC_WRITE = 0x40000000;
    const uint CREATE_SUSPENDED = 4, CREATE_NO_WINDOW = 0x08000000;
    const uint LIM_ACTIVE = 8, LIM_PRIO = 0x20, LIM_KILL = 0x2000, IDLE = 0x40;
    const int STARTF_USESTDHANDLES = 0x100;

    static void Fail(string what)
    {
        throw new InvalidOperationException(what + " failed: " + Marshal.GetLastWin32Error());
    }

    public static int Run(string cmdline, uint maxProcs, uint seconds, string logPath)
    {
        string deskName = "wn-guard-" + Guid.NewGuid().ToString("N");
        IntPtr desk = CreateDesktopW(deskName, null, IntPtr.Zero, 0, GENERIC_ALL, IntPtr.Zero);
        if (desk == IntPtr.Zero) Fail("CreateDesktop");
        IntPtr job = IntPtr.Zero, hin = IntPtr.Zero, hlog = IntPtr.Zero, hp = IntPtr.Zero, ht = IntPtr.Zero;
        try
        {
            return Inner(cmdline, maxProcs, seconds, logPath, deskName, ref job, ref hin, ref hlog, ref hp, ref ht);
        }
        finally
        {
            // Closing the job kills everything still running (KILL_ON_JOB_CLOSE).
            if (job != IntPtr.Zero) CloseHandle(job);
            if (hp != IntPtr.Zero) CloseHandle(hp);
            if (ht != IntPtr.Zero) CloseHandle(ht);
            if (hin != IntPtr.Zero) CloseHandle(hin);
            if (hlog != IntPtr.Zero) CloseHandle(hlog);
            CloseDesktop(desk);
        }
    }

    static int Inner(string cmdline, uint maxProcs, uint seconds, string logPath, string deskName,
        ref IntPtr job, ref IntPtr hin, ref IntPtr hlog, ref IntPtr hp, ref IntPtr ht)
    {
        var sa = new SA { n = Marshal.SizeOf(typeof(SA)), inherit = 1 };
        hin = CreateFileW("NUL", GENERIC_READ, 3, ref sa, 3, 0, IntPtr.Zero);
        hlog = CreateFileW(logPath, GENERIC_WRITE, 3, ref sa, 2, 0, IntPtr.Zero);
        if (hin == new IntPtr(-1) || hlog == new IntPtr(-1)) Fail("CreateFile");

        job = CreateJobObjectW(IntPtr.Zero, null);
        if (job == IntPtr.Zero) Fail("CreateJobObject");
        var ext = new EXT();
        ext.b.flags = LIM_ACTIVE | LIM_PRIO | LIM_KILL;
        ext.b.active = maxProcs;
        ext.b.prio = IDLE;
        if (!SetInformationJobObject(job, 9, ref ext, Marshal.SizeOf(typeof(EXT)))) Fail("SetInformationJobObject");

        var si = new SI();
        si.cb = Marshal.SizeOf(typeof(SI));
        si.desktop = "WinSta0\\" + deskName;
        si.flags = STARTF_USESTDHANDLES;
        si.hin = hin; si.hout = hlog; si.herr = hlog;
        PI pi;
        if (!CreateProcessW(null, new StringBuilder(cmdline), IntPtr.Zero, IntPtr.Zero, true,
                CREATE_SUSPENDED | CREATE_NO_WINDOW, IntPtr.Zero, null, ref si, out pi))
            Fail("CreateProcess");
        hp = pi.hp; ht = pi.ht;
        if (!AssignProcessToJobObject(job, hp)) { TerminateJobObject(job, 125); Fail("AssignProcessToJobObject"); }
        if (ResumeThread(ht) == 0xFFFFFFFF) { TerminateJobObject(job, 125); Fail("ResumeThread"); }

        uint w = WaitForSingleObject(hp, seconds * 1000);
        if (w == 0x102) { TerminateJobObject(job, 124); return 124; }
        uint code;
        if (!GetExitCodeProcess(hp, out code)) Fail("GetExitCodeProcess");
        return unchecked((int)code);
    }
}
"@

$log = [IO.Path]::GetTempFileName()
try {
    $code = [Guard]::Run($cmdline, $maxProcs, $seconds, $log)
} catch {
    [Console]::Error.WriteLine("guarded.ps1: $($_.Exception.InnerException.Message)$($_.Exception.Message)")
    $code = 125
}
$out = [Console]::OpenStandardOutput()
# Killed children may still hold the inherited log handle for a moment: share it.
$fsLog = New-Object IO.FileStream($log, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]'ReadWrite, Delete')
try { $fsLog.CopyTo($out); $out.Flush() } finally { $fsLog.Dispose() }
for ($i = 0; $i -lt 20 -and (Test-Path $log); $i++) { Remove-Item $log -ErrorAction SilentlyContinue; if (Test-Path $log) { Start-Sleep -Milliseconds 100 } }
if ($code -eq 124) { [Console]::Error.WriteLine("guarded.ps1: timed out after $seconds s - whole tree killed") }
exit $code
