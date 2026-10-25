# SPDX-License-Identifier: MIT OR Apache-2.0
# Tools found through PATH: chains, .bat targets through the cmd.exe fallback,
# the '%' refusal, admin routing (never reaching a UAC prompt) and cy/cx.

. (Join-Path $PSScriptRoot 'Helpers.ps1')

Describe 'tools resolved through PATH' {
    $root = New-TestRoot
    $bin = if ($script:hasExe) { New-ToolDir -Root $root } else { $null }
    $savedPath = $env:PATH
    if ($bin) { $env:PATH = "$bin;$env:PATH" }

    It 'a chain of bare tool names runs to the target and keeps its exit code' -Skip:(-not $script:hasExe) {
        & capc 50 capm 90 capn 10 idle cmd /c exit 5
        $LASTEXITCODE | Should Be 5
    }

    It 'a .bat target in a path with a space runs through cmd.exe' -Skip:(-not $script:hasExe) {
        $dir = Join-Path $root 'with space'
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $bat = Join-Path $dir 'exit-with.bat'
        Set-Content -Path $bat -Value "@echo off`r`nexit /b %1"
        (Invoke-Tool $bin 'capc' @('50', $bat, '6')).ExitCode | Should Be 6
    }

    It 'a % argument for a .bat target is refused before anything runs' -Skip:(-not $script:hasExe) {
        $bat = Join-Path $root 'marker.bat'
        $marker = Join-Path $root 'marker-ran.txt'
        Set-Content -Path $bat -Value "@echo off`r`necho ran> `"$marker`""
        $r = Invoke-Tool $bin 'capc' @('50', $bat, '100%OFF')
        $r.ExitCode | Should Be 1
        $r.Output | Should Match "Refusing to run: argument contains '%'"
        Test-Path $marker | Should Be $false
    }

    It 'admin refuses a % argument on its cmd.exe route without a UAC prompt' -Skip:((-not $script:hasExe) -or $script:isAdminRunner) {
        $r = Invoke-Tool $bin 'admin' @('somebatch.bat', '100%OFF')
        $r.ExitCode | Should Be 1
        $r.Output | Should Match "Refusing to run: argument contains '%'"
    }

    It 'admin runs inline when the runner is already elevated' -Skip:((-not $script:hasExe) -or (-not $script:isAdminRunner)) {
        (Invoke-Tool $bin 'admin' @($script:cmdPath, '/c', 'exit', '4')).ExitCode | Should Be 4
    }

    It '<Tool> runs <Agent> with <Flag> and the caller''s arguments' -Skip:(-not $script:hasExe) -TestCases @(
        @{ Tool = 'cy'; Agent = 'claude'; Flag = '--dangerously-skip-permissions' },
        @{ Tool = 'cx'; Agent = 'codex'; Flag = '--dangerously-bypass-approvals-and-sandbox' }
    ) {
        param($Tool, $Agent, $Flag)
        $fakes = Join-Path $root "fake-$Agent"
        New-Item -ItemType Directory -Path $fakes -Force | Out-Null
        Set-Content -Path (Join-Path $fakes "$Agent.bat") -Value "@echo off`r`necho %*"
        $env:PATH = "$fakes;$bin;$savedPath"
        try {
            $r = Invoke-Tool $bin $Tool @('hello', 'world')
        } finally {
            $env:PATH = "$bin;$savedPath"
        }
        $r.ExitCode | Should Be 0
        $r.Output | Should Be "$Flag hello world"
    }

    $env:PATH = $savedPath
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
