# SPDX-License-Identifier: MIT OR Apache-2.0
# win-nice: managed-file
param([switch]$SelfElevated)

$targets = @('explorer', 'dwm', 'sihost', 'ShellExperienceHost', 'StartMenuExperienceHost', 'StartMenu', 'SearchApp', 'audiodg')

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "Not elevated - requesting admin rights (dwm/sihost run under a different account)..."
    try {
        $p = Start-Process powershell -Verb RunAs -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-SelfElevated') -Wait -PassThru
        exit $p.ExitCode
    } catch {
        Write-Error "Elevation was cancelled or failed: $($_.Exception.Message)"
        exit 1
    }
}

$rows = foreach ($name in $targets) {
    $procs = Get-Process -Name $name -ErrorAction SilentlyContinue
    if (-not $procs) {
        [PSCustomObject]@{ Name = $name; Id = '-'; Old = '-'; New = 'not running' }
        continue
    }
    foreach ($proc in $procs) {
        $old = $proc.PriorityClass
        try {
            $proc.PriorityClass = 'High'
            [PSCustomObject]@{ Name = $name; Id = $proc.Id; Old = $old; New = 'High' }
        } catch {
            [PSCustomObject]@{ Name = $name; Id = $proc.Id; Old = $old; New = "FAILED: $($_.Exception.Message)" }
        }
    }
}

$rows | Format-Table -AutoSize
if ($SelfElevated) {
    Write-Host ""
    Write-Host "Done. Press Enter to close..."
    Read-Host | Out-Null
}
