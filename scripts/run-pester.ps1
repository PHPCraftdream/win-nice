# Runs the Pester suite in test\pester (npm run test:pester starts it inside
# scripts\lib\guarded.ps1: hidden desktop, process cap, Idle priority).
$ErrorActionPreference = 'Continue'
Import-Module Pester -MaximumVersion 3.99
$result = Invoke-Pester -Path (Join-Path $PSScriptRoot '..\test\pester') -PassThru
if ($result.FailedCount -gt 0) { exit 1 }
exit 0
