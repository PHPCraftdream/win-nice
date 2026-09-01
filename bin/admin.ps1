# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
$Command = $args
# Deliberately no [Parameter()]/[CmdletBinding()] attributes: see cap.ps1 for why -
# it would expose PowerShell's common parameters and make them ambiguously
# prefix-match flags meant for the wrapped command.

if (-not $Command -or $Command.Count -eq 0) {
    Write-Error "usage: admin <command> [args...]"
    exit 1
}

# Fallback command line for the UAC (-Verb RunAs) branch, and for the inline branch
# when the target isn't a directly-launchable .exe (see Runner.Run below) - re-parsed
# by cmd.exe, so quoting must neutralize its operators (&|<>^) and not just
# whitespace - see cap.ps1 for the same logic and its documented "%" limitation.
$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '[\s"&|<>^]') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if ($isAdmin) {
    # Already elevated - launch inline, sharing the current console. Same
    # direct-CreateProcess-first, cmd.exe-fallback strategy as cap.ps1: a direct .exe
    # target never touches cmd.exe, so it isn't exposed to "%" expansion at all.
    $source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class Runner
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

    static string BuildArgvCommandLine(string[] argv)
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
            foreach (var a in argv)
            {
                if (a.IndexOf('%') >= 0)
                    throw new InvalidOperationException(
                        "Refusing to run: argument contains '%' and the target needs the cmd.exe " +
                        "fallback (not a directly-launchable .exe), where '%' can trigger unintended " +
                        "environment-variable expansion. See README's Argument handling section.");
            }

            string cmdExe = Environment.SystemDirectory + "\\cmd.exe";
            var shellCommandLine = new StringBuilder("\"" + cmdExe + "\" /c " + cmdExeCommandLine);
            created = CreateProcess(null, shellCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                0, IntPtr.Zero, null, ref si, out pi);
            if (!created)
                throw new InvalidOperationException("CreateProcess failed: " + Marshal.GetLastWin32Error());
        }

        WaitForSingleObject(pi.hProcess, 0xFFFFFFFF);

        uint exitCode;
        GetExitCodeProcess(pi.hProcess, out exitCode);

        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);

        return (int)exitCode;
    }
}
"@
    Add-Type -TypeDefinition $source -Language CSharp
    try {
        exit ([Runner]::Run([string[]]$Command, $commandLine))
    } catch {
        Write-Error $_.Exception.InnerException.Message
        exit 1
    }
}

# Not elevated - -Verb RunAs triggers the UAC consent prompt. ShellExecute-based,
# not CreateProcess, so this always opens its own console window (incompatible with
# -NoNewWindow) and always goes through cmd.exe /c with the escaped command line
# above, rather than the direct-launch path used in the already-elevated branch -
# meaning this branch is ALWAYS exposed to "%" expansion risk, elevated besides.
# Fail loudly rather than silently risk it - see cap.ps1/Runner.Run for the same
# check on the inline branch.
foreach ($a in $Command) {
    if ($a.Contains('%')) {
        Write-Error "Refusing to run: argument contains '%', which cmd.exe could expand as an environment variable during elevation. See README's Argument handling section."
        exit 1
    }
}

try {
    $p = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/c', $commandLine) -Verb RunAs -Wait -PassThru
} catch {
    Write-Error "Elevation was cancelled or failed: $($_.Exception.Message)"
    exit 1
}

exit $p.ExitCode
