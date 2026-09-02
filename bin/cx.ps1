# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
# No param(): $args sidesteps PowerShell's parameter binder entirely - see
# cap.ps1 for why that matters (codex's own flags shouldn't get bound here).
$Command = @('codex', '--dangerously-bypass-approvals-and-sandbox') + @($args)

# Fallback command line for when codex isn't a directly-launchable .exe (it's
# typically an npm-installed .cmd shim on Windows) - see CxLauncher.Run below and
# cap.ps1 for the same logic and its documented "%" limitation. cx.bat has its own,
# more severe "%" caveat (see there) that applies before this script ever runs.
$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '[\s"&|<>^]') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class CxLauncher
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

    public static int Run(string[] argv, string cmdExeCommandLine)
    {
        var si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(si);
        PROCESS_INFORMATION pi = new PROCESS_INFORMATION();

        // See cap.ps1 for why .bat/.cmd targets skip the direct attempt entirely:
        // CreateProcess silently re-invokes them through cmd.exe on its own, using
        // unescaped text, instead of failing the way a genuinely missing exe would.
        // A bare name like "codex" (typically an npm .cmd shim on Windows) isn't
        // caught by that check, but CreateProcess only ever auto-appends ".exe" to
        // it, so it fails cleanly here and falls through to the escaped path below.
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
            // /d: skip HKCU AutoRun (user-writable registry key). /v:off: disable delayed
            // expansion so "!var!" in an argument can't be expanded. /s plus the extra outer
            // quote pair: cmd's /S rule strips exactly that outer pair and leaves the rest of
            // the string untouched - without /S, cmd strips the first and last quote of the
            // whole line instead, which breaks quoting whenever the target path itself needs
            // quotes AND another argument is also quoted.
            var shellCommandLine = new StringBuilder("\"" + cmdExe + "\" /d /v:off /s /c \"" + cmdExeCommandLine + "\"");
            created = CreateProcess(null, shellCommandLine, IntPtr.Zero, IntPtr.Zero, true,
                0, IntPtr.Zero, null, ref si, out pi);
            if (!created)
                throw new InvalidOperationException("CreateProcess failed: " + Marshal.GetLastWin32Error());
        }

        if (WaitForSingleObject(pi.hProcess, 0xFFFFFFFF) == 0xFFFFFFFF)
        {
            // The child's actual state is unknown here - don't just report
            // failure and potentially leave it running unmanaged in the
            // background. Best-effort kill before giving up.
            int waitErr = Marshal.GetLastWin32Error();
            TerminateProcess(pi.hProcess, 1);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            throw new InvalidOperationException("WaitForSingleObject failed: " + waitErr);
        }

        uint exitCode;
        if (!GetExitCodeProcess(pi.hProcess, out exitCode))
        {
            int exitErr = Marshal.GetLastWin32Error();
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            throw new InvalidOperationException("GetExitCodeProcess failed: " + exitErr);
        }

        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);

        return (int)exitCode;
    }
}
"@

Add-Type -TypeDefinition $source -Language CSharp

try {
    exit ([CxLauncher]::Run([string[]]$Command, $commandLine))
} catch {
    Write-Error $_.Exception.InnerException.Message
    exit 1
}
