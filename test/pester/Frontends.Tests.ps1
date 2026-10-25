# SPDX-License-Identifier: MIT OR Apache-2.0
# Installed frontends driven from PowerShell: exit codes, usage errors, real
# limit effects, and argument passing from PowerShell's native-command call.

. (Join-Path $PSScriptRoot 'Helpers.ps1')

Describe 'installed frontends from PowerShell' {
    $root = New-TestRoot
    $bin = if ($script:hasExe) { New-ToolDir -Root $root } else { $null }

    It 'wnice propagates the wrapped exit code' -Skip:(-not $script:hasExe) {
        (Invoke-Tool $bin 'wnice' @('-c', '50', '-n', '10', '--', $script:cmdPath, '/c', 'exit', '7')).ExitCode | Should Be 7
    }

    It '<Tool> <Value> propagates the wrapped exit code' -Skip:(-not $script:hasExe) -TestCases @(
        @{ Tool = 'capc'; Value = '50'; Code = 3 },
        @{ Tool = 'capt'; Value = '1'; Code = 4 },
        @{ Tool = 'capm'; Value = '90'; Code = 6 },
        @{ Tool = 'caps'; Value = '30'; Code = 8 },
        @{ Tool = 'capn'; Value = '10'; Code = 9 }
    ) {
        param($Tool, $Value, $Code)
        (Invoke-Tool $bin $Tool @($Value, $script:cmdPath, '/c', 'exit', "$Code")).ExitCode | Should Be $Code
    }

    It '<Tool> propagates the wrapped exit code' -Skip:(-not $script:hasExe) -TestCases @(
        @{ Tool = 'idle' }, @{ Tool = 'belownormal' }, @{ Tool = 'abovenormal' }
    ) {
        param($Tool)
        (Invoke-Tool $bin $Tool @($script:cmdPath, '/c', 'exit', '5')).ExitCode | Should Be 5
    }

    It 'usage errors exit 1 with the tool''s own usage line' -Skip:(-not $script:hasExe) {
        $r = Invoke-Tool $bin 'capc' @('101', 'cmd')
        $r.ExitCode | Should Be 1
        $r.Output | Should Match 'usage: capc <percent 1-100>'
        $r = Invoke-Tool $bin 'idle'
        $r.ExitCode | Should Be 1
        $r.Output | Should Match 'usage: idle <command>'
        $r = Invoke-Tool $bin 'admin'
        $r.ExitCode | Should Be 1
        $r.Output | Should Match 'usage: admin <command>'
    }

    It 'caps kills the whole tree at the deadline and exits 124' -Skip:(-not $script:hasExe) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $r = Invoke-Tool $bin 'caps' @('1', $script:powerShellPath, '-NoProfile', '-Command', 'Start-Sleep 30')
        $sw.Stop()
        $r.ExitCode | Should Be 124
        $r.Output | Should Match 'timed out after 1s'
        $sw.Elapsed.TotalSeconds | Should BeLessThan 20
    }

    It 'capm refuses an allocation above the job memory limit' -Skip:(-not $script:hasExe) {
        $alloc = '$ErrorActionPreference = ''Stop''; $b = New-Object byte[] (300MB); exit 0'
        (Invoke-Tool $bin 'capm' @('150m', $script:powerShellPath, '-NoProfile', '-Command', $alloc)).ExitCode | Should Not Be 0
        (Invoke-Tool $bin 'capm' @('2g', $script:powerShellPath, '-NoProfile', '-Command', $alloc)).ExitCode | Should Be 0
    }

    It '<Tool> gives the wrapped process <Class> priority' -Skip:((-not $script:hasExe) -or $script:priorityPinned) -TestCases @(
        @{ Tool = 'idle'; Class = 'Idle' },
        @{ Tool = 'belownormal'; Class = 'BelowNormal' },
        @{ Tool = 'abovenormal'; Class = 'AboveNormal' }
    ) {
        param($Tool, $Class)
        $r = Invoke-Tool $bin $Tool @($script:powerShellPath, '-NoProfile', '-Command', '(Get-Process -Id $PID).PriorityClass')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be $Class
    }

    It 'arguments pass from PowerShell through the exe to the target unchanged' -Skip:(-not $script:hasExe) {
        $probe = New-ArgvProbe -Root $root
        $out = Join-Path $root 'argv.txt'
        $env:WIN_NICE_TEST_OUT = $out
        try {
            # No trailing-backslash case: Windows PowerShell 5.1 itself mis-quotes it
            # on the way into the first exe (the Rust suite covers exe -> target).
            $passed = @('a b', '&', '|', '%PATH%', '-p', '--', '^x', 'C:\dir with space')
            $r = Invoke-Tool $bin 'capc' (@('50', $script:powerShellPath, '-NoProfile', '-File', $probe) + $passed)
            $r.ExitCode | Should Be 0
            (Get-Content -LiteralPath $out) -join '|' | Should Be ($passed -join '|')
        } finally {
            Remove-Item Env:\WIN_NICE_TEST_OUT -ErrorAction SilentlyContinue
        }
    }

    It 'repeated invocations in one session keep working' -Skip:(-not $script:hasExe) {
        foreach ($code in 11, 12, 13) {
            (Invoke-Tool $bin 'capc' @('50', $script:cmdPath, '/c', 'exit', "$code")).ExitCode | Should Be $code
        }
    }

    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
