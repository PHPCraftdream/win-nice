# SPDX-License-Identifier: MIT OR Apache-2.0
# Shared fixture for the Pester suite: the installed layout (wnice.exe copied
# as every tool name) in a private temp dir, plus small probes. Dot-sourced by
# every *.Tests.ps1 file in this directory.

$script:repoRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$script:wniceExe = Join-Path $script:repoRoot 'bin\wnice.exe'
$script:hasExe = Test-Path $script:wniceExe
$script:noExeReason = 'bin\wnice.exe is missing - run "npm run build:exe" (CI downloads it)'

# Fixed hosts, never resolved through cwd/PATH.
$script:powerShellPath = Join-Path ([Environment]::SystemDirectory) 'WindowsPowerShell\v1.0\powershell.exe'
$script:cmdPath = Join-Path ([Environment]::SystemDirectory) 'cmd.exe'

$script:isAdminRunner = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
# The developer sandbox (scripts/lib/guarded.ps1) pins the whole tree to Idle,
# so priority effects are only observable outside it (CI runs unguarded).
$script:priorityPinned = (Get-Process -Id $PID).PriorityClass -eq 'Idle'

$script:toolNames = @('wnice', 'idle', 'belownormal', 'abovenormal', 'high', 'realtime',
    'capc', 'capt', 'capm', 'caps', 'capn', 'cy', 'cx', 'admin', 'uiup')

function New-TestRoot {
    $dir = Join-Path ([IO.Path]::GetTempPath()) ('win-nice-pester-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    return $dir
}

# <root>\bin: wnice.exe copied as every tool name (what postinstall produces).
function New-ToolDir {
    param([Parameter(Mandatory = $true)][string]$Root)
    $bin = Join-Path $Root 'bin'
    New-Item -ItemType Directory -Path $bin -Force | Out-Null
    foreach ($t in $script:toolNames) {
        Copy-Item -LiteralPath $script:wniceExe -Destination (Join-Path $bin "$t.exe")
    }
    return $bin
}

# Runs <tool>.exe with $ToolArgs; returns exit code plus merged output text.
function Invoke-Tool {
    param(
        [Parameter(Mandatory = $true)][string]$Bin,
        [Parameter(Mandatory = $true)][string]$Tool,
        [string[]]$ToolArgs = @()
    )
    $out = & (Join-Path $Bin "$Tool.exe") @ToolArgs 2>&1 | Out-String
    return [PSCustomObject]@{ ExitCode = $LASTEXITCODE; Output = $out.Trim() }
}

# A PowerShell probe that writes its own argv (one per line) to $OutFile.
function New-ArgvProbe {
    param([Parameter(Mandatory = $true)][string]$Root)
    $probe = Join-Path $Root 'argv-probe.ps1'
    Set-Content -Path $probe -Value 'Set-Content -LiteralPath $env:WIN_NICE_TEST_OUT -Value $args'
    return $probe
}
