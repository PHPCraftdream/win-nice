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

# $commandLine is re-parsed by cmd.exe (via "cmd.exe /c" below), so quoting must
# neutralize its operators (&|<>^) and not just whitespace - see cap.ps1 for the
# same logic and its documented "%" limitation.
$commandLine = ($Command | ForEach-Object {
    $escaped = $_ -replace '"', '\"'
    if ($escaped -eq '' -or $escaped -match '[\s"&|<>^]') { '"' + $escaped + '"' } else { $escaped }
}) -join ' '

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

try {
    if ($isAdmin) {
        # Already elevated - run inline, sharing the current console.
        $p = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/c', $commandLine) -NoNewWindow -Wait -PassThru
    } else {
        # -Verb RunAs triggers the UAC consent prompt. Incompatible with -NoNewWindow
        # (ShellExecute, not CreateProcess), so this opens its own console window.
        $p = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/c', $commandLine) -Verb RunAs -Wait -PassThru
    }
} catch {
    Write-Error "Elevation was cancelled or failed: $($_.Exception.Message)"
    exit 1
}

exit $p.ExitCode
