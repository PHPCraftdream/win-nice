# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Single entry point for the "already elevated" portion of the Pester suite.
# win-nice.Tests.ps1 has 3 admin.ps1 cases gated by
# -Skip:(-not $script:isAdminRunner) - they only exercise admin.ps1's
# already-elevated branch, which requires the WHOLE test-runner process (not
# just admin.ps1 itself) to already be elevated. Running this script asks for
# elevation exactly once (same self-elevation pattern as bin/uiup.ps1's
# -SelfElevated), then runs the FULL suite inside that one elevated session -
# every test, not just the 3 admin ones, since Invoke-Pester itself has no
# per-test elevation concept.
#
# -Verb RunAs is ShellExecute-based, not CreateProcess (see bin/admin.ps1's own
# comment on the same fact), so it always opens a separate console window and
# this script's own console never sees that window's output directly. The
# elevated child instead writes a transcript to -LogPath, which this (the
# original, unelevated) invocation reads back and prints once -Wait returns.
param(
    [switch]$SelfElevated,
    [string]$LogPath
)

$testPath = Join-Path $PSScriptRoot 'win-nice.Tests.ps1'

if ($SelfElevated) {
    # Running inside the elevated child window.
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

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($isAdmin) {
    # Already running elevated (e.g. launched from an admin shell) - no second
    # UAC round-trip needed, run inline so output goes straight to this console.
    $ErrorActionPreference = 'Continue'
    Import-Module Pester -MaximumVersion 3.99
    $result = Invoke-Pester -Path $testPath -PassThru
    exit ([int]($result.FailedCount -gt 0))
}

Write-Host "Not elevated - one UAC prompt will run the full Pester suite (including the 3 admin.ps1 cases that only exercise its already-elevated branch) in a separate elevated window. Output is relayed back here once it finishes."
$logFile = [System.IO.Path]::GetTempFileName()
try {
    $p = Start-Process powershell -Verb RunAs -ArgumentList @(
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
