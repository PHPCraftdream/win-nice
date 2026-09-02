# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
$Command = $args
# Deliberately no [Parameter()]/[CmdletBinding()] attributes: see capc.ps1 for why -
# it would expose PowerShell's common parameters and make them ambiguously
# prefix-match flags meant for the wrapped command.

if (-not $Command -or $Command.Count -eq 0) {
    Write-Error "usage: admin <command> [args...]"
    exit 1
}

# Fallback command line for the UAC (-Verb RunAs) branch, and for the inline branch
# when the target isn't a directly-launchable .exe (see AdminLauncher.Run below) -
# re-parsed by cmd.exe, so quoting must neutralize its operators (&|<>^) and not
# just whitespace - see capc.ps1 for the same logic and its documented "%" limitation.
$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '[\s"&|<>^]') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

# AdminLauncher (embedded C#): direct-CreateProcess-first, cmd.exe-fallback launcher,
# same strategy as capc.ps1's Capper - a direct .exe target never touches cmd.exe, so
# it isn't exposed to "%" expansion at all. Defined unconditionally (not only inside
# the already-elevated branch below) because the not-yet-elevated branch also calls
# AdminLauncher.BuildArgvCommandLine for its own direct (non-cmd.exe) -Verb RunAs launch.
$source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class AdminLauncher
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

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcess(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);

    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr hObject);

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

    // Public: reused from PowerShell by the not-yet-elevated branch to build the
    // -ArgumentList for a direct (non-cmd.exe) -Verb RunAs launch, so that path gets
    // the same CRT argv quoting as this file's own direct-CreateProcess path.
    public static string BuildArgvCommandLine(string[] argv)
    {
        var parts = new string[argv.Length];
        for (int i = 0; i < argv.Length; i++) parts[i] = ArgvQuote(argv[i]);
        return string.Join(" ", parts);
    }

    public static int Run(string[] argv, string cmdExeCommandLine)
    {
        var si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(si);
        PROCESS_INFORMATION pi = new PROCESS_INFORMATION();

        bool isBatOrCmd = argv.Length > 0 && (
            argv[0].EndsWith(".bat", StringComparison.OrdinalIgnoreCase) ||
            argv[0].EndsWith(".cmd", StringComparison.OrdinalIgnoreCase));

        bool created = false;
        if (!isBatOrCmd)
        {
            var directCommandLine = new StringBuilder(BuildArgvCommandLine(argv));
            created = CreateProcess(null, directCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                0, IntPtr.Zero, null, ref si, out pi);
        }

        if (!created)
        {
            // Falling back to cmd.exe /c: a literal "%" in any argument could now
            // trigger environment-variable expansion (cmd.exe pairs up "%" characters
            // across the whole command line, even across separate arguments) and
            // change what actually runs. Fail loudly here instead of silently risking
            // that - there's no reliable per-character escape for "%" at this level.
            // No handle is held at this point, so this throw has nothing to clean up.
            foreach (var a in argv)
            {
                if (a.IndexOf('%') >= 0)
                    throw new InvalidOperationException(
                        "Refusing to run: argument contains '%' and the target needs the cmd.exe " +
                        "fallback (not a directly-launchable .exe), where '%' can trigger unintended " +
                        "environment-variable expansion. See README's Argument handling section.");
            }

            // /d /s /v:off plus wrapping cmdExeCommandLine in one more outer quote pair:
            // cmd.exe's /C quote-stripping only cleanly strips the outer pair when it's
            // the sole/last quote pair on the line; with cmdExeCommandLine's own internal
            // quoted args present, cmd's "exactly two quotes" rule doesn't apply and it
            // falls back to stripping the first char and the LAST quote anywhere on the
            // line - which, without this extra wrap, is one of OUR internal quotes and
            // corrupts the parse (reopening "&" injection). The extra pair guarantees the
            // added closing quote is the true last character, so strip-first/strip-last
            // removes exactly our wrap and nothing else. /v:off pre-empts delayed-expansion
            // ("!VAR!") risk the same way the "%" check above pre-empts "%" expansion.
            string cmdExe = Environment.SystemDirectory + "\\cmd.exe";
            var shellCommandLine = new StringBuilder(
                "\"" + cmdExe + "\" /d /s /v:off /c \"" + cmdExeCommandLine + "\"");
            created = CreateProcess(null, shellCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                0, IntPtr.Zero, null, ref si, out pi);
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
                string message = "WaitForSingleObject failed: " + waitErr;
                // Report if the best-effort kill itself also failed.
                if (!TerminateProcess(hProcess, 1))
                    message += "; TerminateProcess also failed: " + Marshal.GetLastWin32Error();
                throw new InvalidOperationException(message);
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
}
"@
Add-Type -TypeDefinition $source -Language CSharp

if ($isAdmin) {
    # Already elevated - launch inline, sharing the current console.
    try {
        exit ([AdminLauncher]::Run([string[]]$Command, $commandLine))
    } catch {
        Write-Error $_.Exception.InnerException.Message
        exit 1
    }
}

# Not elevated - -Verb RunAs triggers the UAC consent prompt. ShellExecute-based, not
# CreateProcess, so this always opens its own console window (incompatible with
# -NoNewWindow). Same direct-launch-first, cmd.exe-fallback strategy as the
# already-elevated branch above (AdminLauncher.Run): a target that resolves to a real
# Application (.exe) launches directly via -FilePath - never touching cmd.exe and so
# never exposed to "%"/quote-stripping risk - while a .bat/.cmd target (no direct
# elevation-capable equivalent to CreateProcess's own .bat/.cmd auto-relaunch) and a
# target that resolves to no Application at all both go through the cmd.exe fallback
# below. The unresolvable case is the important one: cmd.exe BUILTINS (ver, dir,
# echo, set, start, ...) aren't files, so Start-Process -FilePath would die with
# "The system cannot find the file specified" before any UAC prompt - they must take
# the fallback like every other launcher's CreateProcess-failed branch does.
# Get-Command with -CommandType Application is the resolver: builtins and PowerShell
# aliases/functions (dir, echo, start) are invisible to it, which routes them to the
# fallback, while real executables resolve. Standalone function so the routing
# decision is testable without ever reaching -Verb RunAs (a real UAC prompt).
function Get-AdminLaunchRoute {
    param([Parameter(Mandatory = $true)][string]$Target)
    if ($Target -match '\.(bat|cmd)$') { return 'CmdFallback' }
    $resolved = Get-Command -Name ([System.Management.Automation.WildcardPattern]::Escape($Target)) -CommandType Application -ErrorAction SilentlyContinue
    if ($resolved -and $resolved.Path -and $resolved.Path -notmatch '\.(bat|cmd)$') { return 'Direct' }
    return 'CmdFallback'
}

$route = Get-AdminLaunchRoute -Target $Command[0]

if ($route -eq 'CmdFallback') {
    foreach ($a in $Command) {
        if ("$a".Contains('%')) {
            Write-Error "Refusing to run: argument contains '%', which cmd.exe could expand as an environment variable during elevation. See README's Argument handling section."
            exit 1
        }
    }
}

try {
    if ($route -eq 'CmdFallback') {
        # Same /d /s /v:off + outer-quote-wrap fix as AdminLauncher.Run's cmd.exe
        # fallback, and for the same reason: cmd.exe's /C quote-stripping.
        $p = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/d', '/s', '/v:off', '/c', ('"' + $commandLine + '"')) -Verb RunAs -Wait -PassThru
    } else {
        $startArgs = @{
            FilePath = $Command[0]
            Verb = 'RunAs'
            Wait = $true
            PassThru = $true
        }
        if ($Command.Count -gt 1) {
            $rest = [string[]]$Command[1..($Command.Count - 1)]
            $startArgs.ArgumentList = [AdminLauncher]::BuildArgvCommandLine($rest)
        }
        $p = Start-Process @startArgs
    }
} catch {
    Write-Error "Elevation was cancelled or failed: $($_.Exception.Message)"
    exit 1
}

exit $p.ExitCode
