# SPDX-License-Identifier: MIT OR Apache-2.0
# Integration tests against the real Windows APIs (Job Objects, priority classes).
# Run with: Invoke-Pester (built-in Pester 3.4.0 on Windows 10/11 - no install needed).

$root = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $root 'bin'

# -Verb RunAs (admin.ps1's not-elevated branch) needs an interactive UAC click and
# can't be exercised in an automated test. Tests that need real elevation are
# -Skip:(-not $script:isAdminRunner) rather than silently `return`-ing, so a
# non-elevated run reports them as Skipped instead of a plain (misleading) pass.
$script:isAdminRunner = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

# One unique root per RUN (not per test): every temp artifact this suite creates
# lives inside it, and the final cleanup at the bottom of this file removes exactly
# this directory - nothing else under the shared %TEMP%. The old wildcard sweep
# over %TEMP% deleted the still-in-use artifacts of any OTHER concurrent run
# (second checkout, parallel CI matrix, parallel worktrees) that finished later.
$script:testRoot = Join-Path $env:TEMP ('win-nice-pester-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $script:testRoot -Force | Out-Null

function New-TempFile {
    $f = Join-Path $script:testRoot ("win-nice-pester-" + [guid]::NewGuid().ToString("N") + ".tmp")
    Remove-Item $f -ErrorAction SilentlyContinue
    return $f
}

function New-TempScript {
    $f = Join-Path $script:testRoot ("win-nice-pester-" + [guid]::NewGuid().ToString("N") + ".ps1")
    Remove-Item $f -ErrorAction SilentlyContinue
    return $f
}

# Drives $Exe (a .bat or .ps1 win-nice entry point) with $Prefix positional args
# (e.g. a percent/thread-count) followed by a probe invocation carrying $Args, all
# as literal driver-file text - never as live PowerShell arguments, which have their
# own quoting quirks unrelated to what's under test (see individual tests below for
# why). Returns the probe's captured argv, joined by "|SEP|", or $null if it never ran.
function Get-ForwardedArgs {
    param(
        [Parameter(Mandatory = $true)][string]$Exe,
        [string[]]$Prefix = @(),
        [Parameter(Mandatory = $true)][string[]]$ProbeArgs
    )
    $out = New-TempFile
    $probe = 'Set-Content -Path $env:WIN_NICE_TEST_OUT -Value ($args -join ''|SEP|'')'
    $probeFile = New-TempScript
    Set-Content -Path $probeFile -Value $probe
    $quoted = { param($s) '"' + ($s -replace '"', '""') + '"' }
    $prefixText = ($Prefix | ForEach-Object { & $quoted $_ }) -join ' '
    $argsText = ($ProbeArgs | ForEach-Object { & $quoted $_ }) -join ' '
    $driver = (New-TempScript).Replace('.ps1', '.bat')
    Set-Content -Path $driver -Value ("@echo off`r`nset WIN_NICE_TEST_OUT=$out`r`n`"$Exe`" $prefixText powershell -NoProfile -File `"$probeFile`" $argsText`r`n")
    & $driver | Out-Null
    $exitCode = $LASTEXITCODE
    $result = if (Test-Path $out) { (Get-Content $out).Trim() } else { $null }
    Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    return [PSCustomObject]@{ Output = $result; ExitCode = $exitCode }
}

# Same idea as Get-ForwardedArgs, but for a .ps1 entry point invoked directly via
# PowerShell's own "&" (real .exe, no cmd.exe hop) rather than through a .bat driver.
function Get-DirectForwardedArgs {
    param(
        [Parameter(Mandatory = $true)][string]$Ps1,
        [string[]]$Prefix = @(),
        [Parameter(Mandatory = $true)][string[]]$ProbeArgs
    )
    $out = New-TempFile
    $probe = 'Set-Content -Path $env:WIN_NICE_TEST_OUT -Value ($args -join ''|SEP|'')'
    $probeFile = New-TempScript
    Set-Content -Path $probeFile -Value $probe
    $env:WIN_NICE_TEST_OUT = $out
    & powershell -NoProfile -File $Ps1 @Prefix powershell -NoProfile -File $probeFile @ProbeArgs
    $exitCode = $LASTEXITCODE
    Remove-Item Env:\WIN_NICE_TEST_OUT -ErrorAction SilentlyContinue
    $result = if (Test-Path $out) { (Get-Content $out).Trim() } else { $null }
    Remove-Item $out, $probeFile -ErrorAction SilentlyContinue
    return [PSCustomObject]@{ Output = $result; ExitCode = $exitCode }
}

Describe 'idle.bat' {
    It 'fails with a usage message when no command is given' {
        & (Join-Path $bin 'idle.bat') 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'runs the given command at Idle priority' {
        $out = New-TempFile
        & (Join-Path $bin 'idle.bat') powershell -NoProfile -Command "(Get-Process -Id `$PID).PriorityClass | Out-File -FilePath '$out'"
        (Get-Content $out).Trim() | Should Be 'Idle'
        Remove-Item $out -ErrorAction SilentlyContinue
    }

    It 'propagates Idle priority to the whole spawned process tree' {
        $out = New-TempFile
        $script = @'
$c = Start-Process cmd -ArgumentList "/c ping -n 3 127.0.0.1 >nul" -WindowStyle Hidden -PassThru
try {{
    Start-Sleep -Milliseconds 500
    $c.Refresh()
    Set-Content -Path '{0}' -Value $c.PriorityClass
    # ping -n 3 takes ~2s: 15s is a generous upper bound, so a wedged probe can't
    # hang the suite indefinitely (a bare WaitForExit() here once blocked 206s).
    if (-not $c.WaitForExit(15000)) {{ $c.Kill() }}
}} finally {{
    if (-not $c.HasExited) {{ $c.Kill() }}
}}
'@ -f $out
        $scriptFile = New-TempScript
        Set-Content -Path $scriptFile -Value $script
        & (Join-Path $bin 'idle.bat') powershell -NoProfile -File $scriptFile
        (Get-Content $out).Trim() | Should Be 'Idle'
        Remove-Item $out, $scriptFile -ErrorAction SilentlyContinue
    }

    It 'propagates the exit code of the wrapped command' {
        & (Join-Path $bin 'idle.bat') cmd /c "exit 7"
        $LASTEXITCODE | Should Be 7
    }

    It 'preserves cmd.exe metacharacters (&, |, <, >, ^) as literal argument text' {
        # idle.bat forwards a raw %* straight to "start"; unlike capc.bat/capc.ps1 it
        # doesn't rebuild the command line itself, so this only needs to lock in
        # today's correct behavior against a future regression.
        $out = New-TempFile
        $probe = @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$a)
Set-Content -Path '{0}' -Value ($a -join '|SEP|')
'@ -f $out
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $idleBat = Join-Path $bin 'idle.bat'
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`n`"$idleBat`" powershell -NoProfile -File `"$probeFile`" `"A&B`" `"A|B`" `"A<B>C`" `"A^B`"`r`n"
        & $driver
        $LASTEXITCODE | Should Be 0
        (Get-Content $out).Trim() | Should Be 'A&B|SEP|A|B|SEP|A<B>C|SEP|A^B'
        Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    }

    It 'documents a known limitation: a literal "%" is corrupted by idle.bat itself' {
        # Same cmd.exe batch-parameter quirk as capc.bat - see the capc.ps1 test of the
        # same name. Invoking "idle" bare from PowerShell (idle.ps1 preferred) does not
        # have this problem - see idle.ps1's own "%" test below.
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'idle.bat') -ProbeArgs @('100%OFF')
        $r.Output | Should Be '100OFF'
    }
}

Describe 'idle.ps1' {
    It 'runs the given command at Idle priority' {
        $out = New-TempFile
        & powershell -NoProfile -File (Join-Path $bin 'idle.ps1') powershell -NoProfile -Command "(Get-Process -Id `$PID).PriorityClass | Out-File -FilePath '$out'"
        (Get-Content $out).Trim() | Should Be 'Idle'
        Remove-Item $out -ErrorAction SilentlyContinue
    }

    It 'preserves cmd.exe metacharacters and a literal "%" (no cmd.exe hop for a direct .exe target)' {
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'idle.ps1') -ProbeArgs @('A&B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|100%OFF'
    }
}

Describe 'belownormal.bat' {
    It 'fails with a usage message when no command is given' {
        & (Join-Path $bin 'belownormal.bat') 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'runs the given command at BelowNormal priority' {
        $out = New-TempFile
        & (Join-Path $bin 'belownormal.bat') powershell -NoProfile -Command "(Get-Process -Id `$PID).PriorityClass | Out-File -FilePath '$out'"
        (Get-Content $out).Trim() | Should Be 'BelowNormal'
        Remove-Item $out -ErrorAction SilentlyContinue
    }
}

Describe 'belownormal.ps1' {
    It 'runs the given command at BelowNormal priority' {
        $out = New-TempFile
        & powershell -NoProfile -File (Join-Path $bin 'belownormal.ps1') powershell -NoProfile -Command "(Get-Process -Id `$PID).PriorityClass | Out-File -FilePath '$out'"
        (Get-Content $out).Trim() | Should Be 'BelowNormal'
        Remove-Item $out -ErrorAction SilentlyContinue
    }

    It 'preserves cmd.exe metacharacters and a literal "%" (no cmd.exe hop for a direct .exe target)' {
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'belownormal.ps1') -ProbeArgs @('A&B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|100%OFF'
    }
}

# realtime is downgraded to High without SeIncreaseBasePriorityPrivilege - elevated
# processes have it by default, unelevated ones don't - so the expectation follows
# $script:isAdminRunner rather than a hardcoded literal. See README for why.
$priorityTools = @(
    @{ Name = 'abovenormal'; Expected = 'AboveNormal' }
    @{ Name = 'high'; Expected = 'High' }
    @{ Name = 'realtime'; Expected = if ($script:isAdminRunner) { 'RealTime' } else { 'High' } }
)

Describe 'abovenormal.bat / high.bat / realtime.bat' {
    It 'fails with a usage message when no command is given (<Name>)' -TestCases $priorityTools {
        param($Name, $Expected)
        & (Join-Path $bin "$Name.bat") 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }
}

# Group 6 regression: all 5 of these .bat files used to be "start ... /b /wait %*",
# which disables Ctrl+C for the wrapped command - now thin shims delegating to
# their own .ps1 ("powershell ... -File %~dp0<name>.ps1 %*"). Table-driven,
# matching the priority-class and exit-code assertions the equivalent .ps1 tests
# above already make, to confirm the rewrite didn't change either observable
# behavior.
$allPriorityBatTools = @(
    @{ Name = 'idle'; Expected = 'Idle' }
    @{ Name = 'belownormal'; Expected = 'BelowNormal' }
    @{ Name = 'abovenormal'; Expected = 'AboveNormal' }
    @{ Name = 'high'; Expected = 'High' }
    @{ Name = 'realtime'; Expected = if ($script:isAdminRunner) { 'RealTime' } else { 'High' } }
)

Describe 'idle.bat / belownormal.bat / abovenormal.bat / high.bat / realtime.bat (priority + exit code)' {
    It 'runs the given command at the expected priority (<Name> -> <Expected>)' -TestCases $allPriorityBatTools {
        param($Name, $Expected)
        $out = New-TempFile
        try {
            & (Join-Path $bin "$Name.bat") powershell -NoProfile -Command "(Get-Process -Id `$PID).PriorityClass | Out-File -FilePath '$out'"
            (Get-Content $out).Trim() | Should Be $Expected
        } finally {
            Remove-Item $out -ErrorAction SilentlyContinue
        }
    }

    It 'propagates the exit code of the wrapped command (<Name>)' -TestCases $allPriorityBatTools {
        param($Name, $Expected)
        & (Join-Path $bin "$Name.bat") cmd /c "exit 7"
        $LASTEXITCODE | Should Be 7
    }
}

Describe 'abovenormal.ps1 / high.ps1 / realtime.ps1' {
    It 'runs the given command at the expected priority (<Name> -> <Expected>)' -TestCases $priorityTools {
        param($Name, $Expected)
        $out = New-TempFile
        & powershell -NoProfile -File (Join-Path $bin "$Name.ps1") powershell -NoProfile -Command "(Get-Process -Id `$PID).PriorityClass | Out-File -FilePath '$out'"
        (Get-Content $out).Trim() | Should Be $Expected
        Remove-Item $out -ErrorAction SilentlyContinue
    }

    It 'does not propagate priority to a spawned grandchild (<Name>)' -TestCases $priorityTools {
        param($Name, $Expected)
        $out = New-TempFile
        $script = @'
$c = Start-Process cmd -ArgumentList "/c ping -n 3 127.0.0.1 >nul" -WindowStyle Hidden -PassThru
try {{
    Start-Sleep -Milliseconds 500
    $c.Refresh()
    Set-Content -Path '{0}' -Value $c.PriorityClass
    # ping -n 3 takes ~2s: 15s is a generous upper bound, so a wedged probe can't
    # hang the suite indefinitely (a bare WaitForExit() here once blocked 206s).
    if (-not $c.WaitForExit(15000)) {{ $c.Kill() }}
}} finally {{
    if (-not $c.HasExited) {{ $c.Kill() }}
}}
'@ -f $out
        $scriptFile = New-TempScript
        Set-Content -Path $scriptFile -Value $script
        & powershell -NoProfile -File (Join-Path $bin "$Name.ps1") powershell -NoProfile -File $scriptFile
        (Get-Content $out).Trim() | Should Be 'Normal'
        Remove-Item $out, $scriptFile -ErrorAction SilentlyContinue
    }

    It 'preserves cmd.exe metacharacters and a literal "%" (<Name>)' -TestCases $priorityTools {
        param($Name, $Expected)
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin "$Name.ps1") -ProbeArgs @('A&B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|100%OFF'
    }
}

Describe 'capc.ps1 argument validation' {
    It 'rejects a non-numeric percent' {
        & (Join-Path $bin 'capc.bat') abc cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects percent below 1' {
        & (Join-Path $bin 'capc.bat') 0 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects percent above 100' {
        & (Join-Path $bin 'capc.bat') 101 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a missing command' {
        & (Join-Path $bin 'capc.bat') 50 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'accepts the boundary values 1 and 100' {
        & (Join-Path $bin 'capc.bat') 1 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
        & (Join-Path $bin 'capc.bat') 100 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
    }
}

Describe 'capc.ps1 behavior' {
    It 'propagates the exit code of the wrapped command' {
        & (Join-Path $bin 'capc.bat') 50 cmd /c "exit 3"
        $LASTEXITCODE | Should Be 3
    }

    It 'preserves an empty-string argument through to the wrapped command' {
        # The empty "" argument is baked into a static .bat file's text rather than
        # passed as a live PowerShell argument - Windows PowerShell 5.1's `&` drops
        # literal "" arguments to native commands before they ever reach capc.bat,
        # which would test a PowerShell quirk instead of capc.ps1's own quoting.
        # The probe itself must bind its args via ValueFromRemainingArguments - a
        # plain [string[]] positional parameter has its own PS 5.1 -File quirk that
        # silently truncates the array at an empty element, independent of capc.ps1.
        $out = New-TempFile
        $probe = @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$a)
Set-Content -Path '{0}' -Value ($a.Count.ToString() + "|" + ($a -join ","))
'@ -f $out
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $capcBat = Join-Path $bin 'capc.bat'
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`n`"$capcBat`" 50 powershell -NoProfile -File `"$probeFile`" AAA `"`" BBB`r`n"
        & $driver
        (Get-Content $out).Trim() | Should Be '3|AAA,,BBB'
        Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    }

    It 'preserves cmd.exe metacharacters (&, |, <, >, ^) as literal argument text' {
        # Unescaped, these would be re-parsed by the "cmd.exe /c" hop inside capc.ps1
        # and split the wrapped command into separate commands (or drop the caret).
        $out = New-TempFile
        $probe = @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$a)
Set-Content -Path '{0}' -Value ($a -join '|SEP|')
'@ -f $out
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $capcBat = Join-Path $bin 'capc.bat'
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`n`"$capcBat`" 50 powershell -NoProfile -File `"$probeFile`" `"A&B`" `"A|B`" `"A<B>C`" `"A^B`"`r`n"
        & $driver
        $LASTEXITCODE | Should Be 0
        (Get-Content $out).Trim() | Should Be 'A&B|SEP|A|B|SEP|A<B>C|SEP|A^B'
        Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    }

    It 'forwards flags that collide with PowerShell common parameters (e.g. -e, -Verbose) untouched' {
        # capc.ps1 must not bind these as -ErrorAction/-Verbose itself; node -e is the
        # motivating real-world case. The probe below deliberately uses bare $args
        # (no [Parameter()] attribute) for the same reason capc.ps1 does - a declared
        # ValueFromRemainingArguments parameter would make the *probe* itself subject
        # to the same common-parameter ambiguity being tested here.
        $out = New-TempFile
        $probe = @'
Set-Content -Path $env:WIN_NICE_TEST_OUT -Value ($args -join '|SEP|')
'@
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $capcBat = Join-Path $bin 'capc.bat'
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`nset WIN_NICE_TEST_OUT=$out`r`n`"$capcBat`" 50 powershell -NoProfile -File `"$probeFile`" -e 0 -Verbose`r`n"
        & $driver
        $LASTEXITCODE | Should Be 0
        (Get-Content $out).Trim() | Should Be '-e|SEP|0|SEP|-Verbose'
        Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    }

    It 'forwards a flag that would ambiguously prefix-match the declared -Percent parameter name (e.g. -p)' {
        # Regression test: capc.ps1 used to declare $Percent via param(), and even
        # without [Parameter()] attributes, PowerShell's binder still prefix-matches
        # "-p" against a declared parameter named "Percent" and rebinds it.
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'capc.bat') -Prefix @('50') -ProbeArgs @('-p', '0')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '-p|SEP|0'
    }

    It 'preserves a literal "%" when the target is a directly-launchable .exe' {
        # No cmd.exe involved at all on this path (see capc.ps1's Capper.Run) - unlike
        # the cmd.exe /c fallback path, "%" isn't at risk of environment-variable
        # expansion here. Must invoke capc.ps1 directly (not through capc.bat, which has
        # its own separate, documented "%" corruption at the %* forwarding step).
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'capc.ps1') -Prefix @('50') -ProbeArgs @('100%OFF', '50%50')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '100%OFF|SEP|50%50'
    }

    It 'still preserves cmd.exe metacharacters when the target is a .bat file (fallback path)' {
        $targetBat = New-TempScript
        $targetBat = $targetBat.Replace('.ps1', '.bat')
        Set-Content -Path $targetBat -Value "@echo off`r`necho BATOUT=%*`r`n"
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`n`"$(Join-Path $bin 'capc.bat')`" 50 `"$targetBat`" `"A&B`" `"A|B`"`r`n"
        $out = & $driver
        $LASTEXITCODE | Should Be 0
        ($out | Select-Object -Last 1) | Should Be 'BATOUT="A&B" "A|B"'
        Remove-Item $targetBat, $driver -ErrorAction SilentlyContinue
    }

    It 'documents a known limitation: a literal "%" is corrupted by capc.bat itself, before capc.ps1 ever runs' {
        # Confirmed with nothing more than a bare "echo %1" in a plain .bat file - this
        # is cmd.exe's own batch-parameter substitution rescanning %1/%* for %...%
        # patterns, unrelated to capc.ps1's escaping and not fixable from inside a .bat.
        # Invoking "capc" bare from an actual PowerShell session (capc.ps1 preferred over
        # capc.bat) does not have this problem - see the "%" test above.
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'capc.bat') -Prefix @('50') -ProbeArgs @('100%OFF')
        $r.Output | Should Be '100OFF'
    }

    It 'holds CPU usage of a busy single process measurably below the uncapped baseline' {
        $burn = @'
param([int]$Threads, [int]$Seconds)
$proc = [Diagnostics.Process]::GetCurrentProcess()
$cpuStart = $proc.TotalProcessorTime
$wallStart = Get-Date
$pool = [runspacefactory]::CreateRunspacePool(1, $Threads)
$pool.Open()
$tasks = 0..($Threads - 1) | ForEach-Object {
    $ps = [powershell]::Create()
    $ps.RunspacePool = $pool
    [void]$ps.AddScript({ param($sec) $sw = [Diagnostics.Stopwatch]::StartNew(); $x = 0; while ($sw.Elapsed.TotalSeconds -lt $sec) { $x = $x + 1 } }).AddArgument($Seconds)
    [PSCustomObject]@{ Pipe = $ps; Handle = $ps.BeginInvoke() }
}
foreach ($t in $tasks) { $t.Pipe.EndInvoke($t.Handle) | Out-Null; $t.Pipe.Dispose() }
$pool.Close()
$proc.Refresh()
$cpuSeconds = ($proc.TotalProcessorTime - $cpuStart).TotalSeconds
$wallSeconds = ((Get-Date) - $wallStart).TotalSeconds
$pct = ($cpuSeconds / ($wallSeconds * [Environment]::ProcessorCount)) * 100
Write-Output ("{0:N1}" -f $pct)
'@
        $burnFile = New-TempScript
        Set-Content -Path $burnFile -Value $burn
        $threads = [Environment]::ProcessorCount
        $seconds = 4
        $cap = 30

        # Two absolute, separately-timed measurements are inherently noisy on a
        # loaded machine (confirmed: baseline as low as 37% has been observed here
        # even with no cap at all, from unrelated system load) - retry a few times
        # and accept the first attempt with a clean signal, rather than failing on
        # a single noisy sample. Relative thresholds (vs. $cap and vs. $baseline),
        # not absolute ones - an absolute "baseline must exceed cap+20" gate was
        # tried and rejected valid signal on a loaded machine (baseline=37.3,
        # capped=25.6 - a real, working cap - got skipped for "baseline too low").
        # 10 attempts, not 8: a sustained contention spike (e.g. this test running
        # right after a long, heavy back-to-back Pester run) can poison every
        # attempt's baseline in an 8-attempt window too - confirmed again:
        # isolated re-run passed cleanly in 10.0s immediately after an 8-attempt
        # exhaustion inside a full-suite run (206 passed/1 failed/3 skipped, 338s)
        # with heavy concurrent process-spawning load from other Describe blocks.
        $passed = $false
        $lastBaseline = $null
        $lastCapped = $null
        for ($attempt = 1; $attempt -le 10 -and -not $passed; $attempt++) {
            $baseline = [double](powershell -NoProfile -File $burnFile $threads $seconds)
            $cappedOut = & (Join-Path $bin 'capc.bat') $cap powershell -NoProfile -File $burnFile $threads $seconds
            $capped = [double]($cappedOut | Select-Object -Last 1)
            $lastBaseline = $baseline
            $lastCapped = $capped

            # A contention-poisoned baseline (system too busy to show what "uncapped"
            # looks like even has room to exceed the cap) can't validate anything -
            # retry instead of asserting on a meaningless comparison.
            if ($baseline -lt ($cap * 1.15)) { continue }

            # Not just "below baseline" - a badly wrong cap (e.g. barely below 100%)
            # would also pass that alone. Generous tolerance for scheduler jitter.
            if ($capped -lt $baseline -and $capped -lt ($cap + 15)) { $passed = $true }
        }

        if (-not $passed) { Write-Host "last attempt: baseline=$lastBaseline capped=$lastCapped cap=$cap" }
        $passed | Should Be $true
        Remove-Item $burnFile -ErrorAction SilentlyContinue
    }
}

# Table-driven regression test for the "%"-fail-closed check added in b572594: each
# launcher's embedded C# Run() throws before ever calling CreateProcess when the
# cmd.exe fallback branch (.bat/.cmd target) sees an argument containing "%". Must
# invoke the .ps1 directly, not the .bat wrapper - the .bat wrapper corrupts a
# literal "%" itself before .ps1 ever runs (see the "known limitation" tests above),
# which would test the wrong layer. Marker-file-on-success (same technique as the
# capc.ps1 "fallback path" test above) proves the target never actually launched.
$fallbackPercentTools = @(
    @{ Name = 'idle'; Prefix = @() }
    @{ Name = 'belownormal'; Prefix = @() }
    @{ Name = 'abovenormal'; Prefix = @() }
    @{ Name = 'high'; Prefix = @() }
    @{ Name = 'realtime'; Prefix = @() }
    @{ Name = 'capc'; Prefix = @('50') }
    @{ Name = 'capt'; Prefix = @('1') }
    @{ Name = 'capm'; Prefix = @('100m') }
    @{ Name = 'caps'; Prefix = @('30') }
    @{ Name = 'capn'; Prefix = @('10') }
)

Describe '%-fail-closed on the cmd.exe fallback path' {
    It 'refuses to run and never launches the target when an argument contains "%" (<Name>)' -TestCases $fallbackPercentTools {
        param($Name, $Prefix)
        $marker = New-TempFile
        Remove-Item $marker -ErrorAction SilentlyContinue
        $targetBat = New-TempScript
        $targetBat = $targetBat.Replace('.ps1', '.bat')
        Set-Content -Path $targetBat -Value "@echo off`r`n(echo ran)>`"$marker`"`r`n"
        $ps1 = Join-Path $bin "$Name.ps1"
        $stderr = & powershell -NoProfile -File $ps1 @Prefix $targetBat '100%OFF' 2>&1
        $exitCode = $LASTEXITCODE
        $exitCode | Should Be 1
        # The child powershell wraps Write-Error text at the console buffer width
        # before 2>&1 captures it, so match whitespace-normalized text: the raw text
        # only matches when the wrap point happens to fall outside the pattern.
        (($stderr | Out-String) -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
        Test-Path $marker | Should Be $false
        Remove-Item $targetBat, $marker -ErrorAction SilentlyContinue
    }
}

# P1 regression: the cmd.exe /c fallback used to build its command line as
# `"<cmd.exe>" /c <cmdExeCommandLine>` with no /S and no outer quote pair. cmd's
# /C quote-stripping rule only cleanly strips a lone outer quote pair; as soon as
# the target path itself needs quoting (e.g. contains a space) AND at least one
# other argument is also quoted, cmd falls back to stripping the first and last
# quote characters anywhere on the line instead - splitting the path at its space
# and reopening the "&" injection the whole quoting layer exists to prevent. Fixed
# via "/d /s /v:off" plus wrapping cmdExeCommandLine in an extra outer quote pair
# (see any launcher's Run() for the exact rationale). Reproduced pre-fix with:
#   powershell -File bin\capc.ps1 50 "<TEMP>\wn review N\t.bat" "A&B" plain
# -> "'...\wn' is not recognized ...", "'B' is not recognized ...", exit=1
function Test-SpacedTargetFallback {
    param(
        [Parameter(Mandatory = $true)][string]$Ps1,
        [string[]]$Prefix = @()
    )
    $spacedDir = Join-Path $script:testRoot ("win-nice-pester-spaced " + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $spacedDir | Out-Null
    $targetBat = Join-Path $spacedDir 't.bat'
    Set-Content -Path $targetBat -Value "@echo off`r`necho BATOUT=%*`r`n"
    try {
        $stdout = & powershell -NoProfile -File $Ps1 @Prefix $targetBat 'A&B' 'plain'
        $exitCode = $LASTEXITCODE
        return [PSCustomObject]@{ Output = ($stdout | Select-Object -Last 1); ExitCode = $exitCode }
    } finally {
        Remove-Item $spacedDir -Recurse -ErrorAction SilentlyContinue
    }
}

$spacedFallbackTools = @(
    @{ Name = 'idle'; Prefix = @() }
    @{ Name = 'belownormal'; Prefix = @() }
    @{ Name = 'abovenormal'; Prefix = @() }
    @{ Name = 'high'; Prefix = @() }
    @{ Name = 'realtime'; Prefix = @() }
    @{ Name = 'capc'; Prefix = @('50') }
    @{ Name = 'capt'; Prefix = @('1') }
    @{ Name = 'capm'; Prefix = @('100m') }
    @{ Name = 'caps'; Prefix = @('30') }
    @{ Name = 'capn'; Prefix = @('10') }
)

Describe 'cmd.exe fallback quoting survives a target path containing a space (<Name>)' {
    It 'runs successfully and forwards an "&"-containing argument intact (<Name>)' -TestCases $spacedFallbackTools {
        param($Name, $Prefix)
        $r = Test-SpacedTargetFallback -Ps1 (Join-Path $bin "$Name.ps1") -Prefix $Prefix
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'BATOUT="A&B" plain'
    }

    It 'runs successfully and forwards an "&"-containing argument intact (admin, already elevated)' -Skip:(-not $script:isAdminRunner) {
        $r = Test-SpacedTargetFallback -Ps1 (Join-Path $bin 'admin.ps1') -Prefix @()
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'BATOUT="A&B" plain'
    }
}

# cy.ps1/cx.ps1's own target ("claude"/"codex") is a hardcoded bare word, never a
# user-controlled path, so the "quoted target path" trigger above can't occur for
# them directly - but Test-FakeLauncher's fake stand-in directory name (below)
# includes a space, so every cy.ps1/cx.ps1 test that goes through it (including
# the "A&B" case) already exercises the fix's compatibility with a spaced PATH
# entry resolved by cmd.exe's own PATHEXT search.

Describe 'capt.ps1 argument validation' {
    It 'rejects a non-numeric thread count' {
        & (Join-Path $bin 'capt.bat') abc cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a count below 1' {
        & (Join-Path $bin 'capt.bat') 0 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a count above the logical processor count' {
        $tooMany = [Environment]::ProcessorCount + 1
        & (Join-Path $bin 'capt.bat') $tooMany cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a missing command' {
        & (Join-Path $bin 'capt.bat') 1 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'accepts the boundary value 1' {
        & (Join-Path $bin 'capt.bat') 1 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
    }
}

Describe 'capt.ps1 behavior' {
    # 1, not a higher count: these tests only care about exit-code propagation,
    # argument forwarding, and metacharacter preservation - none of that needs
    # more than one logical processor, and capt itself supports thread-count 1
    # with no minimum-processor-count requirement documented anywhere. A
    # hardcoded higher count here would fail argument validation before ever
    # reaching the behavior under test on a genuinely 1-processor machine
    # (release review 1745-708cb53 P2).
    It 'propagates the exit code of the wrapped command' {
        & (Join-Path $bin 'capt.bat') 1 cmd /c "exit 3"
        $LASTEXITCODE | Should Be 3
    }

    It 'forwards a flag that would ambiguously prefix-match the declared -Count parameter name (e.g. -c)' {
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'capt.bat') -Prefix @('1') -ProbeArgs @('-c', '0')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '-c|SEP|0'
    }

    It 'preserves cmd.exe metacharacters and a literal "%" on the direct-launch path' {
        # Must invoke capt.ps1 directly (not through capt.bat, which has its own
        # separate, documented "%" corruption at the %* forwarding step).
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'capt.ps1') -Prefix @('1') -ProbeArgs @('A&B', 'A|B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|A|B|SEP|100%OFF'
    }

    It 'pins the process to exactly the first N logical processors' {
        # min(3, ProcessorCount): the point of this test is proving "first N,
        # not just 1 or all" - N=3 needs 3 processors, but the test must still
        # be meaningful (N > 1) down to a 2-processor machine.
        $n = [Math]::Min(3, [Environment]::ProcessorCount)
        $expectedMask = '0x' + (([uint64]1 -shl $n) - [uint64]1).ToString('X')
        $out = New-TempFile
        $probe = "Set-Content -Path '$out' -Value ('0x' + (Get-Process -Id `$PID).ProcessorAffinity.ToString('X'))"
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        & (Join-Path $bin 'capt.bat') $n powershell -NoProfile -File $probeFile
        (Get-Content $out).Trim() | Should Be $expectedMask
        Remove-Item $out, $probeFile -ErrorAction SilentlyContinue
    }

    It 'pins the whole spawned process tree, not just the immediate child' {
        $n = [Math]::Min(3, [Environment]::ProcessorCount)
        $expectedMask = '0x' + (([uint64]1 -shl $n) - [uint64]1).ToString('X')
        $out = New-TempFile
        $script = @'
$c = Start-Process cmd -ArgumentList "/c ping -n 3 127.0.0.1 >nul" -WindowStyle Hidden -PassThru
try {{
    Start-Sleep -Milliseconds 500
    $c.Refresh()
    Set-Content -Path '{0}' -Value ('0x' + $c.ProcessorAffinity.ToString('X'))
    # ping -n 3 takes ~2s: 15s is a generous upper bound, so a wedged probe can't
    # hang the suite indefinitely (a bare WaitForExit() here once blocked 206s).
    if (-not $c.WaitForExit(15000)) {{ $c.Kill() }}
}} finally {{
    if (-not $c.HasExited) {{ $c.Kill() }}
}}
'@ -f $out
        $scriptFile = New-TempScript
        Set-Content -Path $scriptFile -Value $script
        & (Join-Path $bin 'capt.bat') $n powershell -NoProfile -File $scriptFile
        (Get-Content $out).Trim() | Should Be $expectedMask
        Remove-Item $out, $scriptFile -ErrorAction SilentlyContinue
    }
}

Describe 'capm.ps1 argument validation' {
    It 'rejects a non-numeric size' {
        & (Join-Path $bin 'capm.bat') abc cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a zero size' {
        & (Join-Path $bin 'capm.bat') 0 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a bare percent above 100' {
        & (Join-Path $bin 'capm.bat') 101 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a bare non-integer percent' {
        & (Join-Path $bin 'capm.bat') 50.5 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a "%"-suffixed size (no longer supported - use a bare integer instead, see README)' {
        & (Join-Path $bin 'capm.bat') 50% cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a huge 400-digit <size> (unit "<Unit>") cleanly - no raw PowerShell conversion error leaked (regression: release review 1609-8824cf7)' -TestCases @(
        @{ Unit = 'm' }
        @{ Unit = 'g' }
        @{ Unit = '' }
    ) {
        param($Unit)
        # A raw [double] cast on a digit string this long throws PowerShell's own
        # "Cannot convert value ... InvalidCastFromStringToDoubleOrSingle" error,
        # including the script's own path/line number, before the usage message -
        # TryParse must fail cleanly instead, with only the controlled usage error.
        $digits = '9' * 400
        $out = & (Join-Path $bin 'capm.bat') "$digits$Unit" cmd /c "echo hi" 2>&1
        $LASTEXITCODE | Should Be 1
        $joined = $out -join "`n"
        $joined | Should Not Match 'Cannot convert value'
        $joined | Should Not Match 'InvalidCastFromStringToDoubleOrSingle'
        $joined | Should Match 'usage: capm'
    }

    It 'rejects a missing command' {
        & (Join-Path $bin 'capm.bat') 100m 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'accepts each supported size suffix (<Size>)' -TestCases @(
        @{ Size = '1' }
        @{ Size = '50' }
        @{ Size = '100' }
        @{ Size = '100m' }
        @{ Size = '100M' }
        @{ Size = '1g' }
        @{ Size = '1G' }
        @{ Size = '0.5g' }
    ) {
        param($Size)
        & (Join-Path $bin 'capm.bat') $Size cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
    }
}

Describe 'capm.ps1 behavior' {
    It 'propagates the exit code of the wrapped command' {
        & (Join-Path $bin 'capm.bat') 100m cmd /c "exit 3"
        $LASTEXITCODE | Should Be 3
    }

    It 'forwards a flag that would ambiguously prefix-match a declared -Size-shaped parameter name (e.g. -s)' {
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'capm.bat') -Prefix @('100m') -ProbeArgs @('-s', '0')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '-s|SEP|0'
    }

    It 'preserves cmd.exe metacharacters and a literal "%" on the direct-launch path' {
        # Must invoke capm.ps1 directly (not through capm.bat, which has its own
        # separate, documented "%" corruption at the %* forwarding step for
        # arguments *after* the size token). This "%" belongs to a wrapped-command
        # argument, not the size - capm's own <size> never accepts "%" (see the
        # "rejects a %-suffixed size" test above).
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'capm.ps1') -Prefix @('100m') -ProbeArgs @('A&B', 'A|B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|A|B|SEP|100%OFF'
    }

    It 'does not falsely reject a large legitimate size (regression: [UIntPtr]::MaxValue is unavailable on .NET Framework)' {
        # Windows PowerShell 5.1 runs on .NET Framework, where [UIntPtr] has no
        # MaxValue member - "[UIntPtr]::MaxValue" silently evaluates to $null there
        # instead of throwing, which previously made the addressable-limit guard
        # compare every non-zero byte count against 0 and reject all of them. 90
        # (percent) reproduces the exact input class that first caught this (a real
        # npm-test run failed capm's 90%-cap shim test - now bare 90 - with a bogus
        # "exceeds the addressable limit ... = bytes here" error, the blank value
        # was the giveaway).
        $out = & (Join-Path $bin 'capm.bat') 90 cmd /c "exit 0" 2>&1
        $LASTEXITCODE | Should Be 0
        ($out -join "`n") | Should Not Match 'exceeds the addressable limit'
    }

    # Allocation probe: tries to commit a big-ish byte array and reports pass/fail
    # instead of throwing all the way out, so the test can tell "ran and refused
    # to allocate" apart from "crashed before even getting there" (see below).
    # __SIZE_MB__ substituted via -replace, not the "-f" format operator - the
    # probe's own try/catch braces are literal text that "-f" would misparse as
    # unescaped format-string braces ("Input string was not in a correct format").
    $allocProbeTemplate = @'
try {
    $arr = New-Object byte[] (__SIZE_MB__*1MB)
    [System.GC]::KeepAlive($arr)
    Write-Output 'ALLOCATED'
} catch {
    Write-Output ('FAILED: ' + $_.Exception.GetType().Name)
}
'@

    It 'enforces a hard ceiling: a generous cap allows a 200MB allocation, a tight one refuses it' {
        # Same 200MB allocation on both branches - only the cap value differs, so
        # a pass/fail flip is attributable to the cap, not to allocation size (an
        # earlier version of this test asked for 2000MB on the tight branch and
        # 200MB on the roomy one, which could pass on a units bug or an unrelated
        # near-2GB .NET allocation failure instead of proving the cap enforced).
        # 100m is comfortably above Windows PowerShell 5.1's own startup footprint
        # (confirmed empirically: 30MB crashes PowerShell itself with
        # StackOverflowException before the probe script even runs; 100-150MB lets
        # PowerShell start normally and the allocation attempt fail cleanly and
        # catchably instead) but far short of 200MB, so this doesn't depend on
        # exactly where that footprint sits on a given machine.
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value ($allocProbeTemplate -replace '__SIZE_MB__', '200')

        $tight = & powershell -NoProfile -File (Join-Path $bin 'capm.ps1') 100m powershell -NoProfile -File $probeFile
        $tightExit = $LASTEXITCODE
        $tightExit | Should Be 0
        ($tight | Select-Object -Last 1) | Should Match '^FAILED:'

        # 100 (percent): effectively "whole machine's RAM", generous by
        # construction - exercises the percent-of-total-physical-RAM code path
        # (GlobalMemoryStatusEx) for the "should succeed" side of the same probe.
        # Also catches a wrong conversion factor: if percent conversion divided
        # by the wrong constant, 200MB would exceed the (miscalculated) cap on
        # any real machine and the allocation would fail instead.
        $probeFile2 = New-TempScript
        Set-Content -Path $probeFile2 -Value ($allocProbeTemplate -replace '__SIZE_MB__', '200')
        $roomy = & powershell -NoProfile -File (Join-Path $bin 'capm.ps1') 100 powershell -NoProfile -File $probeFile2
        $roomyExit = $LASTEXITCODE
        $roomyExit | Should Be 0
        ($roomy | Select-Object -Last 1) | Should Be 'ALLOCATED'

        Remove-Item $probeFile, $probeFile2 -ErrorAction SilentlyContinue
    }

    It 'caps the whole spawned process tree, not just the immediate child' {
        # A grandchild (spawned by the immediate child, not by capm.ps1 itself)
        # must still be subject to the same Job Object memory ceiling - Windows
        # auto-joins new child processes to the parent's job by default, same
        # inheritance capc/capt already rely on for their own "whole tree" tests.
        # Both the outer (child) and inner (grandchild) scripts are separate temp
        # FILES, not inline -Command strings - nested inline quoting across three
        # process hops (Pester -> child -> grandchild) is exactly the kind of thing
        # that silently mis-quotes; every other test in this file that needs a
        # nested script uses a file for the same reason.
        #
        # 400m, not 100m: the cap here is the JOB's aggregate, shared by BOTH the
        # outer wrapper AND the grandchild - two separate PowerShell/CLR runtime
        # instances, each needing their own startup footprint, drawing from the
        # SAME budget (confirmed empirically: at 100m here, the grandchild crashed
        # with StackOverflowException before it could even run the probe, because
        # the outer wrapper's own startup had already used up most of the shared
        # 100m). 400m leaves comfortable room for two runtimes to start while
        # staying far short of the grandchild's 2000MB allocation attempt.
        $out = New-TempFile
        $grandchildProbe = $allocProbeTemplate -replace '__SIZE_MB__', '2000'
        $grandchildFile = New-TempScript
        Set-Content -Path $grandchildFile -Value $grandchildProbe

        $outerScript = @"
`$c = Start-Process powershell -ArgumentList @('-NoProfile', '-File', '$grandchildFile') -WindowStyle Hidden -PassThru -RedirectStandardOutput '$out'
if (-not `$c.WaitForExit(15000)) { `$c.Kill() }
"@
        $outerFile = New-TempScript
        Set-Content -Path $outerFile -Value $outerScript

        & (Join-Path $bin 'capm.bat') 400m powershell -NoProfile -File $outerFile
        (Get-Content $out -ErrorAction SilentlyContinue | Select-Object -Last 1) | Should Match '^FAILED:'
        Remove-Item $out, $grandchildFile, $outerFile -ErrorAction SilentlyContinue
    }
}

# Real usage stacks these tools, e.g. "capm 10g capc 50 idle <command>" - each
# wrapper's own direct-launch attempt fails for a bare tool name (no capc.exe/
# idle.exe exists, only .bat/.ps1/extensionless shims), so it falls back to
# cmd.exe, which resolves the bare name via PATHEXT (.BAT is in the default
# PATHEXT list). That only works when the tools' directory is actually on
# PATH, so every test below runs under a PATH temporarily REPLACED (not
# prepended, so nothing on the real developer PATH can leak in) to contain
# only $bin plus the minimum Windows needs to run cmd.exe/powershell.exe.
$chainPath = "$bin;$env:SystemRoot\System32;$env:SystemRoot\System32\WindowsPowerShell\v1.0"

Describe 'chained tool invocation (bare tool names resolved via PATH, cmd.exe PATHEXT fallback)' {
    It 'propagates the exit code through a 2-level chain (capc wrapping idle wrapping a probe)' {
        $prevPath = $env:PATH
        $env:PATH = $chainPath
        try {
            & powershell -NoProfile -File (Join-Path $bin 'capc.ps1') 50 idle cmd.exe /c exit 8
            $LASTEXITCODE | Should Be 8
        } finally {
            $env:PATH = $prevPath
        }
    }

    It 'propagates the exit code through a 3-level chain (capm wrapping capc wrapping idle wrapping a probe) and applies Idle priority to the innermost process' {
        # capm's own cap is deliberately roomy (100, i.e. 100%) here - this test is
        # about the chain mechanics (bare-name resolution through two extra cmd.exe
        # hops) and the priority class reaching the innermost process, not about
        # capm's own enforcement (covered separately, and separately again below
        # with a tight cap in a 2-level chain, where extra nested-CLR startup cost
        # is smaller).
        $prevPath = $env:PATH
        $env:PATH = $chainPath
        try {
            $out = New-TempFile
            $probe = "(Get-Process -Id `$PID).PriorityClass | Out-File -FilePath '$out'; exit 8"
            $probeFile = New-TempScript
            Set-Content -Path $probeFile -Value $probe
            & powershell -NoProfile -File (Join-Path $bin 'capm.ps1') 100 capc 50 idle powershell -NoProfile -File $probeFile
            $LASTEXITCODE | Should Be 8
            (Get-Content $out).Trim() | Should Be 'Idle'
            Remove-Item $out, $probeFile -ErrorAction SilentlyContinue
        } finally {
            $env:PATH = $prevPath
        }
    }

    It 'propagates the exit code through a 2-level chain with capc OUTSIDE capm (regression: capm used to reject a "%"-style percent here)' {
        # The exact broken repro from the 1500-1e191bc release review: "capc 50
        # capm 25% ..." failed the fail-closed "%" check (capm's own size, "25%",
        # is an argument in CAPC's cmd.exe fallback command line - any "%" there
        # trips the same guard that protects the wrapped command's own arguments),
        # while the reverse order or a non-percent size worked fine - an
        # order-dependent foot-gun. Removing "%" from capm's size grammar (bare
        # integer = percent now, same as capc) fixes this for every order, since a
        # bare integer never contains "%" in the first place.
        $prevPath = $env:PATH
        $env:PATH = $chainPath
        try {
            & powershell -NoProfile -File (Join-Path $bin 'capc.ps1') 50 capm 50 cmd.exe /c exit 8
            $LASTEXITCODE | Should Be 8
        } finally {
            $env:PATH = $prevPath
        }
    }

    It 'enforces the outer capm memory cap on a process launched through an extra bare-name/cmd.exe hop, nested inside capc''s own Job Object' {
        # Reuses the already-validated 400m/2000MB pairing from the "whole spawned
        # process tree" test above (two nested PowerShell/CLR instances sharing one
        # job-wide memory budget - an earlier version of this test asked for only
        # 200MB, which left enough headroom after both instances started that the
        # allocation actually succeeded instead of proving anything). Here the
        # second instance is capc.ps1's own host, reached via capm's cmd.exe/PATHEXT
        # fallback (bare "capc" has no direct .exe), and capc assigns the final probe
        # to its OWN separate Job Object (CPU 50%) nested inside capm's (Windows 8+
        # nested jobs) - proving the outer memory ceiling still binds through both
        # the extra hop and the nested job, not just on a direct, single-level child.
        #
        # Roomy-cap control branch (release review 1500-1e191bc P2): without it, a
        # 2000MB allocation failing under a tight cap doesn't prove the cap did
        # anything - it could fail on unrelated address-space/CLR limits regardless
        # of capm, and this test would stay green either way. Same oracle as the
        # main capm enforcement test above: the SAME allocation must succeed under
        # a generous cap through the identical chain shape.
        $localAllocProbe = @'
try {
    $arr = New-Object byte[] (2000*1MB)
    [System.GC]::KeepAlive($arr)
    Write-Output 'ALLOCATED'
} catch {
    Write-Output ('FAILED: ' + $_.Exception.GetType().Name)
}
'@
        $prevPath = $env:PATH
        $env:PATH = $chainPath
        try {
            $probeFile = New-TempScript
            Set-Content -Path $probeFile -Value $localAllocProbe
            $tight = & powershell -NoProfile -File (Join-Path $bin 'capm.ps1') 400m capc 50 powershell -NoProfile -File $probeFile
            $LASTEXITCODE | Should Be 0
            ($tight | Select-Object -Last 1) | Should Match '^FAILED:'
            Remove-Item $probeFile -ErrorAction SilentlyContinue

            $probeFile2 = New-TempScript
            Set-Content -Path $probeFile2 -Value $localAllocProbe
            $roomy = & powershell -NoProfile -File (Join-Path $bin 'capm.ps1') 100 capc 50 powershell -NoProfile -File $probeFile2
            $LASTEXITCODE | Should Be 0
            ($roomy | Select-Object -Last 1) | Should Be 'ALLOCATED'
            Remove-Item $probeFile2 -ErrorAction SilentlyContinue
        } finally {
            $env:PATH = $prevPath
        }
    }

    It 'holds CPU usage of a nested "capc 50 capc 50" measurably below a single "capc 50" (CPU rate multiplies when nested, not "smaller wins")' {
        # Documents/locks in the release review's P1 finding: nested Job Object
        # CPU rate is relative to the parent's, so equal caps multiply rather than
        # take the minimum - "capc 50 capc 50" should land near 25%, clearly below a
        # single "capc 50" (~50%), not equal to it (which "smaller wins" predicts).
        $burn = @'
param([int]$Threads, [int]$Seconds)
$proc = [Diagnostics.Process]::GetCurrentProcess()
$cpuStart = $proc.TotalProcessorTime
$wallStart = Get-Date
$pool = [runspacefactory]::CreateRunspacePool(1, $Threads)
$pool.Open()
$tasks = 0..($Threads - 1) | ForEach-Object {
    $ps = [powershell]::Create()
    $ps.RunspacePool = $pool
    [void]$ps.AddScript({ param($sec) $sw = [Diagnostics.Stopwatch]::StartNew(); $x = 0; while ($sw.Elapsed.TotalSeconds -lt $sec) { $x = $x + 1 } }).AddArgument($Seconds)
    [PSCustomObject]@{ Pipe = $ps; Handle = $ps.BeginInvoke() }
}
foreach ($t in $tasks) { $t.Pipe.EndInvoke($t.Handle) | Out-Null; $t.Pipe.Dispose() }
$pool.Close()
$proc.Refresh()
$cpuSeconds = ($proc.TotalProcessorTime - $cpuStart).TotalSeconds
$wallSeconds = ((Get-Date) - $wallStart).TotalSeconds
$pct = ($cpuSeconds / ($wallSeconds * [Environment]::ProcessorCount)) * 100
Write-Output ("{0:N1}" -f $pct)
'@
        $burnFile = New-TempScript
        Set-Content -Path $burnFile -Value $burn
        $threads = [Environment]::ProcessorCount
        $seconds = 4
        $cap = 50

        $prevPath = $env:PATH
        $env:PATH = $chainPath
        try {
            # Same noisy-machine retry strategy as the single-cap CPU test above:
            # relative thresholds (nested vs. single, not an absolute number).
            $passed = $false
            $lastSingle = $null
            $lastNested = $null
            for ($attempt = 1; $attempt -le 3 -and -not $passed; $attempt++) {
                $singleOut = & (Join-Path $bin 'capc.bat') $cap powershell -NoProfile -File $burnFile $threads $seconds
                $single = [double]($singleOut | Select-Object -Last 1)
                $nestedOut = & powershell -NoProfile -File (Join-Path $bin 'capc.ps1') $cap capc $cap powershell -NoProfile -File $burnFile $threads $seconds
                $nested = [double]($nestedOut | Select-Object -Last 1)
                $lastSingle = $single
                $lastNested = $nested

                # No "single must be near cap" gate here (unlike the uncapped-vs-cap
                # test above): the comparison below is relative (nested vs. single),
                # not absolute, so single landing anywhere comfortably above a
                # near-zero floor is fine - a real observed pair (single=34.5,
                # nested=23.3, cap=50) already satisfies the pass check below and
                # was wrongly discarded by an earlier "single >= cap*1.15" gate that
                # didn't apply to this comparison at all.
                if ($single -lt 5) { continue }
                if ($nested -lt ($single * 0.75) -and $nested -lt ($cap * 0.75)) { $passed = $true }
            }

            if (-not $passed) { Write-Host "last attempt: single=$lastSingle nested=$lastNested cap=$cap" }
            $passed | Should Be $true
        } finally {
            $env:PATH = $prevPath
        }
        Remove-Item $burnFile -ErrorAction SilentlyContinue
    }

    It 'clamps a nested "capt 1 capt 2" to the OUTER (tighter) mask, not the inner (wider) request' -Skip:([Environment]::ProcessorCount -lt 2) {
        # 1/2, not 2/3: capt itself rejects a thread-count above the machine's
        # own logical processor count, and the product doesn't document a
        # minimum-processor-count requirement - a hardcoded "capt 2 capt 3"
        # here would fail argument validation on a 1-2 processor machine
        # before ever reaching the nested-affinity behavior under test
        # (release review 1745-708cb53 P2). Skipped outright (not run with a
        # smaller/meaningless count) on a genuinely 1-processor machine, since
        # there's no way to express "wider than 1" below thread-count 2 there.
        #
        # Empirically verified (release review 1609-8824cf7 P3): a nested capt
        # requesting a WIDER mask than its parent job allows does not error out
        # and does not get the wider mask either - the effective affinity comes
        # back clamped to the outer, tighter mask. Matches Microsoft's "child can
        # be stricter, not less strict, than parent" model for nested Job Object
        # affinity (Nested Jobs - Job Limits). Manually confirmed with capt 2
        # capt 4 -> 0x3 (not 0xF) before writing this test.
        $prevPath = $env:PATH
        $env:PATH = $chainPath
        try {
            $out = New-TempFile
            $probe = "(Get-Process -Id `$PID).ProcessorAffinity.ToString('X') | Out-File -FilePath '$out'; exit 8"
            $probeFile = New-TempScript
            Set-Content -Path $probeFile -Value $probe
            & powershell -NoProfile -File (Join-Path $bin 'capt.ps1') 1 capt 2 powershell -NoProfile -File $probeFile
            $LASTEXITCODE | Should Be 8
            ('0x' + (Get-Content $out).Trim()) | Should Be '0x1'
            Remove-Item $out, $probeFile -ErrorAction SilentlyContinue
        } finally {
            $env:PATH = $prevPath
        }
    }
}

Describe 'admin.bat' {
    It 'fails with a usage message when no command is given' {
        & (Join-Path $bin 'admin.bat') 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'parses without syntax errors' {
        $parseErrors = $null
        [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $bin 'admin.ps1'), [ref]$null, [ref]$parseErrors) | Out-Null
        $parseErrors.Count | Should Be 0
    }

    It 'runs the wrapped command inline and propagates its exit code when already elevated' -Skip:(-not $script:isAdminRunner) {
        & (Join-Path $bin 'admin.bat') cmd /c "exit 5"
        $LASTEXITCODE | Should Be 5
    }

    It 'preserves cmd.exe metacharacters and "%" on the direct-launch path when already elevated' -Skip:(-not $script:isAdminRunner) {
        # Must drive admin.ps1 directly (Get-DirectForwardedArgs), not admin.bat
        # (Get-ForwardedArgs) - admin.bat's own %* forwarding corrupts a literal "%"
        # before admin.ps1 ever runs, same as every other tool's .bat wrapper (see
        # admin.bat's own comment). This test is about the .ps1's direct-launch path,
        # which only bare-name PowerShell invocation reaches.
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'admin.ps1') -ProbeArgs @('A&B', 'A|B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|A|B|SEP|100%OFF'
    }

    # admin.ps1's not-yet-elevated branch now has a real direct-launch path (no
    # cmd.exe hop) for a target that isn't .bat/.cmd - the "%" check is scoped to
    # ONLY the .bat/.cmd branch (matching AdminLauncher.Run's own fallback check),
    # since a direct-launch target never touches cmd.exe and so isn't at risk of
    # "%" expansion at all. Only the .bat/.cmd case is exercised here: proving the
    # direct-launch case now ALLOWS "%" through would require actually reaching
    # Start-Process -Verb RunAs, which pops a real interactive UAC prompt and would
    # hang an automated run - that side is intentionally left unverified by an
    # automated test (would need an elevated test runner and manual UAC approval).
    It 'refuses to run and never attempts elevation when an argument contains "%" for a .bat/.cmd target (not-yet-elevated branch)' -Skip:$script:isAdminRunner {
        # This check runs directly in PowerShell before Start-Process -Verb RunAs is
        # ever called (see admin.ps1), so no UAC consent prompt is at risk here - if
        # this ever hangs, the check moved past the RunAs call and needs investigating.
        $stderr = & powershell -NoProfile -File (Join-Path $bin 'admin.ps1') 'somebatch.bat' /c '100%OFF' 2>&1
        $exitCode = $LASTEXITCODE
        $exitCode | Should Be 1
        (($stderr | Out-String) -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
    }

    It 'does not throw a MethodInvocation error when a non-string argument reaches the "%" check (not-yet-elevated branch, .bat/.cmd target)' -Skip:$script:isAdminRunner {
        # Regression for the $a.Contains('%') -> "$a".Contains('%') fix: $a.Contains
        # used to throw "does not contain a method named 'Contains'" for any $args
        # element that isn't already a string (e.g. a bare integer). -File invocation
        # from an external process always stringifies argv, so the only way to get a
        # genuine non-string element into $args is a same-session "&" call with a
        # parenthesized expression - hence the driver script below. First argument is
        # a .bat target so the "%" check path is actually reached (it's now scoped to
        # .bat/.cmd targets only); the second (5, an [int]) never contains "%", so the
        # loop must move on to its third (string) argument, which does - proving the
        # int didn't throw along the way.
        $adminPs1 = Join-Path $bin 'admin.ps1'
        $driverScript = New-TempScript
        Set-Content -Path $driverScript -Value "& '$adminPs1' 'somebatch.bat' (5) '100%OFF'`r`nexit `$LASTEXITCODE`r`n"
        $stderr = & powershell -NoProfile -File $driverScript 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
        $exitCode | Should Be 1
        $stderr | Should Not Match 'does not contain a method'
        ($stderr -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
        Remove-Item $driverScript -ErrorAction SilentlyContinue
    }
}

# P1 regression (docs/reviews/2026-09-02-0650-0cb8a1b-code-review.md): the
# not-yet-elevated branch used to pick the cmd.exe fallback purely from a
# ".bat/.cmd suffix" regex on the first argument, so a cmd.exe BUILTIN target
# (ver, set, echo, ... - not files at all) went straight to
# Start-Process -Verb RunAs -FilePath and died there with "The system cannot find
# the file specified" before any UAC prompt, losing the CreateProcess-failed ->
# cmd.exe-fallback semantic every other launcher has. The route now comes from
# admin.ps1's Get-AdminLaunchRoute: Application-resolvable target -> direct
# -FilePath launch; .bat/.cmd or unresolvable (builtins) -> cmd.exe fallback.
# Two layers of coverage, both UAC-safe (no test here ever reaches -Verb RunAs):
# - the routing function itself, AST-extracted from admin.ps1 and evaluated here,
#   so the decision table is asserted without launching anything. Builtin targets
#   are 'ver'/'set' only: other builtin names are PATH-dependent (Git for Windows
#   ships a real dir.exe, so 'dir' legitimately resolves Direct on such machines -
#   matching the elevated branch's own CreateProcess lookup).
# - the fail-closed "%" check: it now guards the whole fallback route (builtins
#   included) and runs BEFORE Start-Process -Verb RunAs, so a builtin target with a
#   "%" argument must end in the refusal message - not the old pre-UAC "cannot find
#   the file specified" - which discriminates the route taken with no elevation
#   ever attempted.
$adminPs1ForRouting = Join-Path $bin 'admin.ps1'

Describe 'admin.ps1 launch routing (not-yet-elevated branch)' {
    $parseErrors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($adminPs1ForRouting, [ref]$null, [ref]$parseErrors)
    $fnAst = $ast.Find({ param($a) $a -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $a.Name -eq 'Get-AdminLaunchRoute' }, $true)
    if ($fnAst) { Invoke-Expression $fnAst.Extent.Text }

    It 'exposes the routing decision as a testable function (parsed from admin.ps1)' {
        $parseErrors.Count | Should Be 0
        $fnAst | Should Not Be $null
        Get-Command Get-AdminLaunchRoute -ErrorAction SilentlyContinue | Should Not Be $null
    }

    It 'routes cmd.exe builtins (no file at all) to the cmd.exe fallback (<Target>)' -TestCases @(
        @{ Target = 'ver' }
        @{ Target = 'set' }
    ) {
        param($Target)
        Get-AdminLaunchRoute -Target $Target | Should Be 'CmdFallback'
    }

    It 'routes a resolvable Application target to the direct launch (<Target>)' -TestCases @(
        @{ Target = 'cmd' }
        @{ Target = $env:ComSpec }
    ) {
        param($Target)
        Get-AdminLaunchRoute -Target $Target | Should Be 'Direct'
    }

    It 'routes a .bat target to the cmd.exe fallback even though Get-Command resolves it' {
        $bat = Join-Path $script:testRoot 'routing-probe.bat'
        Set-Content -Path $bat -Value "@echo off`r`nexit /b 0`r`n"
        Get-AdminLaunchRoute -Target $bat | Should Be 'CmdFallback'
    }

    It 'routes an unresolvable bare word to the cmd.exe fallback' {
        Get-AdminLaunchRoute -Target ('no-such-tool-' + [guid]::NewGuid().ToString('N')) | Should Be 'CmdFallback'
    }

    It 'attempts the cmd.exe fallback route (not a pre-UAC file-not-found failure) for a builtin target with a "%" argument' -Skip:$script:isAdminRunner {
        # Skipped on elevated runners: there the same refusal comes from
        # AdminLauncher.Run's own fallback check instead, so the not-yet-elevated
        # routing under test wouldn't be exercised (same message, wrong branch).
        foreach ($target in @('ver', 'set')) {
            $stderr = & powershell -NoProfile -File $adminPs1ForRouting $target '100%OFF' 2>&1
            $exitCode = $LASTEXITCODE
            $exitCode | Should Be 1
            (($stderr | Out-String) -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
            (($stderr | Out-String) -replace '\s+', ' ') | Should Not Match 'cannot\s+find\s+the\s+file'
        }
    }
}

# cy.ps1/cx.ps1 wrap "claude"/"codex" directly - these tests must never invoke the
# real binaries (that would actually run an AI agent with permission checks
# bypassed). A fake same-named .bat stand-in is prepended to PATH instead, since a
# real npm-installed claude/codex is itself typically a .cmd shim on Windows - this
# also exercises the .bat/.cmd fallback path, not just a direct .exe.
function Test-FakeLauncher {
    param(
        [Parameter(Mandatory = $true)][string]$Ps1,
        [Parameter(Mandatory = $true)][string]$FakeTargetName,
        [Parameter(Mandatory = $true)][string[]]$ExtraArgs
    )
    # Deliberate space in the dir name: cy.ps1/cx.ps1's target ("claude"/"codex")
    # is a fixed bare word, never quoted on the cmd.exe command line itself, so this
    # can't reproduce the P1 quoted-target-path bug the way a caller-supplied path
    # can (see the spaced-target tests below) - but a space here still exercises
    # cmd.exe's own PATHEXT resolution finding a PATH entry with a space in it, a
    # common real-world case (e.g. "C:\Users\John Smith\AppData\Roaming\npm").
    $fakeDir = Join-Path $script:testRoot ("win-nice-fakebin-spaced " + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $fakeDir | Out-Null
    $out = Join-Path $fakeDir 'out.txt'
    $fakeTarget = Join-Path $fakeDir $FakeTargetName
    Set-Content -Path $fakeTarget -Value "@echo off`r`n(echo %*)>`"$out`"`r`n"
    $prevPath = $env:PATH
    # REPLACE, not prepend: a real claude.exe/codex.exe elsewhere on the
    # developer's PATH must never be reachable from this test, regardless of
    # PATH order or cmd.exe's current-directory-first search quirk.
    $env:PATH = "$fakeDir;$env:SystemRoot\System32;$env:SystemRoot\System32\WindowsPowerShell\v1.0"
    try {
        $stderr = & powershell -NoProfile -File $Ps1 @ExtraArgs 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
    } finally {
        $env:PATH = $prevPath
    }
    $result = if (Test-Path $out) { (Get-Content $out).Trim() } else { $null }
    Remove-Item $fakeDir -Recurse -ErrorAction SilentlyContinue
    return [PSCustomObject]@{ Output = $result; ExitCode = $exitCode; StdErr = $stderr }
}

Describe 'cy.ps1' {
    It 'prepends --dangerously-skip-permissions and forwards the rest, protecting metacharacters' {
        $r = Test-FakeLauncher -Ps1 (Join-Path $bin 'cy.ps1') -FakeTargetName 'claude.bat' -ExtraArgs @('-p', 'A&B')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '--dangerously-skip-permissions -p "A&B"'
    }

    It 'refuses to run and never launches the target when an argument contains "%" (cmd.exe fallback path)' {
        # Fake claude.bat forces the .bat/.cmd fallback branch in cy.ps1's embedded
        # C# Run(), the same code path covered by the table-driven test above.
        $r = Test-FakeLauncher -Ps1 (Join-Path $bin 'cy.ps1') -FakeTargetName 'claude.bat' -ExtraArgs @('100%OFF')
        $r.ExitCode | Should Be 1
        ($r.StdErr -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
        $r.Output | Should Be $null
    }
}

Describe 'cx.ps1' {
    It 'prepends --dangerously-bypass-approvals-and-sandbox and forwards the rest, protecting metacharacters' {
        $r = Test-FakeLauncher -Ps1 (Join-Path $bin 'cx.ps1') -FakeTargetName 'codex.bat' -ExtraArgs @('-p', 'A&B')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '--dangerously-bypass-approvals-and-sandbox -p "A&B"'
    }

    It 'refuses to run and never launches the target when an argument contains "%" (cmd.exe fallback path)' {
        $r = Test-FakeLauncher -Ps1 (Join-Path $bin 'cx.ps1') -FakeTargetName 'codex.bat' -ExtraArgs @('100%OFF')
        $r.ExitCode | Should Be 1
        ($r.StdErr -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
        $r.Output | Should Be $null
    }
}

# Regression: Test-FakeLauncher used to PREPEND $fakeDir to $env:PATH, so a real
# claude.exe/codex.exe elsewhere on the developer's PATH stayed reachable as a
# fallback (PATH order isn't the only lookup rule - e.g. cmd.exe checks the
# current directory before PATH). These tests run with NO fake target present at
# all, on the same replaced-not-prepended PATH cy.ps1/cx.ps1's own tests now use -
# if isolation ever regressed back to a prepend and a real claude/codex leaked
# through, the call would hang waiting on stdin instead of failing fast, so the
# check runs in a job with a timeout rather than a plain synchronous call.
$isolationTools = @(
    @{ Name = 'cy'; Ps1 = 'cy.ps1' }
    @{ Name = 'cx'; Ps1 = 'cx.ps1' }
)

Describe 'cy.ps1 / cx.ps1 PATH isolation' {
    It 'fails fast (not silently, not hung) when no fake target is on the isolated PATH (<Name>)' -TestCases $isolationTools {
        param($Name, $Ps1)
        $fakeDir = Join-Path $script:testRoot ("win-nice-fakebin-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $fakeDir | Out-Null
        $isolatedPath = "$fakeDir;$env:SystemRoot\System32;$env:SystemRoot\System32\WindowsPowerShell\v1.0"
        $ps1Path = Join-Path $bin $Ps1
        $job = Start-Job -ScriptBlock {
            param($Ps1Path, $Path)
            $env:PATH = $Path
            $out = & powershell -NoProfile -File $Ps1Path 2>&1 | Out-String
            [PSCustomObject]@{ Out = $out; ExitCode = $LASTEXITCODE }
        } -ArgumentList $ps1Path, $isolatedPath
        $done = Wait-Job $job -Timeout 20
        if (-not $done) {
            Stop-Job $job
            Remove-Job $job -Force
            Remove-Item $fakeDir -Recurse -ErrorAction SilentlyContinue
            throw "$Name.ps1 did not exit within 20s under an isolated PATH with no fake target present - possible fallback to a real binary hanging on stdin."
        }
        $result = Receive-Job $job
        Remove-Job $job -Force
        $result.ExitCode | Should Not Be 0
        $result.Out | Should Match '(is not recognized|cannot find|CreateProcess failed)'
        Remove-Item $fakeDir -Recurse -ErrorAction SilentlyContinue
    }
}

# ---------------------------------------------------------------------------
# Fault-injection harness. bin/ is NOT modified by any of this.
#
# ResumeThread / WaitForSingleObject / WaitForMultipleObjects / the timer
# APIs / GetExitCodeProcess / AssignProcessToJobObject only ever fail on handles the launcher itself just
# created, so no command line can reach those branches - the integration tests
# above can only ever exercise the success paths. Instead of adding a toggle to
# production code, these tests take the SAME embedded C# out of the .ps1 (via
# the AST, like the admin routing tests above), swap individual [DllImport]
# declarations for instrumented managed stubs that forward to the real entry
# point (or force a documented failure on demand), compile that copy into its
# own namespace, and call Run() directly.
function Get-LauncherCSharp {
    param([Parameter(Mandatory = $true)][string]$Ps1Path)
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Ps1Path, [ref]$null, [ref]$null)
    $node = $ast.Find({
        param($a)
        $a -is [System.Management.Automation.Language.StringConstantExpressionAst] -and
        $a.StringConstantType -eq 'DoubleQuotedHereString' -and
        $a.Value -match 'public static class \w+Launcher'
    }, $true)
    if (-not $node) { throw "no embedded C# here-string found in $Ps1Path" }
    # LF-normalized so the anchors below don't have to care about CRLF.
    return ($node.Value -replace "`r`n", "`n")
}

# Anchored single-occurrence replacement. Throws when the anchor is missing OR
# ambiguous, so a launcher edit that moves it fails the test loudly instead of
# quietly producing an uninstrumented (always-green) probe.
function Edit-SourceOnce {
    param(
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][string]$Find,
        [Parameter(Mandatory = $true)][string]$Replace,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $needle = $Find -replace "`r`n", "`n"
    $i = $Text.IndexOf($needle, [StringComparison]::Ordinal)
    if ($i -lt 0) { throw "fault-probe anchor '$Label' not found - launcher source changed shape" }
    if ($Text.IndexOf($needle, $i + 1, [StringComparison]::Ordinal) -ge 0) { throw "fault-probe anchor '$Label' is not unique" }
    return $Text.Substring(0, $i) + ($Replace -replace "`r`n", "`n") + $Text.Substring($i + $needle.Length)
}

# Builds (once) an instrumented copy of a launcher's class and returns its Type.
# -JobObject additionally instruments the three Job-Object-only imports.
function New-LauncherFaultProbe {
    param(
        [Parameter(Mandatory = $true)][string]$Ps1Path,
        [Parameter(Mandatory = $true)][string]$Namespace,
        [Parameter(Mandatory = $true)][string]$ClassName,
        [switch]$JobObject
    )
    $already = [System.Management.Automation.PSTypeName]"$Namespace.$ClassName"
    if ($already.Type) { return $already.Type }

    $src = Get-LauncherCSharp -Ps1Path $Ps1Path

    # CloseHandle: record every close, forward to the real one, count failures.
    # A double close would make the second CloseHandle return false, so
    # "CloseHandleFailures -eq 0 and every logged handle distinct" IS the
    # closed-exactly-once oracle. All shared probe state lives here.
    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr hObject);
'@ @'
    [DllImport("kernel32.dll", EntryPoint = "CloseHandle")]
    static extern bool CloseHandleReal(IntPtr hObject);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern void SetLastError(uint dwErrCode);

    public static bool FailWait;
    public static bool FailResume;
    public static bool FailAssign;
    public static bool FailSetInfo;
    public static bool FailGetExitCode;
    public static bool FailTerminate;
    public static int TerminateCalls;
    public static int CloseHandleFailures;
    public static int LastProcessId;
    public static int SetInfoCalls;
    public static int SetInfoFailOnCall;
    public static bool FailCreateTimer;
    public static bool FailSetTimer;
    public static bool ForceBothSignaledThenReturnProcess;
    public static System.Collections.Generic.List<IntPtr> ClosedHandles = new System.Collections.Generic.List<IntPtr>();

    public static void ResetProbe()
    {
        FailWait = false; FailResume = false; FailAssign = false;
        FailSetInfo = false; FailGetExitCode = false; FailTerminate = false;
        TerminateCalls = 0; CloseHandleFailures = 0; LastProcessId = 0;
        SetInfoCalls = 0; SetInfoFailOnCall = 0;
        FailCreateTimer = false; FailSetTimer = false; ForceBothSignaledThenReturnProcess = false;
        ClosedHandles.Clear();
    }

    static bool CloseHandle(IntPtr hObject)
    {
        ClosedHandles.Add(hObject);
        bool ok = CloseHandleReal(hObject);
        if (!ok) CloseHandleFailures++;
        return ok;
    }
'@ 'CloseHandle'

    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "TerminateProcess")]
    static extern bool TerminateProcessReal(IntPtr hProcess, uint uExitCode);

    static bool TerminateProcess(IntPtr hProcess, uint uExitCode)
    {
        TerminateCalls++;
        if (FailTerminate) { SetLastError(5); return false; }
        return TerminateProcessReal(hProcess, uExitCode);
    }
'@ 'TerminateProcess'

    # ERROR_INVALID_HANDLE (6) via a real SetLastError P/Invoke, so
    # Marshal.GetLastWin32Error() returns a deterministic value - asserting on
    # it proves the launcher captures the error BEFORE calling TerminateProcess.
    # Not every launcher declares WaitForSingleObject anymore: since the
    # round-17 waitable-timer fix, caps waits via WaitForMultipleObjects on
    # {process, timer} and has no WaitForSingleObject at all. Instrument
    # whichever wait function the launcher actually declares, and give FailWait
    # the same uniform meaning on both ("make the deadline wait fail"), so the
    # table-driven tests below only need the function name for their expected
    # message (WaitFn in $launcherExecCases).
    $waitSingleAnchor = @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);
'@
    if ($src.Contains(($waitSingleAnchor -replace "`r`n", "`n"))) {
        $src = Edit-SourceOnce $src $waitSingleAnchor @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "WaitForSingleObject")]
    static extern uint WaitForSingleObjectReal(IntPtr hHandle, uint dwMilliseconds);

    static uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds)
    {
        if (FailWait) { SetLastError(6); return 0xFFFFFFFF; }
        return WaitForSingleObjectReal(hHandle, dwMilliseconds);
    }
'@ 'WaitForSingleObject'
    }

    $waitMultiAnchor = @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForMultipleObjects(uint nCount, IntPtr[] lpHandles, bool bWaitAll, uint dwMilliseconds);
'@
    if ($src.Contains(($waitMultiAnchor -replace "`r`n", "`n"))) {
        $src = Edit-SourceOnce $src $waitMultiAnchor @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "WaitForMultipleObjects")]
    static extern uint WaitForMultipleObjectsReal(uint nCount, IntPtr[] lpHandles, bool bWaitAll, uint dwMilliseconds);

    static uint WaitForMultipleObjects(uint nCount, IntPtr[] lpHandles, bool bWaitAll, uint dwMilliseconds)
    {
        if (FailWait) { SetLastError(6); return 0xFFFFFFFF; }
        if (ForceBothSignaledThenReturnProcess)
        {
            // Tie-break regression driver: block until BOTH handles are
            // really signaled (child exited AND deadline timer due), then
            // report ONLY the process index - the exact lowest-index state
            // production must disambiguate with the process's real exit time.
            WaitForMultipleObjectsReal(nCount, lpHandles, true, dwMilliseconds);
            return 0;
        }
        return WaitForMultipleObjectsReal(nCount, lpHandles, bWaitAll, dwMilliseconds);
    }
'@ 'WaitForMultipleObjects'
    }

    # caps-only timer instrumentation (the timer declarations exist nowhere
    # else). Same conditional-anchor pattern as the wait functions above.
    $createTimerAnchor = @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr CreateWaitableTimer(IntPtr lpTimerAttributes, bool bManualReset, string lpTimerName);
'@
    if ($src.Contains(($createTimerAnchor -replace "`r`n", "`n"))) {
        $src = Edit-SourceOnce $src $createTimerAnchor @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "CreateWaitableTimer")]
    static extern IntPtr CreateWaitableTimerReal(IntPtr lpTimerAttributes, bool bManualReset, string lpTimerName);

    static IntPtr CreateWaitableTimer(IntPtr lpTimerAttributes, bool bManualReset, string lpTimerName)
    {
        if (FailCreateTimer) { SetLastError(5); return IntPtr.Zero; }
        return CreateWaitableTimerReal(lpTimerAttributes, bManualReset, lpTimerName);
    }
'@ 'CreateWaitableTimer'
    }

    $setTimerAnchor = @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetWaitableTimer(IntPtr hTimer, ref long pDueTime, int lPeriod,
        IntPtr pfnCompletionRoutine, IntPtr lpArgToCompletionRoutine, bool fResume);
'@
    if ($src.Contains(($setTimerAnchor -replace "`r`n", "`n"))) {
        $src = Edit-SourceOnce $src $setTimerAnchor @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "SetWaitableTimer")]
    static extern bool SetWaitableTimerReal(IntPtr hTimer, ref long pDueTime, int lPeriod,
        IntPtr pfnCompletionRoutine, IntPtr lpArgToCompletionRoutine, bool fResume);

    static bool SetWaitableTimer(IntPtr hTimer, ref long pDueTime, int lPeriod,
        IntPtr pfnCompletionRoutine, IntPtr lpArgToCompletionRoutine, bool fResume)
    {
        if (FailSetTimer) { SetLastError(5); return false; }
        return SetWaitableTimerReal(hTimer, ref pDueTime, lPeriod, pfnCompletionRoutine, lpArgToCompletionRoutine, fResume);
    }
'@ 'SetWaitableTimer'
    }

    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "GetExitCodeProcess")]
    static extern bool GetExitCodeProcessReal(IntPtr hProcess, out uint lpExitCode);

    static bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode)
    {
        if (FailGetExitCode) { lpExitCode = 0; SetLastError(6); return false; }
        return GetExitCodeProcessReal(hProcess, out lpExitCode);
    }
'@ 'GetExitCodeProcess'

    # Records the child's PID so a test can assert the process is really gone
    # after an injected failure (the orphan-child concern from the release review).
    $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcess(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode, EntryPoint = "CreateProcess")]
    static extern bool CreateProcessReal(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation);

    static bool CreateProcess(string lpApplicationName, StringBuilder lpCommandLine,
        IntPtr lpProcessAttributes, IntPtr lpThreadAttributes, bool bInheritHandles,
        uint dwCreationFlags, IntPtr lpEnvironment, string lpCurrentDirectory,
        ref STARTUPINFO lpStartupInfo, out PROCESS_INFORMATION lpProcessInformation)
    {
        bool ok = CreateProcessReal(lpApplicationName, lpCommandLine, lpProcessAttributes,
            lpThreadAttributes, bInheritHandles, dwCreationFlags, lpEnvironment,
            lpCurrentDirectory, ref lpStartupInfo, out lpProcessInformation);
        if (ok) LastProcessId = lpProcessInformation.dwProcessId;
        return ok;
    }
'@ 'CreateProcess'

    if ($JobObject) {
        $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint ResumeThread(IntPtr hThread);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "ResumeThread")]
    static extern uint ResumeThreadReal(IntPtr hThread);

    static uint ResumeThread(IntPtr hThread)
    {
        if (FailResume) { SetLastError(5); return 0xFFFFFFFF; }
        return ResumeThreadReal(hThread);
    }
'@ 'ResumeThread'

        $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "AssignProcessToJobObject")]
    static extern bool AssignProcessToJobObjectReal(IntPtr hJob, IntPtr hProcess);

    static bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess)
    {
        if (FailAssign) { SetLastError(5); return false; }
        return AssignProcessToJobObjectReal(hJob, hProcess);
    }
'@ 'AssignProcessToJobObject'

        $src = Edit-SourceOnce $src @'
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength);
'@ @'
    [DllImport("kernel32.dll", SetLastError = true, EntryPoint = "SetInformationJobObject")]
    static extern bool SetInformationJobObjectReal(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength);

    static bool SetInformationJobObject(IntPtr hJob, int JobObjectInfoClass, IntPtr lpJobObjectInfo, uint cbJobObjectInfoLength)
    {
        SetInfoCalls++;
        if (FailSetInfo || (SetInfoFailOnCall > 0 && SetInfoCalls == SetInfoFailOnCall)) { SetLastError(87); return false; }
        return SetInformationJobObjectReal(hJob, JobObjectInfoClass, lpJobObjectInfo, cbJobObjectInfoLength);
    }
'@ 'SetInformationJobObject'
    }

    # Wrapping in a namespace keeps the ORIGINAL class name (so the copy really is
    # the shipped code) while guaranteeing no Add-Type collision with a production
    # class - the "using" lines end up inside the namespace, which is legal C#.
    Add-Type -TypeDefinition ("namespace $Namespace`n{`n" + $src + "`n}`n") -Language CSharp
    return ([System.Management.Automation.PSTypeName]"$Namespace.$ClassName").Type
}

# The 8 non-Job launchers share a byte-identical Run() body (verified), and so
# do 2 of 3 original Job-Object ones - but caps's and capn's Run() bodies
# are NOT byte-identical to the other Job launchers (caps's first Run()
# argument is a timeout in ms that sets its deadline timer's absolute due time, plus a
# deadline branch the others don't have; capn's first Run() argument is an
# active-process count whose distinguishing struct field is ActiveProcessLimit
# (uint), not Affinity (UIntPtr)), which is exactly why they get their own
# compiled probe entries below. These two probes are still enough to cover
# every FAILURE branch in detail. But a byte-identical body is only a claim
# about SOURCE TEXT - the source-shape guard below checks it textually, and
# neither that nor these two probes ever actually RUNS
# admin/cy/cx/capt/capm/caps/capn's own compiled Run(). $script:allLauncherProbes
# (below) compiles and executes all 13, closing that gap with a success-path
# smoke test per file.
$script:probePriority = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'idle.ps1') `
    -Namespace 'WinNiceFaultProbePriority' -ClassName 'IdleLauncher'
$script:probeJob = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'capc.ps1') `
    -Namespace 'WinNiceFaultProbeJob' -ClassName 'CapcLauncher' -JobObject

# One compiled probe per launcher file - reuses the two above for idle/capc
# rather than recompiling them under a different namespace.
$script:allLauncherProbes = [ordered]@{
    idle        = $script:probePriority
    belownormal = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'belownormal.ps1') -Namespace 'WinNiceFaultProbeBelowNormal' -ClassName 'BelowNormalLauncher'
    abovenormal = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'abovenormal.ps1') -Namespace 'WinNiceFaultProbeAboveNormal' -ClassName 'AboveNormalLauncher'
    high        = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'high.ps1') -Namespace 'WinNiceFaultProbeHigh' -ClassName 'HighLauncher'
    realtime    = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'realtime.ps1') -Namespace 'WinNiceFaultProbeRealtime' -ClassName 'RealtimeLauncher'
    cy          = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'cy.ps1') -Namespace 'WinNiceFaultProbeCy' -ClassName 'CyLauncher'
    cx          = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'cx.ps1') -Namespace 'WinNiceFaultProbeCx' -ClassName 'CxLauncher'
    admin       = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'admin.ps1') -Namespace 'WinNiceFaultProbeAdmin' -ClassName 'AdminLauncher'
    capc        = $script:probeJob
    capt        = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'capt.ps1') -Namespace 'WinNiceFaultProbePint' -ClassName 'CaptLauncher' -JobObject
    capm        = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'capm.ps1') -Namespace 'WinNiceFaultProbeCapm' -ClassName 'CapmLauncher' -JobObject
    caps        = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'caps.ps1') -Namespace 'WinNiceFaultProbeCaps' -ClassName 'CapsLauncher' -JobObject
    capn        = New-LauncherFaultProbe -Ps1Path (Join-Path $bin 'capn.ps1') -Namespace 'WinNiceFaultProbeCapn' -ClassName 'CapnLauncher' -JobObject
}

# Kills a probe child that survived a failed assertion (a successful test's
# injected TerminateProcess has already killed it).
function Remove-ProbeChild {
    param([int]$ProcessId)
    if ($ProcessId -gt 0) {
        $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if ($p) { Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue }
    }
}

# TerminateProcess only INITIATES termination and returns asynchronously - an
# immediate Get-Process check right after a successful call can flake under
# load. Poll with a bounded deadline instead of asserting instantly.
function Wait-ProbeChildGone {
    param([int]$ProcessId, [int]$TimeoutMs = 5000)
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    do {
        if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) { return $true }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    return -not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)
}

# One row per launcher file, describing how to call ITS Run() - the embedded
# C# signature differs by shape (see docs/plans/2026-09-02-safehandle-fault-
# injection-plan.md section 1.1): JobArg is non-null only for the 5 Job Object
# launchers (percent/affinity-mask/memory-bytes/timeout-ms/active-process-count
# as their first argument - caps's JobArg is its timeout in ms, which sets its deadline timer's absolute due time; capn's is its active-process count; neither ever makes a
# test slow, since the smoke child exits instantly and every fault case either
# fails before the wait or makes the wait fail immediately),
# HasPriorityFlag is true only for the 5 launchers that take a raw
# dwCreationFlags priority value, and both are absent for cy/cx/admin (no
# first argument at all). ExpectedHandles is ClosedHandles.Count on a clean
# success run - 2 for non-Job (hThread/hProcess), 3 for Job (+hJob). Shared by
# every Describe below so a failure case and a success case for the same
# launcher can never silently disagree on how to invoke it.
# HandlesAtWait is the handle count on any failure AT or AFTER the deadline
# wait (FailWait / FailGetExitCode): caps also owns the deadline waitable timer
# by then (hThread/hProcess/hJob/hTimer = 4), every other launcher matches
# ExpectedHandles - pre-wait failures (ResumeThread/AssignProcessToJobObject)
# happen before caps's timer exists, so they close ExpectedHandles. WaitFn is
# the name of the wait function the launcher declares (caps has no
# WaitForSingleObject since the round-17 timer fix), used for FailWait's
# expected error message.
$launcherExecCases = @(
    @{ Name = 'idle';        HasPriorityFlag = $true;  JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'belownormal'; HasPriorityFlag = $true;  JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'abovenormal'; HasPriorityFlag = $true;  JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'high';        HasPriorityFlag = $true;  JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'realtime';    HasPriorityFlag = $true;  JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'cy';          HasPriorityFlag = $false; JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'cx';          HasPriorityFlag = $false; JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'admin';       HasPriorityFlag = $false; JobArg = $null;      ExpectedHandles = 2; HandlesAtWait = 2; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'capc';        HasPriorityFlag = $null;  JobArg = 50;         ExpectedHandles = 3; HandlesAtWait = 3; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'capt';        HasPriorityFlag = $null;  JobArg = 1;          ExpectedHandles = 3; HandlesAtWait = 3; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'capm';        HasPriorityFlag = $null;  JobArg = 209715200;  ExpectedHandles = 3; HandlesAtWait = 3; WaitFn = 'WaitForSingleObject' }
    @{ Name = 'caps';        HasPriorityFlag = $null;  JobArg = 10000;      ExpectedHandles = 3; HandlesAtWait = 4; WaitFn = 'WaitForMultipleObjects' }
    @{ Name = 'capn';        HasPriorityFlag = $null;  JobArg = 10;         ExpectedHandles = 3; HandlesAtWait = 3; WaitFn = 'WaitForSingleObject' }
)
# Just the 5 Job Object launchers, for fault cases that only exist on that
# shape (ResumeThread/AssignProcessToJobObject/SetInformationJobObject).
# caps and capn join automatically by filtering on non-null JobArg - caps's
# Run() first argument IS the timeout in ms and capn's is its active-process
# count, the exact same call shape Invoke-LauncherProbe
# below already uses for every Job launcher, so no per-launcher special case.
$jobLauncherCases = @($launcherExecCases | Where-Object { $null -ne $_.JobArg })

function Invoke-LauncherProbe {
    param($Case, [object[]]$Argv, [string]$CmdLine)
    $t = $script:allLauncherProbes[$Case.Name]
    if ($null -ne $Case.JobArg) { return $t::Run($Case.JobArg, [string[]]$Argv, $CmdLine) }
    if ($Case.HasPriorityFlag) { return $t::Run(64, [string[]]$Argv, $CmdLine) }
    return $t::Run([string[]]$Argv, $CmdLine)
}

Describe 'native failure branches (fault-injected copy of the embedded C#)' {
    It 'kills the child, reports the wait error, and closes every handle exactly once when the deadline wait fails (<Name>)' -TestCases $launcherExecCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles, $HandlesAtWait, $WaitFn)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $t::FailWait = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @('ping', '-n', '30', '127.0.0.1') -CmdLine 'ping -n 30 127.0.0.1' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        # "failed: 6" (not 0) proves the Win32 error is captured BEFORE the
        # TerminateProcess call, which would otherwise overwrite it.
        $message | Should Be ($WaitFn + ' failed: 6')
        $t::TerminateCalls | Should Be 1
        # Every owned handle closed once, each close succeeded (a double close
        # would return false and bump CloseHandleFailures).
        $t::ClosedHandles.Count | Should Be $HandlesAtWait
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be $HandlesAtWait
        $t::CloseHandleFailures | Should Be 0
        # Fail-closed: the wrapper reported failure, so the child must not still
        # be running in the background.
        Wait-ProbeChildGone -ProcessId $t::LastProcessId | Should Be $true
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'reports the TerminateProcess failure alongside the original error when the best-effort kill itself fails (<Name>)' -TestCases $launcherExecCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles, $HandlesAtWait, $WaitFn)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $t::FailWait = $true
        $t::FailTerminate = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @('ping', '-n', '30', '127.0.0.1') -CmdLine 'ping -n 30 127.0.0.1' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        $message | Should Be ($WaitFn + ' failed: 6; TerminateProcess also failed: 5')
        $t::TerminateCalls | Should Be 1
        $t::ClosedHandles.Count | Should Be $HandlesAtWait
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be $HandlesAtWait
        $t::CloseHandleFailures | Should Be 0
        # The probe's TerminateProcess never called through to the real one -
        # proving the message above isn't silently overclaiming a kill that
        # didn't actually happen. What kills the child then differs by shape:
        # non-Job launchers have no job, nothing else kills it, so it must
        # still be running here. Job-Object launchers' finally closes hJob (the
        # last job handle) after the throw, and KILL_ON_JOB_CLOSE makes Windows
        # terminate the child even though the in-process kill failed - assert
        # the cascade's outcome, with the same bounded poll as elsewhere.
        if ($null -eq $JobArg) {
            (Get-Process -Id $t::LastProcessId -ErrorAction SilentlyContinue) | Should Not Be $null
        } else {
            (Wait-ProbeChildGone -ProcessId $t::LastProcessId) | Should Be $true
        }
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'kills the still-suspended child and reports the resume error when ResumeThread fails (<Name>)' -TestCases $jobLauncherCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $t::FailResume = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @('ping', '-n', '30', '127.0.0.1') -CmdLine 'ping -n 30 127.0.0.1' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        # ERROR_ACCESS_DENIED (5) - deterministic, injected by the probe.
        $message | Should Be 'ResumeThread failed: 5'
        # Without this kill the child stays suspended forever: CREATE_SUSPENDED
        # was never undone and nobody else holds a handle to it.
        $t::TerminateCalls | Should Be 1
        $t::ClosedHandles.Count | Should Be $ExpectedHandles
        $t::CloseHandleFailures | Should Be 0
        Wait-ProbeChildGone -ProcessId $t::LastProcessId | Should Be $true
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'closes the ownership handle(s) exactly once, launches nothing, on the "%" fail-closed branch (<Name>)' -TestCases $launcherExecCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $targetBat = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $targetBat -Value "@echo off`r`nexit /b 0`r`n"
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @($targetBat, '100%OFF') -CmdLine 'x' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        ($message -replace '\s+', ' ') | Should Match ([regex]::Escape("Refusing to run: argument contains '%'"))
        # No process was ever created on this branch. Job-shape launchers
        # already hold hJob at this point (1 handle); non-Job launchers hold
        # nothing yet - ownership only starts once CreateProcess succeeds for
        # them (Part 1's deliberate asymmetry, plan section 2.3).
        $expectedOwnershipHandles = if ($null -ne $JobArg) { 1 } else { 0 }
        $t::ClosedHandles.Count | Should Be $expectedOwnershipHandles
        $t::CloseHandleFailures | Should Be 0
        $t::LastProcessId | Should Be 0
        Remove-Item $targetBat -ErrorAction SilentlyContinue
        $t::ResetProbe()
    }

    It 'kills the still-suspended child and reports the assign error when AssignProcessToJobObject fails (<Name>)' -TestCases $jobLauncherCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $t::FailAssign = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @('ping', '-n', '30', '127.0.0.1') -CmdLine 'ping -n 30 127.0.0.1' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        # ERROR_ACCESS_DENIED (5) - deterministic, injected by the probe.
        $message | Should Be 'AssignProcessToJobObject failed: 5'
        $t::TerminateCalls | Should Be 1
        $t::ClosedHandles.Count | Should Be $ExpectedHandles
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be $ExpectedHandles
        $t::CloseHandleFailures | Should Be 0
        Wait-ProbeChildGone -ProcessId $t::LastProcessId | Should Be $true
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'closes only the job handle and never launches the child when SetInformationJobObject fails (<Name>)' -TestCases $jobLauncherCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $t::FailSetInfo = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @('cmd', '/c', 'exit 7') -CmdLine 'cmd /c "exit 7"' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        # ERROR_INVALID_PARAMETER (87) - deterministic, injected by the probe.
        $message | Should Be 'SetInformationJobObject failed: 87'
        $t::TerminateCalls | Should Be 0
        # No process was ever created on this branch - hJob is the only handle in flight.
        $t::ClosedHandles.Count | Should Be 1
        $t::CloseHandleFailures | Should Be 0
        $t::LastProcessId | Should Be 0
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'reports the exit-code error and still closes every handle when GetExitCodeProcess fails (<Name>)' -TestCases $launcherExecCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles, $HandlesAtWait, $WaitFn)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $t::FailGetExitCode = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @('cmd', '/c', 'exit 7') -CmdLine 'cmd /c "exit 7"' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        $message | Should Be 'GetExitCodeProcess failed: 6'
        # The real wait already succeeded (the child ran to completion) - only
        # the exit-code fetch was faked, so nothing needed killing.
        $t::TerminateCalls | Should Be 0
        $t::ClosedHandles.Count | Should Be $HandlesAtWait
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be $HandlesAtWait
        $t::CloseHandleFailures | Should Be 0
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    # Release-call-specific failure (round-17 P3-2): plain FailSetInfo can only
    # fail EVERY SetInformationJobObject call - which for every Job launcher
    # happens during setup, before the child exists - so the success-path
    # release call was unreachable by fault injection. SetInfoFailOnCall
    # instead fails only call N: the release call is the 2nd in
    # capt/capm/caps/capn (setup, release) and the 3rd in capc (cpu-rate
    # setup, kill-on-close setup, release). This pins all three release-path
    # guarantees: the real exit code survives, the warning hits stderr WITH
    # the Win32 code, and every handle (hJob included) is still closed exactly
    # once - fail-closed for any surviving descendant, since the failed
    # release leaves KILL_ON_JOB_CLOSE armed.
    $jobReleaseCases = @(
        @{ Name = 'capc'; JobArg = 50;        ReleaseCall = 3; Handles = 3 }
        @{ Name = 'capt'; JobArg = 1;         ReleaseCall = 2; Handles = 3 }
        @{ Name = 'capm'; JobArg = 209715200; ReleaseCall = 2; Handles = 3 }
        @{ Name = 'caps'; JobArg = 10000;     ReleaseCall = 2; Handles = 4 }
        @{ Name = 'capn'; JobArg = 10;        ReleaseCall = 2; Handles = 3 }
    )

    It 'preserves the exit code, warns on stderr with the Win32 code, and closes every handle when only the release SetInformationJobObject call fails (<Name>)' -TestCases $jobReleaseCases {
        param($Name, $JobArg, $ReleaseCall, $Handles)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $t::SetInfoFailOnCall = $ReleaseCall
        $stderrWriter = New-Object System.IO.StringWriter
        $oldError = [Console]::Error
        [Console]::SetError($stderrWriter)
        $result = $null
        try {
            $result = Invoke-LauncherProbe -Case @{ HasPriorityFlag = $null; JobArg = $JobArg; Name = $Name } -Argv @('cmd', '/c', 'exit 7') -CmdLine 'cmd /c "exit 7"'
        } finally {
            [Console]::SetError($oldError)
        }
        # (a) The wrapped command's real exit code - not lost, not replaced.
        $result | Should Be 7
        # (b) The warning went to stderr (Console.Error, the exact sink the
        # launchers write to) and carries the injected Win32 code (87).
        $warning = $stderrWriter.ToString()
        $warning | Should Match ([regex]::Escape("could not release the job's kill-on-close guard"))
        $warning | Should Match ([regex]::Escape('SetInformationJobObject failed with Win32 error 87'))
        # The failure landed exactly on the release call: setup call(s) passed,
        # then one failed. (Also pins each launcher's total SetInfo call count
        # on the happy path: 3 for capc, 2 for the rest.)
        $t::SetInfoCalls | Should Be $ReleaseCall
        # (c) Nothing failed fatally: no kill, and every owned handle - hJob
        # included, which is what keeps a surviving descendant fail-closed
        # here - closed exactly once.
        $t::TerminateCalls | Should Be 0
        $t::ClosedHandles.Count | Should Be $Handles
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be $Handles
        $t::CloseHandleFailures | Should Be 0
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    # caps-only timer failure paths (round-18 P3-2): both happen after the
    # child is already running, so neither may leave it unmanaged. The child
    # is killed not by an explicit kill but by the ownership finally closing
    # hJob with KILL_ON_JOB_CLOSE still armed - the same fail-closed backstop
    # as a non-cooperative wrapper death, which is why TerminateCalls stays 0.
    It 'kills the running child via the job guard and reports the error when CreateWaitableTimer fails (caps)' {
        $t = $script:allLauncherProbes['caps']
        $t::ResetProbe()
        $t::FailCreateTimer = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $null; JobArg = 30000; Name = 'caps' } -Argv @('ping', '-n', '30', '127.0.0.1') -CmdLine 'ping -n 30 127.0.0.1' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        $message | Should Be 'CreateWaitableTimer failed: 5'
        # No explicit kill: the armed kill-on-close guard firing on the
        # finally's hJob close is what terminates the child.
        $t::TerminateCalls | Should Be 0
        # hTimer was never acquired - exactly the three other handles close, once each.
        $t::ClosedHandles.Count | Should Be 3
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be 3
        $t::CloseHandleFailures | Should Be 0
        Wait-ProbeChildGone -ProcessId $t::LastProcessId | Should Be $true
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    It 'kills the running child via the job guard, reports the error, and closes all four handles when SetWaitableTimer fails (caps)' {
        $t = $script:allLauncherProbes['caps']
        $t::ResetProbe()
        $t::FailSetTimer = $true
        $message = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $null; JobArg = 30000; Name = 'caps' } -Argv @('ping', '-n', '30', '127.0.0.1') -CmdLine 'ping -n 30 127.0.0.1' | Out-Null
        } catch {
            $message = $_.Exception.InnerException.Message
        }
        $message | Should Be 'SetWaitableTimer failed: 5'
        $t::TerminateCalls | Should Be 0
        # All four owned handles (hThread/hProcess/hJob/hTimer) close exactly once.
        $t::ClosedHandles.Count | Should Be 4
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be 4
        $t::CloseHandleFailures | Should Be 0
        Wait-ProbeChildGone -ProcessId $t::LastProcessId | Should Be $true
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }

    # Round-18 P2 regression: WaitForMultipleObjects reports the LOWEST
    # signaled index, so when BOTH handles are already signaled (deadline
    # passed, child exited after it) the process "wins" the tie by array
    # order alone. The stub below forces exactly that state deterministically:
    # it blocks until both are really signaled (child exited AND timer due)
    # and then reports only index 0. Production must then use the kernel's
    # own exit timestamp (GetProcessTimes) to notice the exit happened after
    # the 600 ms deadline and take the timeout path (124-style
    # TimeoutException + job kill) instead of reporting the child's exit
    # code 0 as an on-time success. A child sleeping 1500 ms cannot exit
    # before a 600 ms deadline (startup only adds time), so this is
    # deterministic, not probabilistic.
    It 'rejects a post-deadline exit that won the lowest-index tie (GetProcessTimes tie-break, caps)' {
        $t = $script:allLauncherProbes['caps']
        $t::ResetProbe()
        $t::ForceBothSignaledThenReturnProcess = $true
        $threw = $null
        try {
            Invoke-LauncherProbe -Case @{ HasPriorityFlag = $null; JobArg = 600; Name = 'caps' } -Argv @('powershell', '-NoProfile', '-Command', 'Start-Sleep -Milliseconds 1500') -CmdLine 'x' | Out-Null
        } catch {
            $threw = $_.Exception
        }
        # The old code returned the child's exit code 0 here (false success).
        $threw | Should Not Be $null
        if ($threw.InnerException) { $ex = $threw.InnerException } else { $ex = $threw }
        ($ex -is [System.TimeoutException]) | Should Be $true
        $ex.Message | Should Match ([regex]::Escape('timed out after 600 ms'))
        # Timeout path killed the job (TerminateJobObject - not the instrumented
        # TerminateProcess, hence 0) and the child is really gone.
        $t::TerminateCalls | Should Be 0
        Wait-ProbeChildGone -ProcessId $t::LastProcessId | Should Be $true
        $t::ClosedHandles.Count | Should Be 4
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be 4
        $t::CloseHandleFailures | Should Be 0
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }
}

# $launcherExecCases (defined above, alongside Invoke-LauncherProbe) drives
# both the failure-branch Describe above and this one - the source-shape
# guard below checks the other files' SOURCE TEXT matches the same template,
# but neither that nor a fault case alone proves admin/cy/cx/capt/capm/caps/capn's own
# compiled Run() actually executes correctly end to end. This closes that
# gap: every one of the 13 gets a real success-path execution, proving each
# file's unique body (comment wording, per-tool struct/flag differences)
# still compiles, links, and runs correctly - not just that a line count
# matches.
Describe 'fault-injection probes actually execute all 13 launcher files (not just idle/capc)' {
    It 'runs Run() for real, propagates the exit code, and closes the expected handle count (<Name>)' -TestCases $launcherExecCases {
        param($Name, $HasPriorityFlag, $JobArg, $ExpectedHandles, $HandlesAtWait, $WaitFn)
        $t = $script:allLauncherProbes[$Name]
        $t::ResetProbe()
        $result = Invoke-LauncherProbe -Case @{ HasPriorityFlag = $HasPriorityFlag; JobArg = $JobArg; Name = $Name } -Argv @('cmd', '/c', 'exit 7') -CmdLine 'cmd /c "exit 7"'
        $result | Should Be 7
        $t::TerminateCalls | Should Be 0
        # Success reaches the deadline wait, so caps's timer handle is included (HandlesAtWait).
        $t::ClosedHandles.Count | Should Be $HandlesAtWait
        (($t::ClosedHandles) | Select-Object -Unique).Count | Should Be $HandlesAtWait
        $t::CloseHandleFailures | Should Be 0
        Remove-ProbeChild -ProcessId $t::LastProcessId
        $t::ResetProbe()
    }
}

$launcherSourceFiles = @(
    @{ Name = 'idle';        Shape = 'Priority' }
    @{ Name = 'belownormal'; Shape = 'Priority' }
    @{ Name = 'abovenormal'; Shape = 'Priority' }
    @{ Name = 'high';        Shape = 'Priority' }
    @{ Name = 'realtime';    Shape = 'Priority' }
    @{ Name = 'cy';          Shape = 'Priority' }
    @{ Name = 'cx';          Shape = 'Priority' }
    @{ Name = 'admin';       Shape = 'Priority' }
    @{ Name = 'capc';        Shape = 'Job' }
    @{ Name = 'capt';        Shape = 'Job' }
    @{ Name = 'capm';        Shape = 'Job' }
    @{ Name = 'caps';        Shape = 'Job' }
    @{ Name = 'capn';        Shape = 'Job' }
)

Describe 'embedded launcher C# keeps the single-owner cleanup shape (<Name>)' {
    It 'closes each handle exactly once, only from the ownership finally (<Name>)' -TestCases $launcherSourceFiles {
        param($Name, $Shape)
        $src = Get-LauncherCSharp -Ps1Path (Join-Path $bin "$Name.ps1")
        # Count only the CLOSES inside Run() own body: one per owned handle,
        # all inside the single ownership finally. Any per-branch CloseHandle
        # coming back bumps this count. The [DllImport] declaration itself
        # lives outside Run() (asserted separately below), so it is not part
        # of this count. (caps ProbePastDueTimerWait test hook has its own
        # separate finally with a CloseHandle - deliberately scoped out: it
        # is not the ownership pattern this guard protects.)
        $runSrc = $src.Substring($src.IndexOf('public static int Run('))
        $expected = 2
        if ($Shape -eq 'Job') { $expected = 3 }
        if ($Name -eq 'caps') { $expected = 4 }
        ([regex]::Matches($runSrc, 'CloseHandle\(')).Count | Should Be $expected
        # The probe transform in this file anchors on these exact declarations.
        $src | Should Match ([regex]::Escape('static extern bool CloseHandle(IntPtr hObject);'))
        $src | Should Match ([regex]::Escape('static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);'))
        if ($Name -eq 'caps') {
            # caps waits via WaitForMultipleObjects on {process, timer} (round-17
            # waitable-timer fix) and declares no WaitForSingleObject at all.
            $src | Should Match ([regex]::Escape('static extern uint WaitForMultipleObjects(uint nCount, IntPtr[] lpHandles, bool bWaitAll, uint dwMilliseconds);'))
        } else {
            $src | Should Match ([regex]::Escape('static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);'))
        }
    }
}

Describe 'Job Object kill-on-close cascade when the launcher is killed non-cooperatively (capc.ps1)' {
    It 'terminates the wrapped child and its grandchild when the launcher itself is taskkilled /F without /T' {
        $marker = 'win-nice-cascade-' + [guid]::NewGuid().ToString('N')
        $childPidFile = New-TempFile
        $grandchildPidFile = New-TempFile
        # The grandchild script's own path carries the unique marker, it writes
        # its PID, and it self-terminates on a bounded deadline - so a failure
        # path can never leak a live process even if an assertion below aborts.
        $grandchildFile = Join-Path $script:testRoot ($marker + '.ps1')
        Set-Content -Path $grandchildFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
Set-Content -Path '$grandchildPidFile' -Value `$PID
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        # Same rule as every other multi-hop test in this file: nested scripts go
        # in temp FILES, never inline -Command strings (triple-nested quoting).
        # The child writes its PID, spawns the grandchild, then sleeps - long
        # enough to still be alive when the test kills the launcher mid-flight,
        # bounded so a failure path can't wedge the suite.
        $outerFile = New-TempScript
        Set-Content -Path $outerFile -Value @"
Set-Content -Path '$childPidFile' -Value `$PID
Start-Process powershell -ArgumentList @('-NoProfile', '-File', '$grandchildFile') -WindowStyle Hidden | Out-Null
Start-Sleep -Seconds 60
"@
        $launcher = Start-Process powershell -ArgumentList @('-NoProfile', '-File', (Join-Path $bin 'capc.ps1'), '50', 'powershell', '-NoProfile', '-File', $outerFile) -WindowStyle Hidden -PassThru
        $childPid = 0
        $grandchildPid = 0
        try {
            # Bounded wait until BOTH the child and its grandchild are confirmed
            # running - killing the launcher only proves anything once the
            # grandchild is really inside the job.
            $deadline = [DateTime]::UtcNow.AddSeconds(15)
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($childPid -eq 0 -and (Test-Path $childPidFile)) {
                    $childPid = [int](Get-Content $childPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $grandchildPid -eq 0 -and (Test-Path $grandchildPidFile)) {
                    $grandchildPid = [int](Get-Content $grandchildPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $grandchildPid -ne 0) { break }
                Start-Sleep -Milliseconds 100
            }
            $childPid | Should Not Be 0
            $grandchildPid | Should Not Be 0
            (Get-Process -Id $childPid -ErrorAction SilentlyContinue) | Should Not Be $null
            (Get-Process -Id $grandchildPid -ErrorAction SilentlyContinue) | Should Not Be $null

            # The bug scenario itself: /F = hard kill (no in-process cleanup can
            # run), and deliberately NO /T - any tree-wide cleanup must come from
            # the job's kill-on-close cascade, not from taskkill itself.
            & taskkill /F /PID $launcher.Id | Out-Null
            $LASTEXITCODE | Should Be 0
            (Wait-ProbeChildGone -ProcessId $launcher.Id -TimeoutMs 10000) | Should Be $true

            # Cascade assertion, bounded poll (job termination completes
            # asynchronously - an instant check is a documented race here).
            (Wait-ProbeChildGone -ProcessId $grandchildPid -TimeoutMs 15000) | Should Be $true
            (Wait-ProbeChildGone -ProcessId $childPid -TimeoutMs 15000) | Should Be $true
        } finally {
            # Best-effort cleanup on every path - a failed assertion above must
            # not leave the launcher, child, or grandchild running (the two
            # generated scripts also self-terminate within 60s as a backstop).
            if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
            if ($childPid -gt 0) { Remove-ProbeChild -ProcessId $childPid }
            if ($grandchildPid -gt 0) { Remove-ProbeChild -ProcessId $grandchildPid }
            Remove-Item $childPidFile, $grandchildPidFile, $grandchildFile, $outerFile -ErrorAction SilentlyContinue
        }
    }

    It 'sets JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE in every Job-Object launcher (<Name>)' -TestCases @(
        @{ Name = 'capc' }
        @{ Name = 'capt' }
        @{ Name = 'capm' }
        @{ Name = 'caps' }
        @{ Name = 'capn' }
    ) {
        param($Name)
        # Textual companion to the behavioral cascade test above (which exercises
        # capc only): the flag must be present AND OR'd into a LimitFlags
        # assignment in all five files, so a future edit can't silently drop it
        # from capt/capm/caps/capn while the capc-only cascade stays green.
        $src = Get-LauncherCSharp -Ps1Path (Join-Path $bin "$Name.ps1")
        $src | Should Match ([regex]::Escape('JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000'))
        $src | Should Match 'LimitFlags\s*=\s*[^;\r\n]*JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE'
    }
}

Describe 'clean root exit leaves a detached daemon alive (kill-on-close released on success)' {
    # The OPPOSITE direction from the cascade tests above, which prove everything
    # in the job dies together on an abnormal path. Here the wrapped root spawns
    # an independent daemon and then exits successfully ON ITS OWN - the exact
    # build-daemon/watcher scenario README's daemon-survival paragraph documents.
    # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE terminates every process still in the job
    # when the last job handle closes for ANY reason, including the wrapper's own
    # orderly finally-close, so each launcher must clear the flag on its success
    # path before that close - or the daemon dies even though the wrapped command
    # returned 0. Behavioral regression for release review round 16
    # (docs/reviews/2026-09-03-1619-6329eef) P1-1. The daemon self-terminates on
    # a bounded 60s deadline (job death must NOT be what stops it), and the
    # finally block stops it explicitly like every other multi-process test.
    It 'keeps the daemon the wrapped command left running alive after the wrapper exits 0 (<Name>)' -TestCases @(
        @{ Name = 'capc'; ToolArg = '50' }
        @{ Name = 'capt'; ToolArg = '1' }
        @{ Name = 'capm'; ToolArg = '2g' }
        @{ Name = 'caps'; ToolArg = '30' }
        @{ Name = 'capn'; ToolArg = '10' }
    ) {
        param($Name, $ToolArg)
        $childPidFile = New-TempFile
        $daemonPidFile = New-TempFile
        $daemonFile = New-TempScript
        Set-Content -Path $daemonFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
Set-Content -Path '$daemonPidFile' -Value `$PID
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        # The child (the directly wrapped root) writes its PID, spawns the daemon
        # via Start-Process (independent and detached; it inherits the child's job
        # membership - the cascade tests prove grandchild-in-job), then exits 0 on
        # its own: no timeout, no kill, a completely normal exit.
        $outerFile = New-TempScript
        Set-Content -Path $outerFile -Value @"
Set-Content -Path '$childPidFile' -Value `$PID
Start-Process powershell -ArgumentList @('-NoProfile', '-File', '$daemonFile') -WindowStyle Hidden | Out-Null
exit 0
"@
        $launcher = Start-Process powershell -ArgumentList @('-NoProfile', '-File', (Join-Path $bin "$Name.ps1"), $ToolArg, 'powershell', '-NoProfile', '-File', $outerFile) -WindowStyle Hidden -PassThru
        $childPid = 0
        $daemonPid = 0
        try {
            # Bounded wait until the child has written its PID, the daemon its
            # own, and the WRAPPER has exited - the ordering proves the daemon
            # was inside the job before the wrapper's finally closed hJob.
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($childPid -eq 0 -and (Test-Path $childPidFile)) {
                    $childPid = [int](Get-Content $childPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $daemonPid -eq 0 -and (Test-Path $daemonPidFile)) {
                    $daemonPid = [int](Get-Content $daemonPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $daemonPid -ne 0 -and $launcher.HasExited) { break }
                Start-Sleep -Milliseconds 100
            }
            $childPid | Should Not Be 0
            $daemonPid | Should Not Be 0
            # The wrapper finished normally, propagating the wrapped root's real
            # exit code - not 124, not 1.
            $launcher.HasExited | Should Be $true
            $launcher.ExitCode | Should Be 0
            # The root exited on its own (nothing killed it)...
            (Wait-ProbeChildGone -ProcessId $childPid -TimeoutMs 15000) | Should Be $true
            # ...and the daemon it left behind is STILL ALIVE several seconds
            # after the wrapper's process is gone - the documented contract.
            # The opposite assertion of the cascade tests: nothing dies here.
            Start-Sleep -Seconds 3
            (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue) | Should Not Be $null
            # Round-17 P3-1 (preferred probe, capt only): survival alone doesn't
            # prove the limit survived with it - check the surviving daemon's
            # LIVE affinity mask. ToolArg '1' = first logical processor only,
            # so the job's JOB_OBJECT_LIMIT_AFFINITY must still pin the daemon
            # to mask 0x1 now that the wrapper's handle is long gone.
            if ($Name -eq 'capt') {
                $affinity = (Get-Process -Id $daemonPid -ErrorAction Stop).ProcessorAffinity
                ([int64]$affinity) | Should Be 1
            }
        } finally {
            # Best-effort cleanup on every path - a failed assertion above must
            # not leak the launcher, child, or daemon (the generated scripts
            # also self-terminate within 60s as a backstop).
            if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
            if ($childPid -gt 0) { Remove-ProbeChild -ProcessId $childPid }
            if ($daemonPid -gt 0) { Remove-ProbeChild -ProcessId $daemonPid }
            Remove-Item $childPidFile, $daemonPidFile, $daemonFile, $outerFile -ErrorAction SilentlyContinue
        }
    }

    It 'releases the kill-on-close guard AND re-applies the launcher own preserved limit in the release struct (<Name>)' -TestCases @(
        @{ Name = 'capc'; Flags = 'LimitFlags = 0';                               Extra = $null }
        @{ Name = 'capt'; Flags = 'LimitFlags = JOB_OBJECT_LIMIT_AFFINITY';       Extra = 'Affinity = (UIntPtr)affinityMask' }
        @{ Name = 'capm'; Flags = 'LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY';     Extra = 'JobMemoryLimit = (UIntPtr)memoryLimitBytes' }
        @{ Name = 'caps'; Flags = 'LimitFlags = 0';                               Extra = $null }
        @{ Name = 'capn'; Flags = 'LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS'; Extra = 'ActiveProcessLimit = activeProcessLimit' }
    ) {
        param($Name, $Flags, $Extra)
        # Round-17 P3-1 (minimum, all five): the release call must re-apply the
        # launcher's OWN preserved limit, not just "some" SetInformationJobObject
        # call - a future refactor that cleared the wrong limit (LimitFlags = 0
        # in capt/capm/capn, or an accidental CPU-rate reset in capc) must fail
        # here. Anchor on the release struct (var releaseInfo) so the SETUP
        # struct's kill-on-close OR-assignment can't satisfy the match.
        $src = Get-LauncherCSharp -Ps1Path (Join-Path $bin "$Name.ps1")
        $idx = $src.IndexOf('var releaseInfo')
        $idx | Should BeGreaterThan 0
        $releaseBlock = $src.Substring($idx)
        $releaseBlock | Should Match ([regex]::Escape('SetInformationJobObject(hJob, JobObjectExtendedLimitInformation, releasePtr, (uint)releaseSize)'))
        $releaseBlock | Should Match ([regex]::Escape("could not release the job's kill-on-close guard"))
        $releaseBlock | Should Match ([regex]::Escape($Flags))
        if ($Extra) { $releaseBlock | Should Match ([regex]::Escape($Extra)) }
        if ($Name -eq 'capc') {
            # capc's CPU-rate limit lives in a SEPARATE info class and must be
            # untouched by the release path: the identifier legitimately
            # appears twice in the whole file (constant declaration + setup
            # call), but NEVER anywhere in the release block below
            # 'var releaseInfo' - any occurrence there is a reset bug.
            ([regex]::Matches($releaseBlock, 'JobObjectCpuRateControlInformation')).Count | Should Be 0
        }
    }

    It 'still enforces the process-count ceiling on the surviving daemon after the wrapper exits (capn)' {
        # Round-17 P3-1 (preferred probe): extends the daemon-survival scenario
        # above with capn-specific behavior. Limit 2: the wrapped root (1 active)
        # spawns the daemon (2) - allowed - then exits 0; the wrapper exits 0 and
        # releases the kill-on-close guard. After the wrapper is gone, the
        # surviving daemon attempts two spawns: the first must succeed (daemon +
        # child = 2 <= 2), the second must still be refused (would be 3 > 2) -
        # proving the kernel still enforces the ceiling on the live job, not
        # just that the launcher's source struct mentions it. Same
        # failure-detection pattern as capn's own over-limit test above:
        # Start-Process throws when the job refuses the spawn.
        $childPidFile = New-TempFile
        $daemonPidFile = New-TempFile
        $resultFile = New-TempFile
        $goFile = New-TempFile
        # The daemon waits for the test's go signal (written only after the
        # wrapper has exited), then attempts the two spawns and records both
        # outcomes. Children sleep bounded and are stopped by the daemon, so a
        # failure path can't leak live processes (the daemon also self-terminates
        # on its 60s deadline as a backstop).
        $daemonFile = New-TempScript
        Set-Content -Path $daemonFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
Set-Content -Path '$daemonPidFile' -Value `$PID
while (-not (Test-Path '$goFile') -and [DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 100 }
`$kids = @()
`$spawn1Ok = `$false
`$spawn2Failed = `$false
try {
    `$p1 = Start-Process powershell -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 10') -WindowStyle Hidden -PassThru
    `$kids += `$p1
    `$spawn1Ok = `$true
    Start-Sleep -Milliseconds 500
    try {
        `$p2 = Start-Process powershell -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 10') -WindowStyle Hidden -PassThru
        `$kids += `$p2
    } catch {
        `$spawn2Failed = `$true
    }
} catch {
    # Spawn1 itself refused: the ceiling fired too early - record honestly
    # (the assertion below fails on SPAWN1-OK=False rather than guessing).
}
Set-Content -Path '$resultFile' -Value "SPAWN1-OK=`$spawn1Ok SPAWN2-FAILED=`$spawn2Failed"
foreach (`$k in `$kids) { Stop-Process -Id `$k.Id -Force -ErrorAction SilentlyContinue }
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        $outerFile = New-TempScript
        Set-Content -Path $outerFile -Value @"
Set-Content -Path '$childPidFile' -Value `$PID
Start-Process powershell -ArgumentList @('-NoProfile', '-File', '$daemonFile') -WindowStyle Hidden | Out-Null
exit 0
"@
        $launcher = Start-Process powershell -ArgumentList @('-NoProfile', '-File', (Join-Path $bin 'capn.ps1'), '2', 'powershell', '-NoProfile', '-File', $outerFile) -WindowStyle Hidden -PassThru
        $childPid = 0
        $daemonPid = 0
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($childPid -eq 0 -and (Test-Path $childPidFile)) {
                    $childPid = [int](Get-Content $childPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $daemonPid -eq 0 -and (Test-Path $daemonPidFile)) {
                    $daemonPid = [int](Get-Content $daemonPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $daemonPid -ne 0 -and $launcher.HasExited) { break }
                Start-Sleep -Milliseconds 100
            }
            $childPid | Should Not Be 0
            $daemonPid | Should Not Be 0
            $launcher.HasExited | Should Be $true
            $launcher.ExitCode | Should Be 0
            (Wait-ProbeChildGone -ProcessId $childPid -TimeoutMs 15000) | Should Be $true
            (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue) | Should Not Be $null
            # Wrapper is gone - let the daemon try its luck against the ceiling.
            Set-Content -Path $goFile -Value 'go'
            $resultDeadline = [DateTime]::UtcNow.AddSeconds(20)
            while (-not (Test-Path $resultFile) -and [DateTime]::UtcNow -lt $resultDeadline) { Start-Sleep -Milliseconds 100 }
            (Test-Path $resultFile) | Should Be $true
            (Get-Content $resultFile).Trim() | Should Be 'SPAWN1-OK=True SPAWN2-FAILED=True'
            (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue) | Should Not Be $null
        } finally {
            if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
            if ($childPid -gt 0) { Remove-ProbeChild -ProcessId $childPid }
            if ($daemonPid -gt 0) { Remove-ProbeChild -ProcessId $daemonPid }
            Remove-Item $childPidFile, $daemonPidFile, $daemonFile, $outerFile, $resultFile, $goFile -ErrorAction SilentlyContinue
        }
    }

    It 'kernel-queries the surviving daemon job and confirms the memory ceiling is still set with kill-on-close released (capm)' {
        # Round-17 P3-1 middle ground for capm: stronger than a source-struct
        # assertion, without the cost/flakiness of a real allocation-past-
        # ceiling. After the wrapper has exited, the surviving daemon queries
        # its OWN job (QueryInformationJobObject with a NULL handle queries the
        # caller's job) and reports the KERNEL's current limit state. capm 2g:
        # LimitFlags must be exactly 0x200 (JOB_OBJECT_LIMIT_JOB_MEMORY) - the
        # ceiling is still there AND kill-on-close (0x2000) is really gone from
        # the kernel object, not just from the source - and JobMemoryLimit must
        # still be 2 GB = 2147483648.
        $childPidFile = New-TempFile
        $daemonPidFile = New-TempFile
        $resultFile = New-TempFile
        $goFile = New-TempFile
        $csFile = Join-Path $script:testRoot ('jobquery-' + [guid]::NewGuid().ToString('N') + '.cs')
        Set-Content -Path $csFile -Value @'
using System;
using System.Runtime.InteropServices;
public static class WinNiceDaemonJobQuery {
    [StructLayout(LayoutKind.Sequential)]
    public struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
        public long PerProcessUserTimeLimit;
        public long PerJobUserTimeLimit;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass;
        public uint SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct IO_COUNTERS {
        public ulong ReadOperationCount;
        public ulong WriteOperationCount;
        public ulong OtherOperationCount;
        public ulong ReadTransferCount;
        public ulong WriteTransferCount;
        public ulong OtherTransferCount;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
        public IO_COUNTERS IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool QueryInformationJobObject(IntPtr hJob, int JobObjectInfoClass,
        ref JOBOBJECT_EXTENDED_LIMIT_INFORMATION lpJobObjectInfo, uint cbJobObjectInfoLength, out uint lpReturnLength);
}
'@
        $daemonFile = New-TempScript
        Set-Content -Path $daemonFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
Set-Content -Path '$daemonPidFile' -Value `$PID
Add-Type -Path '$csFile'
while (-not (Test-Path '$goFile') -and [DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 100 }
`$info = New-Object WinNiceDaemonJobQuery+JOBOBJECT_EXTENDED_LIMIT_INFORMATION
`$returned = [uint32]0
`$ok = [WinNiceDaemonJobQuery]::QueryInformationJobObject([IntPtr]::Zero, 9, [ref]`$info, [uint32][System.Runtime.InteropServices.Marshal]::SizeOf(`$info), [ref]`$returned)
Set-Content -Path '$resultFile' -Value "OK=`$ok FLAGS=`$(`$info.BasicLimitInformation.LimitFlags) JOBMEM=`$(`$info.JobMemoryLimit)"
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        $outerFile = New-TempScript
        Set-Content -Path $outerFile -Value @"
Set-Content -Path '$childPidFile' -Value `$PID
Start-Process powershell -ArgumentList @('-NoProfile', '-File', '$daemonFile') -WindowStyle Hidden | Out-Null
exit 0
"@
        $launcher = Start-Process powershell -ArgumentList @('-NoProfile', '-File', (Join-Path $bin 'capm.ps1'), '2g', 'powershell', '-NoProfile', '-File', $outerFile) -WindowStyle Hidden -PassThru
        $childPid = 0
        $daemonPid = 0
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($childPid -eq 0 -and (Test-Path $childPidFile)) {
                    $childPid = [int](Get-Content $childPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $daemonPid -eq 0 -and (Test-Path $daemonPidFile)) {
                    $daemonPid = [int](Get-Content $daemonPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $daemonPid -ne 0 -and $launcher.HasExited) { break }
                Start-Sleep -Milliseconds 100
            }
            $childPid | Should Not Be 0
            $daemonPid | Should Not Be 0
            $launcher.HasExited | Should Be $true
            $launcher.ExitCode | Should Be 0
            (Wait-ProbeChildGone -ProcessId $childPid -TimeoutMs 15000) | Should Be $true
            (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue) | Should Not Be $null
            # Wrapper is gone - ask the KERNEL what the daemon's job looks like.
            Set-Content -Path $goFile -Value 'go'
            $resultDeadline = [DateTime]::UtcNow.AddSeconds(20)
            while (-not (Test-Path $resultFile) -and [DateTime]::UtcNow -lt $resultDeadline) { Start-Sleep -Milliseconds 100 }
            (Test-Path $resultFile) | Should Be $true
            (Get-Content $resultFile).Trim() | Should Be 'OK=True FLAGS=512 JOBMEM=2147483648'
            (Get-Process -Id $daemonPid -ErrorAction SilentlyContinue) | Should Not Be $null
        } finally {
            if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
            if ($childPid -gt 0) { Remove-ProbeChild -ProcessId $childPid }
            if ($daemonPid -gt 0) { Remove-ProbeChild -ProcessId $daemonPid }
            Remove-Item $childPidFile, $daemonPidFile, $daemonFile, $outerFile, $resultFile, $goFile, $csFile -ErrorAction SilentlyContinue
        }
    }
}

Describe 'caps.ps1 argument validation' {
    # Same driver pattern as capm/capc validation tests: through the .bat wrapper
    # (real user entry point), stderr discarded, only the exit code asserted.
    It 'rejects a non-numeric seconds value' {
        & (Join-Path $bin 'caps.bat') abc cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects 0' {
        & (Join-Path $bin 'caps.bat') 0 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a negative value' {
        & (Join-Path $bin 'caps.bat') -1 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a missing command' {
        & (Join-Path $bin 'caps.bat') 5 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a huge 400-digit <seconds> cleanly with the usage error - no raw PowerShell conversion error leaked' {
        # Same TryParse rationale as capm's huge-digit test above: the regex has
        # no length limit, so an absurdly long digit string must fail cleanly
        # into the controlled usage error rather than crash with a raw cast
        # error. Whitespace-normalize before matching (Pester-console word-wrap).
        $digits = '9' * 400
        $out = & (Join-Path $bin 'caps.bat') $digits cmd /c "echo hi" 2>&1
        $LASTEXITCODE | Should Be 1
        (($out | Out-String) -replace '\s+', ' ') | Should Match 'out of range'
        (($out | Out-String) -replace '\s+', ' ') | Should Not Match 'Cannot convert value'
    }

    It 'rejects a seconds value whose millisecond conversion overflows the uint32/INFINITE boundary (<Seconds>)' -TestCases @(
        @{ Seconds = '4294967295' }
        @{ Seconds = '4294967.3' }
    ) {
        param($Seconds)
        # 0xFFFFFFFF ms is WaitForMultipleObjects' wait-forever sentinel, not a
        # deadline - anything converting to more than 0xFFFFFFFE ms must be the
        # clean usage error. The decimal case is the same guard for a fractional
        # seconds value whose *1000 conversion crosses the boundary (a value
        # that looks small in seconds but overflows in milliseconds).
        & (Join-Path $bin 'caps.bat') $Seconds cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'shows the out-of-range usage error (not a crash) for the overflowing decimal value' {
        $out = & (Join-Path $bin 'caps.bat') 4294967.3 cmd /c "echo hi" 2>&1
        $LASTEXITCODE | Should Be 1
        (($out | Out-String) -replace '\s+', ' ') | Should Match 'out of range'
    }

    It 'accepts a seconds value just under the boundary and propagates the wrapped exit code' {
        # 4294967 seconds = 4,294,967,000 ms <= 4,294,967,294 (0xFFFFFFFE) - the
        # deadline is accepted and only BOUNDS the wait, never delays it: the
        # child exits instantly, so this must be fast.
        & (Join-Path $bin 'caps.bat') 4294967 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
    }
}

Describe 'caps.ps1 behavior' {
    It 'propagates the wrapped exit code when the command finishes inside the timeout' {
        & (Join-Path $bin 'caps.bat') 30 cmd /c "exit 7"
        $LASTEXITCODE | Should Be 7
    }

    It 'times out at the deadline, kills the wrapped child, and exits 124 with a stderr message' {
        $childPidFile = New-TempFile
        # Generated as a temp FILE (never inline -Command), same pattern as the
        # cascade test: writes its PID, then sleeps up to a bounded 60s
        # self-terminate deadline so a failure path can't wedge the suite.
        $hungFile = New-TempScript
        Set-Content -Path $hungFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
Set-Content -Path '$childPidFile' -Value `$PID
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        try {
            $stderr = & powershell -NoProfile -File (Join-Path $bin 'caps.ps1') 2 powershell -NoProfile -File $hungFile 2>&1
            $exitCode = $LASTEXITCODE
            $exitCode | Should Be 124
            # The console word-wraps Write-Error text (same technique as the
            # "%"-fail-closed assertion), so match whitespace-normalized text.
            # Two fragments rather than one full sentence: the 2>&1 capture
            # interleaves the NativeCommandError/CategoryInfo boilerplate
            # BETWEEN the wrapped halves of the message ("...were <boilerplate>
            # force-killed"), which no amount of whitespace collapsing removes.
            $normalized = (($stderr | Out-String) -replace '\s+', ' ')
            $normalized | Should Match ([regex]::Escape('caps: timed out after 2s'))
            $normalized | Should Match ([regex]::Escape('force-killed'))
            $childPid = [int](Get-Content $childPidFile | Select-Object -First 1)
            (Wait-ProbeChildGone -ProcessId $childPid -TimeoutMs 15000) | Should Be $true
        } finally {
            if (Test-Path $childPidFile) {
                Remove-ProbeChild -ProcessId ([int](Get-Content $childPidFile | Select-Object -First 1)) -ErrorAction SilentlyContinue
            }
            Remove-Item $childPidFile, $hungFile -ErrorAction SilentlyContinue
        }
    }

    It 'kills the wrapped child AND its grandchild when the timeout fires' {
        # Same structure as the capc kill-on-close cascade test, but here nobody
        # taskkills anything - the deadline ITSELF is the kill being tested:
        # caps must exit 124 on its own at ~5s and the job's termination must
        # take both generations with it.
        $childPidFile = New-TempFile
        $grandchildPidFile = New-TempFile
        $grandchildFile = New-TempScript
        Set-Content -Path $grandchildFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
Set-Content -Path '$grandchildPidFile' -Value `$PID
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        # Nested scripts in temp FILES, never inline -Command strings (same
        # rule as every other multi-hop test in this file).
        $outerFile = New-TempScript
        Set-Content -Path $outerFile -Value @"
Set-Content -Path '$childPidFile' -Value `$PID
Start-Process powershell -ArgumentList @('-NoProfile', '-File', '$grandchildFile') -WindowStyle Hidden | Out-Null
Start-Sleep -Seconds 60
"@
        $launcher = Start-Process powershell -ArgumentList @('-NoProfile', '-File', (Join-Path $bin 'caps.ps1'), '5', 'powershell', '-NoProfile', '-File', $outerFile) -WindowStyle Hidden -PassThru
        $childPid = 0
        $grandchildPid = 0
        try {
            # Bounded wait until BOTH pid files exist and BOTH processes are
            # confirmed alive - this ordering is what proves the grandchild was
            # already inside the job BEFORE the 5s deadline fired (otherwise a
            # pass could just mean the grandchild never started).
            $deadline = [DateTime]::UtcNow.AddSeconds(4)
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($childPid -eq 0 -and (Test-Path $childPidFile)) {
                    $childPid = [int](Get-Content $childPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $grandchildPid -eq 0 -and (Test-Path $grandchildPidFile)) {
                    $grandchildPid = [int](Get-Content $grandchildPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $grandchildPid -ne 0 -and
                    (Get-Process -Id $childPid -ErrorAction SilentlyContinue) -and
                    (Get-Process -Id $grandchildPid -ErrorAction SilentlyContinue)) { break }
                Start-Sleep -Milliseconds 100
            }
            $childPid | Should Not Be 0
            $grandchildPid | Should Not Be 0
            (Get-Process -Id $childPid -ErrorAction SilentlyContinue) | Should Not Be $null
            (Get-Process -Id $grandchildPid -ErrorAction SilentlyContinue) | Should Not Be $null

            # caps exits BY ITSELF at the deadline - nobody kills the launcher.
            (Wait-ProbeChildGone -ProcessId $launcher.Id -TimeoutMs 15000) | Should Be $true
            $launcher.ExitCode | Should Be 124

            # Cascade assertions, bounded polls (job termination completes
            # asynchronously - an instant check is a documented race).
            (Wait-ProbeChildGone -ProcessId $grandchildPid -TimeoutMs 15000) | Should Be $true
            (Wait-ProbeChildGone -ProcessId $childPid -TimeoutMs 15000) | Should Be $true
        } finally {
            # Best-effort cleanup on every path (the generated scripts also
            # self-terminate within 60s as a backstop).
            if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
            if ($childPid -gt 0) { Remove-ProbeChild -ProcessId $childPid }
            if ($grandchildPid -gt 0) { Remove-ProbeChild -ProcessId $grandchildPid }
            Remove-Item $childPidFile, $grandchildPidFile, $grandchildFile, $outerFile -ErrorAction SilentlyContinue
        }
    }

    It 'returns promptly when the command finishes well inside the deadline (no deadline-stall)' {
        # The wait is WaitForMultipleObjects on {process, timer}: the process
        # handle is signaled the instant the child exits, so the wrapper
        # completes in roughly its own startup runtime, nowhere near the 60s
        # deadline it was given - and the timer adds no delay on this path
        # either, since only the process handle going signaled ends the wait.
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & (Join-Path $bin 'caps.bat') 60 cmd /c "exit 0"
        $sw.Stop()
        $LASTEXITCODE | Should Be 0
        # Generous bound (PowerShell + Add-Type startup dominates). The point:
        # this finishes in seconds against a 60s deadline, never ~60s.
        ($sw.Elapsed.TotalMilliseconds -lt 25000) | Should Be $true
    }

    It 'fires the timeout at approximately the deadline wall-clock time' {
        $hungFile = New-TempScript
        Set-Content -Path $hungFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        try {
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            & powershell -NoProfile -File (Join-Path $bin 'caps.ps1') 3 powershell -NoProfile -File $hungFile 2>&1 | Out-Null
            $sw.Stop()
            $LASTEXITCODE | Should Be 124
            # The timer's ABSOLUTE due time fires at the deadline regardless of
            # anything the wrapper does - there is no poll slice to overshoot
            # by. Elapsed time is deadline + process startup + scheduler
            # jitter, hence the same generous bounds as before.
            ($sw.Elapsed.TotalMilliseconds -ge 2800) | Should Be $true
            ($sw.Elapsed.TotalMilliseconds -lt 15000) | Should Be $true
        } finally {
            Remove-Item $hungFile -ErrorAction SilentlyContinue
        }
    }
}

Describe 'caps.ps1 waitable-timer deadline (sleep/suspend-safe by construction)' {
    # Round-17 P2 fix. The old design (relative WaitForSingleObject poll
    # slices re-derived from UtcNow) had a sleep-blind window: a slice wait
    # ALREADY IN PROGRESS when the machine suspended kept running down its
    # pre-sleep remainder after the wake, so caps could overshoot the deadline
    # by up to a slice and even accept a post-deadline exit as on-time
    # success. The replacement - a one-shot waitable timer with an ABSOLUTE
    # due time, waited on together with the process handle - closes that BY
    # CONSTRUCTION: a timer whose absolute due time has passed comes up
    # already-signaled whenever the system next looks at it; signaled-ness is
    # a property of the absolute clock, not of an in-progress wait
    # (SetWaitableTimer docs). A CI runner can't be genuinely suspended on
    # demand, so these tests exercise the REAL Win32 primitive on the compiled
    # probe copy: an already-past-due absolute time is mechanically identical
    # to a deadline that passed while the machine was asleep, because the
    # timer's signaled state comes up the same way in both cases.
    $t = $script:allLauncherProbes['caps']

    It 'signals immediately (WAIT_OBJECT_0) for an absolute due time already in the past' {
        # 10s past due: exactly the state the timer is in after a suspend that
        # outlasted the deadline. WaitForMultipleObjects must return
        # WAIT_OBJECT_0 right away - not after the 30s budget - proving no
        # relative-wait remainder can postpone the deadline past a wake.
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $result = $t::ProbePastDueTimerWait([DateTime]::UtcNow.AddSeconds(-10), 30000)
        $sw.Stop()
        $result | Should Be 0
        # "Immediately" = scheduler latency, not the wait budget and not a
        # leftover poll slice.
        ($sw.Elapsed.TotalMilliseconds -lt 5000) | Should Be $true
    }

    It 'does not signal an absolute due time in the future before its time (control case)' {
        # Refutation control for the probe above: a due time 5s in the FUTURE
        # must stay unsignaled - WaitForMultipleObjects may only come back
        # WAIT_TIMEOUT (0x102) after the 1.5s budget. Without this, a probe
        # that returned 0 for everything (e.g. a marshaling bug signaling the
        # timer immediately) would still pass the past-due test.
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $result = $t::ProbePastDueTimerWait([DateTime]::UtcNow.AddSeconds(5), 1500)
        $sw.Stop()
        $result | Should Be 0x00000102
        ($sw.Elapsed.TotalMilliseconds -ge 1400) | Should Be $true
        ($sw.Elapsed.TotalMilliseconds -lt 10000) | Should Be $true
    }
}

Describe 'capn.ps1 argument validation' {
    # Same driver pattern as capc/capt/capm/caps validation tests: through the
    # .bat wrapper (real user entry point), stderr discarded, only the exit
    # code asserted.
    It 'rejects a non-numeric count' {
        & (Join-Path $bin 'capn.bat') abc cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects 0' {
        & (Join-Path $bin 'capn.bat') 0 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a negative count' {
        & (Join-Path $bin 'capn.bat') -1 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a missing command' {
        & (Join-Path $bin 'capn.bat') 5 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a huge 400-digit <count> cleanly with the usage error - no raw PowerShell conversion error leaked' {
        # Same TryParse rationale as capm/caps's huge-digit tests above: the
        # parse has no length limit, so an absurdly long digit string must fail
        # cleanly into the controlled usage error rather than crash with a raw
        # cast error. Whitespace-normalize before matching (Pester-console
        # word-wrap).
        $digits = '9' * 400
        $out = & (Join-Path $bin 'capn.bat') $digits cmd /c "echo hi" 2>&1
        $LASTEXITCODE | Should Be 1
        (($out | Out-String) -replace '\s+', ' ') | Should Match 'usage: capn'
        (($out | Out-String) -replace '\s+', ' ') | Should Not Match 'Cannot convert value'
    }

    It 'rejects a count above the uint32 ActiveProcessLimit boundary (<Count>)' -TestCases @(
        @{ Count = '4294967296' }
        @{ Count = '99999999999' }
    ) {
        param($Count)
        # 4294967295 (0xFFFFFFFF) is the largest value the uint32
        # ActiveProcessLimit struct field can hold; anything above it must be
        # the clean usage error, never a silently wrapped/truncated limit.
        & (Join-Path $bin 'capn.bat') $Count cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'accepts a count at the uint32 boundary and propagates the wrapped exit code' {
        # 4294967295 is the uint32 maximum - a valid ActiveProcessLimit. The
        # child exits instantly, so the huge ceiling never delays anything.
        & (Join-Path $bin 'capn.bat') 4294967295 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
    }
}

Describe 'capn.ps1 behavior' {
    It 'propagates the wrapped exit code when the command stays under the limit' {
        & (Join-Path $bin 'capn.bat') 10 cmd /c "exit 7"
        $LASTEXITCODE | Should Be 7
    }

    It 'runs a wrapped command with 2 children normally under a limit of 3 (nothing is refused inside the budget)' {
        # Wrapped parent + 2 simultaneously-alive children = exactly 3 active
        # processes against a ceiling of 3, so every spawn must succeed. The
        # children self-terminate on a bounded 10s sleep, and the finally kills
        # them, so a failure path can't leak live processes.
        $out = New-TempFile
        $script = @'
$ErrorActionPreference = 'Stop'
$p1 = Start-Process powershell -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 10') -WindowStyle Hidden -PassThru
$p2 = Start-Process powershell -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 10') -WindowStyle Hidden -PassThru
try {{
    Start-Sleep -Seconds 2
    $a1 = $null -ne (Get-Process -Id $p1.Id -ErrorAction SilentlyContinue)
    $a2 = $null -ne (Get-Process -Id $p2.Id -ErrorAction SilentlyContinue)
    Set-Content -Path '{0}' -Value "CHILD1-ALIVE=$a1 CHILD2-ALIVE=$a2"
}} finally {{
    Stop-Process -Id $p1.Id, $p2.Id -Force -ErrorAction SilentlyContinue
}}
exit 0
'@ -f $out
        $scriptFile = New-TempScript
        Set-Content -Path $scriptFile -Value $script
        & (Join-Path $bin 'capn.bat') 3 powershell -NoProfile -File $scriptFile
        $LASTEXITCODE | Should Be 0
        (Get-Content $out).Trim() | Should Be 'CHILD1-ALIVE=True CHILD2-ALIVE=True'
        Remove-Item $out, $scriptFile -ErrorAction SilentlyContinue
    }

    It 'refuses an over-limit child spawn without killing the wrapped process (limit 1: the wrapped process itself fills the job)' {
        # The count includes the directly wrapped process: it is assigned to
        # the still-empty job before it can spawn anything, so at limit 1 the
        # wrapped command runs but its very first child-spawn attempt fails.
        # The marker file is written AFTER the failed spawn attempt and proves
        # the wrapped process itself was not killed - only the excess spawn
        # was refused (confirmed empirically before writing this test: the
        # spawn surfaces "Not enough quota is available to process this
        # command." and the parent goes on running).
        $out = New-TempFile
        $script = @'
$ErrorActionPreference = 'Stop'
$spawnFailed = $false
try {{
    Start-Process powershell -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 10') -WindowStyle Hidden | Out-Null
}} catch {{
    $spawnFailed = $true
}}
# Written after the failed spawn: proves the wrapped process kept running -
# the limit refused the child, it did not kill the parent.
Set-Content -Path '{0}' -Value "SPAWN-FAILED=$spawnFailed PARENT-STILL-RUNNING=$PID"
exit 0
'@ -f $out
        $scriptFile = New-TempScript
        Set-Content -Path $scriptFile -Value $script
        & (Join-Path $bin 'capn.bat') 1 powershell -NoProfile -File $scriptFile
        $LASTEXITCODE | Should Be 0
        (Get-Content $out).Trim() | Should Match 'SPAWN-FAILED=True PARENT-STILL-RUNNING=\d+'
        Remove-Item $out, $scriptFile -ErrorAction SilentlyContinue
    }

    It 'terminates the wrapped child and its grandchild when the capn launcher itself is taskkilled /F without /T (KILL_ON_JOB_CLOSE cascade)' {
        # Same taskkill-without-/T pattern as the capc-based cascade test
        # above, proven explicitly for this launcher too: capn's job carries
        # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE like every Job-Object launcher
        # here, so a non-cooperatively killed capn wrapper must take the whole
        # spawned tree down with it - the limit flag is not what does this,
        # the KILL_ON_JOB_CLOSE flag is, and it must be there independently.
        $marker = 'win-nice-cascade-' + [guid]::NewGuid().ToString('N')
        $childPidFile = New-TempFile
        $grandchildPidFile = New-TempFile
        # The grandchild script's own path carries the unique marker, it writes
        # its PID, and it self-terminates on a bounded deadline - so a failure
        # path can never leak a live process even if an assertion below aborts.
        $grandchildFile = Join-Path $script:testRoot ($marker + '.ps1')
        Set-Content -Path $grandchildFile -Value @"
`$deadline = [DateTime]::UtcNow.AddSeconds(60)
Set-Content -Path '$grandchildPidFile' -Value `$PID
while ([DateTime]::UtcNow -lt `$deadline) { Start-Sleep -Milliseconds 250 }
"@
        # Same rule as every other multi-hop test in this file: nested scripts go
        # in temp FILES, never inline -Command strings (triple-nested quoting).
        # The child writes its PID, spawns the grandchild, then sleeps - long
        # enough to still be alive when the test kills the launcher mid-flight,
        # bounded so a failure path can't wedge the suite.
        $outerFile = New-TempScript
        Set-Content -Path $outerFile -Value @"
Set-Content -Path '$childPidFile' -Value `$PID
Start-Process powershell -ArgumentList @('-NoProfile', '-File', '$grandchildFile') -WindowStyle Hidden | Out-Null
Start-Sleep -Seconds 60
"@
        $launcher = Start-Process powershell -ArgumentList @('-NoProfile', '-File', (Join-Path $bin 'capn.ps1'), '10', 'powershell', '-NoProfile', '-File', $outerFile) -WindowStyle Hidden -PassThru
        $childPid = 0
        $grandchildPid = 0
        try {
            # Bounded wait until BOTH the child and its grandchild are confirmed
            # running - killing the launcher only proves anything once the
            # grandchild is really inside the job.
            $deadline = [DateTime]::UtcNow.AddSeconds(15)
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($childPid -eq 0 -and (Test-Path $childPidFile)) {
                    $childPid = [int](Get-Content $childPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $grandchildPid -eq 0 -and (Test-Path $grandchildPidFile)) {
                    $grandchildPid = [int](Get-Content $grandchildPidFile | Select-Object -First 1)
                }
                if ($childPid -ne 0 -and $grandchildPid -ne 0) { break }
                Start-Sleep -Milliseconds 100
            }
            $childPid | Should Not Be 0
            $grandchildPid | Should Not Be 0
            (Get-Process -Id $childPid -ErrorAction SilentlyContinue) | Should Not Be $null
            (Get-Process -Id $grandchildPid -ErrorAction SilentlyContinue) | Should Not Be $null

            # The bug scenario itself: /F = hard kill (no in-process cleanup can
            # run), and deliberately NO /T - any tree-wide cleanup must come from
            # the job's kill-on-close cascade, not from taskkill itself.
            & taskkill /F /PID $launcher.Id | Out-Null
            $LASTEXITCODE | Should Be 0
            (Wait-ProbeChildGone -ProcessId $launcher.Id -TimeoutMs 10000) | Should Be $true

            # Cascade assertion, bounded poll (job termination completes
            # asynchronously - an instant check is a documented race here).
            (Wait-ProbeChildGone -ProcessId $grandchildPid -TimeoutMs 15000) | Should Be $true
            (Wait-ProbeChildGone -ProcessId $childPid -TimeoutMs 15000) | Should Be $true
        } finally {
            # Best-effort cleanup on every path - a failed assertion above must
            # not leave the launcher, child, or grandchild running (the two
            # generated scripts also self-terminate within 60s as a backstop).
            if ($launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
            if ($childPid -gt 0) { Remove-ProbeChild -ProcessId $childPid }
            if ($grandchildPid -gt 0) { Remove-ProbeChild -ProcessId $grandchildPid }
            Remove-Item $childPidFile, $grandchildPidFile, $grandchildFile, $outerFile -ErrorAction SilentlyContinue
        }
    }
}

Describe 'sequential invocation in one PowerShell session' {
    It 'runs idle/belownormal/abovenormal/high/realtime/capc/capt/capm/caps/capn/cy/cx/admin one after another without an Add-Type type-collision error' {
        # Regression test: bare-name resolution (idle args..., not idle.bat) runs the
        # .ps1 in the CURRENT process/AppDomain, not a new one - each of these used to
        # Add-Type an identically-named "Launcher" class, so calling a second one in the
        # same session threw "Cannot add type. The type name 'Launcher' already exists."
        # (and, for cy/cx's different Run() signature, could fail outright). Confirmed
        # empirically before the fix; each now has its own unique class name
        # (IdleLauncher, BelowNormalLauncher, ..., CapcLauncher, CaptLauncher, CapmLauncher,
        # CapsLauncher, CapnLauncher, CyLauncher, CxLauncher, AdminLauncher).
        $out = New-TempFile
        $probe = 'Set-Content -Path $env:WIN_NICE_TEST_OUT -Value "ok"'
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $fakeDir = Join-Path $script:testRoot ("win-nice-fakebin-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $fakeDir | Out-Null
        Set-Content -Path (Join-Path $fakeDir 'claude.bat') -Value "@echo off`r`nexit /b 0`r`n"
        Set-Content -Path (Join-Path $fakeDir 'codex.bat') -Value "@echo off`r`nexit /b 0`r`n"

        # REPLACE, not prepend - same PATH-isolation rule as Test-FakeLauncher above.
        $script = @"
`$env:WIN_NICE_TEST_OUT = '$out'
`$env:PATH = '$fakeDir;$env:SystemRoot\System32;$env:SystemRoot\System32\WindowsPowerShell\v1.0'
foreach (`$name in @('idle', 'belownormal', 'abovenormal', 'high', 'realtime')) {
    & (Join-Path '$bin' "`$name.ps1") powershell -NoProfile -File '$probeFile'
    if (`$LASTEXITCODE -ne 0) { throw "`$name failed with exit `$LASTEXITCODE" }
}
& (Join-Path '$bin' 'capc.ps1') 50 powershell -NoProfile -File '$probeFile'
if (`$LASTEXITCODE -ne 0) { throw "capc failed with exit `$LASTEXITCODE" }
& (Join-Path '$bin' 'capt.ps1') 1 powershell -NoProfile -File '$probeFile'
if (`$LASTEXITCODE -ne 0) { throw "capt failed with exit `$LASTEXITCODE" }
& (Join-Path '$bin' 'capm.ps1') 90 powershell -NoProfile -File '$probeFile'
if (`$LASTEXITCODE -ne 0) { throw "capm failed with exit `$LASTEXITCODE" }
& (Join-Path '$bin' 'caps.ps1') 30 powershell -NoProfile -File '$probeFile'
if (`$LASTEXITCODE -ne 0) { throw "caps failed with exit `$LASTEXITCODE" }
& (Join-Path '$bin' 'capn.ps1') 10 powershell -NoProfile -File '$probeFile'
if (`$LASTEXITCODE -ne 0) { throw "capn failed with exit `$LASTEXITCODE" }
& (Join-Path '$bin' 'cy.ps1')
if (`$LASTEXITCODE -ne 0) { throw "cy failed with exit `$LASTEXITCODE" }
& (Join-Path '$bin' 'cx.ps1')
if (`$LASTEXITCODE -ne 0) { throw "cx failed with exit `$LASTEXITCODE" }
# admin.ps1's own exit code depends on elevation state (refuses via the "%"
# check when not elevated, runs "cmd" inline when already elevated) - only the
# Add-Type collision is under test here, not admin's success/failure semantics.
& (Join-Path '$bin' 'admin.ps1') cmd /c 'echo 100%OFF' 2>`$null
exit 0
"@
        $sessionScript = New-TempScript
        Set-Content -Path $sessionScript -Value $script
        $errorOutput = & powershell -NoProfile -File $sessionScript 2>&1
        $LASTEXITCODE | Should Be 0
        ($errorOutput -join "`n") | Should Not Match 'already exists'

        Remove-Item $out, $probeFile, $sessionScript -ErrorAction SilentlyContinue
        Remove-Item $fakeDir -Recurse -ErrorAction SilentlyContinue
    }
}

Describe 'uiup.ps1' {
    It 'parses without syntax errors' {
        $parseErrors = $null
        [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $bin 'uiup.ps1'), [ref]$null, [ref]$parseErrors) | Out-Null
        $parseErrors.Count | Should Be 0
    }

    It 'accepts the -SelfElevated switch without error (syntax/param check only - does not elevate)' {
        { Get-Command (Join-Path $bin 'uiup.ps1') -ErrorAction Stop } | Should Not Throw
    }
}

# test/run-elevated.ps1 is the single-UAC entry point for the 3 admin.ps1
# already-elevated cases above. It calls Start-Process -Verb RunAs the same way
# bin/uiup.ps1 does when not already elevated - a real interactive UAC prompt -
# so only its syntax/parameter shape is checked here, never its elevation path.
Describe 'test/run-elevated.ps1' {
    It 'parses without syntax errors' {
        $parseErrors = $null
        [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'run-elevated.ps1'), [ref]$null, [ref]$parseErrors) | Out-Null
        $parseErrors.Count | Should Be 0
    }

    It 'accepts the -SelfElevated and -LogPath parameters without error (syntax/param check only - does not elevate)' {
        { Get-Command (Join-Path $PSScriptRoot 'run-elevated.ps1') -ErrorAction Stop } | Should Not Throw
    }

    It 'refuses -SelfElevated without -LogPath (internal flag - not meant to be passed by hand)' {
        $errOut = & powershell -NoProfile -File (Join-Path $PSScriptRoot 'run-elevated.ps1') -SelfElevated 2>&1
        $LASTEXITCODE | Should Be 1
        # PowerShell's default error-view word-wraps Write-Error text to the
        # console width, which can split "requires -LogPath" across a line
        # break - collapse whitespace before matching (same pattern as the
        # sibling test below and the "%" fail-closed assertion above).
        (($errOut -join ' ') -replace '\s+', ' ') | Should Match 'requires -LogPath'
    }

    # The regression this guards: -SelfElevated used to be trusted at face value,
    # so calling this script directly with -SelfElevated -LogPath <file> from a
    # plain non-admin console ran the NORMAL (non-elevated) suite and exited 0 -
    # a false "elevated coverage" result that silently skipped the 3 admin.ps1
    # already-elevated cases. Only meaningful to check from a non-elevated
    # runner - under an already-elevated runner, -SelfElevated is legitimately
    # honored and this would instead recurse into a real (nested) Pester run.
    It 'refuses to run the suite when -SelfElevated is passed but the process is not actually elevated (guards a false elevated-coverage result)' -Skip:$script:isAdminRunner {
        $log = New-TempFile
        $errOut = & powershell -NoProfile -File (Join-Path $PSScriptRoot 'run-elevated.ps1') -SelfElevated -LogPath $log 2>&1
        $LASTEXITCODE | Should Be 1
        # PowerShell's default error-view word-wraps Write-Error text to the
        # console width, which can split "not actually elevated" across a
        # line break - collapse whitespace before matching (same pattern as
        # the "%" fail-closed assertion above).
        (($errOut -join ' ') -replace '\s+', ' ') | Should Match 'not actually elevated'
        # The suite must never have actually run - no transcript was started.
        (Test-Path $log) | Should Be $false
        Remove-Item $log -ErrorAction SilentlyContinue
    }
}

# Final best-effort cleanup: most tests above Remove-Item their own temp files only
# on the success path, so a failed `Should` assertion (a terminating error in
# Pester) skips that cleanup and leaks the artifact. Everything lands inside
# $script:testRoot, so removing exactly that one directory - this run's own root,
# never a wildcard over the shared %TEMP% - catches every leak without touching a
# concurrent run's artifacts. Pester 3's Describe blocks run inline as this file
# executes top to bottom, so this statement runs after every Describe above has
# finished.
Remove-Item $script:testRoot -Recurse -Force -ErrorAction SilentlyContinue
