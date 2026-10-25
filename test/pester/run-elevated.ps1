# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Single entry point for the "already elevated" portion of the Pester suite.
# The suite in this directory has an admin case gated by
# -Skip:(-not $script:isAdminRunner) (the whole runner must be elevated) and
# one gated by -Skip:$script:isAdminRunner. Running this script asks for
# elevation exactly once, then runs the suite inside that elevated session.
# Combine the result with a normal (non-elevated) run for full coverage.
#
# -Verb RunAs is ShellExecute-based, not CreateProcess, so it always opens a separate console window and
# this script's own console never sees that window's output directly. The
# elevated child instead writes a transcript to -LogPath, which this (the
# original, unelevated) invocation reads back and prints once -Wait returns.
param(
    [switch]$SelfElevated,
    [string]$LogPath
)

$testPath = $PSScriptRoot
# This is a managed maintainer entry point. Resolve the Windows PowerShell
# host from the system directory before the UAC hop; a bare executable name
# would let CreateProcess/ShellExecute search the working directory or PATH.
$powershellPath = [Environment]::SystemDirectory + '\WindowsPowerShell\v1.0\powershell.exe'
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if ($SelfElevated) {
    # -SelfElevated is an internal flag this script passes to its OWN elevated
    # relaunch below - it is not itself proof of elevation. Without this check,
    # anyone could run "run-elevated.ps1 -SelfElevated -LogPath <file>" from a
    # plain console and get a normal (non-elevated) Pester run reported back as
    # if it were the elevated one: the 3 admin-already-elevated cases would
    # silently Skip and the script would still exit 0 - a false elevated-
    # coverage result.
    if (-not $LogPath) {
        Write-Error "run-elevated.ps1: -SelfElevated requires -LogPath (internal flag - not meant to be passed by hand; run without -SelfElevated to trigger real elevation)."
        exit 1
    }
    if (-not $isAdmin) {
        Write-Error "run-elevated.ps1: -SelfElevated was passed but this process is not actually elevated - refusing (this flag is internal; run without -SelfElevated to trigger real elevation)."
        exit 1
    }
    try {
        Start-Transcript -Path $LogPath -Force | Out-Null
        # Same reasoning as ci.yml's Pester step: reset from GitHub Actions'
        # (or an inherited) 'Stop' default so a wrapped tool's Write-Error
        # (non-terminating, captured via 2>&1 inside the suite) doesn't abort
        # a test before its own Should assertion runs.
        $ErrorActionPreference = 'Continue'
        # Pin to Pester 3.4.0's syntax - see CONTRIBUTING.md/README.md.
        Import-Module Pester -MaximumVersion 3.99
        $result = Invoke-Pester -Path $testPath -PassThru
        Stop-Transcript | Out-Null
        exit ([int]($result.FailedCount -gt 0))
    } catch {
        try { Stop-Transcript | Out-Null } catch {}
        Add-Content -Path $LogPath -Value "run-elevated.ps1: unexpected error: $($_.Exception.Message)"
        exit 1
    }
}

if ($isAdmin) {
    # Already running elevated (e.g. launched from an admin shell) - no second
    # UAC round-trip needed, run inline so output goes straight to this console.
    $ErrorActionPreference = 'Continue'
    Import-Module Pester -MaximumVersion 3.99
    $result = Invoke-Pester -Path $testPath -PassThru
    exit ([int]($result.FailedCount -gt 0))
}

Write-Host "Not elevated - one UAC prompt will run the suite (activating the already-elevated admin case) in a separate elevated window. Output is relayed back here once it finishes. Run this suite normally (without elevation) too for full coverage."
$logFile = [System.IO.Path]::GetTempFileName()
try {
    $p = Start-Process -FilePath $powershellPath -Verb RunAs -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-SelfElevated', '-LogPath', "`"$logFile`""
    ) -Wait -PassThru
} catch {
    Write-Error "Elevation was cancelled or failed: $($_.Exception.Message)"
    Remove-Item $logFile -ErrorAction SilentlyContinue
    exit 1
}

if (Test-Path $logFile) {
    Get-Content $logFile
    Remove-Item $logFile -ErrorAction SilentlyContinue
}
exit $p.ExitCode
