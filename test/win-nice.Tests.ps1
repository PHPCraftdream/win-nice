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

function New-TempFile {
    $f = Join-Path $env:TEMP ("win-nice-pester-" + [guid]::NewGuid().ToString("N") + ".tmp")
    Remove-Item $f -ErrorAction SilentlyContinue
    return $f
}

function New-TempScript {
    $f = Join-Path $env:TEMP ("win-nice-pester-" + [guid]::NewGuid().ToString("N") + ".ps1")
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
Start-Sleep -Milliseconds 500
$c.Refresh()
Set-Content -Path '{0}' -Value $c.PriorityClass
$c.WaitForExit()
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
        # idle.bat forwards a raw %* straight to "start"; unlike cap.bat/cap.ps1 it
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
        # Same cmd.exe batch-parameter quirk as cap.bat - see the cap.ps1 test of the
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

# realtime is downgraded to High without SeIncreaseBasePriorityPrivilege (elevated
# processes have it by default) - this test runner isn't elevated, so it expects
# the same downgraded result as "high". See README for why.
$priorityTools = @(
    @{ Name = 'abovenormal'; Expected = 'AboveNormal' }
    @{ Name = 'high'; Expected = 'High' }
    @{ Name = 'realtime'; Expected = 'High' }
)

Describe 'abovenormal.bat / high.bat / realtime.bat' {
    It 'fails with a usage message when no command is given (<Name>)' -TestCases $priorityTools {
        param($Name, $Expected)
        & (Join-Path $bin "$Name.bat") 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
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
Start-Sleep -Milliseconds 500
$c.Refresh()
Set-Content -Path '{0}' -Value $c.PriorityClass
$c.WaitForExit()
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

Describe 'cap.ps1 argument validation' {
    It 'rejects a non-numeric percent' {
        & (Join-Path $bin 'cap.bat') abc cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects percent below 1' {
        & (Join-Path $bin 'cap.bat') 0 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects percent above 100' {
        & (Join-Path $bin 'cap.bat') 101 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a missing command' {
        & (Join-Path $bin 'cap.bat') 50 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'accepts the boundary values 1 and 100' {
        & (Join-Path $bin 'cap.bat') 1 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
        & (Join-Path $bin 'cap.bat') 100 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
    }
}

Describe 'cap.ps1 behavior' {
    It 'propagates the exit code of the wrapped command' {
        & (Join-Path $bin 'cap.bat') 50 cmd /c "exit 3"
        $LASTEXITCODE | Should Be 3
    }

    It 'preserves an empty-string argument through to the wrapped command' {
        # The empty "" argument is baked into a static .bat file's text rather than
        # passed as a live PowerShell argument - Windows PowerShell 5.1's `&` drops
        # literal "" arguments to native commands before they ever reach cap.bat,
        # which would test a PowerShell quirk instead of cap.ps1's own quoting.
        # The probe itself must bind its args via ValueFromRemainingArguments - a
        # plain [string[]] positional parameter has its own PS 5.1 -File quirk that
        # silently truncates the array at an empty element, independent of cap.ps1.
        $out = New-TempFile
        $probe = @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$a)
Set-Content -Path '{0}' -Value ($a.Count.ToString() + "|" + ($a -join ","))
'@ -f $out
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $capBat = Join-Path $bin 'cap.bat'
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`n`"$capBat`" 50 powershell -NoProfile -File `"$probeFile`" AAA `"`" BBB`r`n"
        & $driver
        (Get-Content $out).Trim() | Should Be '3|AAA,,BBB'
        Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    }

    It 'preserves cmd.exe metacharacters (&, |, <, >, ^) as literal argument text' {
        # Unescaped, these would be re-parsed by the "cmd.exe /c" hop inside cap.ps1
        # and split the wrapped command into separate commands (or drop the caret).
        $out = New-TempFile
        $probe = @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$a)
Set-Content -Path '{0}' -Value ($a -join '|SEP|')
'@ -f $out
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $capBat = Join-Path $bin 'cap.bat'
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`n`"$capBat`" 50 powershell -NoProfile -File `"$probeFile`" `"A&B`" `"A|B`" `"A<B>C`" `"A^B`"`r`n"
        & $driver
        $LASTEXITCODE | Should Be 0
        (Get-Content $out).Trim() | Should Be 'A&B|SEP|A|B|SEP|A<B>C|SEP|A^B'
        Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    }

    It 'forwards flags that collide with PowerShell common parameters (e.g. -e, -Verbose) untouched' {
        # cap.ps1 must not bind these as -ErrorAction/-Verbose itself; node -e is the
        # motivating real-world case. The probe below deliberately uses bare $args
        # (no [Parameter()] attribute) for the same reason cap.ps1 does - a declared
        # ValueFromRemainingArguments parameter would make the *probe* itself subject
        # to the same common-parameter ambiguity being tested here.
        $out = New-TempFile
        $probe = @'
Set-Content -Path $env:WIN_NICE_TEST_OUT -Value ($args -join '|SEP|')
'@
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $capBat = Join-Path $bin 'cap.bat'
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`nset WIN_NICE_TEST_OUT=$out`r`n`"$capBat`" 50 powershell -NoProfile -File `"$probeFile`" -e 0 -Verbose`r`n"
        & $driver
        $LASTEXITCODE | Should Be 0
        (Get-Content $out).Trim() | Should Be '-e|SEP|0|SEP|-Verbose'
        Remove-Item $out, $probeFile, $driver -ErrorAction SilentlyContinue
    }

    It 'forwards a flag that would ambiguously prefix-match the declared -Percent parameter name (e.g. -p)' {
        # Regression test: cap.ps1 used to declare $Percent via param(), and even
        # without [Parameter()] attributes, PowerShell's binder still prefix-matches
        # "-p" against a declared parameter named "Percent" and rebinds it.
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'cap.bat') -Prefix @('50') -ProbeArgs @('-p', '0')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '-p|SEP|0'
    }

    It 'preserves a literal "%" when the target is a directly-launchable .exe' {
        # No cmd.exe involved at all on this path (see cap.ps1's Capper.Run) - unlike
        # the cmd.exe /c fallback path, "%" isn't at risk of environment-variable
        # expansion here. Must invoke cap.ps1 directly (not through cap.bat, which has
        # its own separate, documented "%" corruption at the %* forwarding step).
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'cap.ps1') -Prefix @('50') -ProbeArgs @('100%OFF', '50%50')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '100%OFF|SEP|50%50'
    }

    It 'still preserves cmd.exe metacharacters when the target is a .bat file (fallback path)' {
        $targetBat = New-TempScript
        $targetBat = $targetBat.Replace('.ps1', '.bat')
        Set-Content -Path $targetBat -Value "@echo off`r`necho BATOUT=%*`r`n"
        $driver = (New-TempScript).Replace('.ps1', '.bat')
        Set-Content -Path $driver -Value "@echo off`r`n`"$(Join-Path $bin 'cap.bat')`" 50 `"$targetBat`" `"A&B`" `"A|B`"`r`n"
        $out = & $driver
        $LASTEXITCODE | Should Be 0
        ($out | Select-Object -Last 1) | Should Be 'BATOUT="A&B" "A|B"'
        Remove-Item $targetBat, $driver -ErrorAction SilentlyContinue
    }

    It 'documents a known limitation: a literal "%" is corrupted by cap.bat itself, before cap.ps1 ever runs' {
        # Confirmed with nothing more than a bare "echo %1" in a plain .bat file - this
        # is cmd.exe's own batch-parameter substitution rescanning %1/%* for %...%
        # patterns, unrelated to cap.ps1's escaping and not fixable from inside a .bat.
        # Invoking "cap" bare from an actual PowerShell session (cap.ps1 preferred over
        # cap.bat) does not have this problem - see the "%" test above.
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'cap.bat') -Prefix @('50') -ProbeArgs @('100%OFF')
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
        $passed = $false
        $lastBaseline = $null
        $lastCapped = $null
        for ($attempt = 1; $attempt -le 3 -and -not $passed; $attempt++) {
            $baseline = [double](powershell -NoProfile -File $burnFile $threads $seconds)
            $cappedOut = & (Join-Path $bin 'cap.bat') $cap powershell -NoProfile -File $burnFile $threads $seconds
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

Describe 'pint.ps1 argument validation' {
    It 'rejects a non-numeric thread count' {
        & (Join-Path $bin 'pint.bat') abc cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a count below 1' {
        & (Join-Path $bin 'pint.bat') 0 cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a count above the logical processor count' {
        $tooMany = [Environment]::ProcessorCount + 1
        & (Join-Path $bin 'pint.bat') $tooMany cmd /c "echo hi" 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'rejects a missing command' {
        & (Join-Path $bin 'pint.bat') 1 2>&1 | Out-Null
        $LASTEXITCODE | Should Be 1
    }

    It 'accepts the boundary value 1' {
        & (Join-Path $bin 'pint.bat') 1 cmd /c "exit 0"
        $LASTEXITCODE | Should Be 0
    }
}

Describe 'pint.ps1 behavior' {
    It 'propagates the exit code of the wrapped command' {
        & (Join-Path $bin 'pint.bat') 2 cmd /c "exit 3"
        $LASTEXITCODE | Should Be 3
    }

    It 'forwards a flag that would ambiguously prefix-match the declared -Count parameter name (e.g. -c)' {
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'pint.bat') -Prefix @('2') -ProbeArgs @('-c', '0')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '-c|SEP|0'
    }

    It 'preserves cmd.exe metacharacters and a literal "%" on the direct-launch path' {
        # Must invoke pint.ps1 directly (not through pint.bat, which has its own
        # separate, documented "%" corruption at the %* forwarding step).
        $r = Get-DirectForwardedArgs -Ps1 (Join-Path $bin 'pint.ps1') -Prefix @('2') -ProbeArgs @('A&B', 'A|B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|A|B|SEP|100%OFF'
    }

    It 'pins the process to exactly the first N logical processors' {
        $out = New-TempFile
        $probe = "Set-Content -Path '$out' -Value ('0x' + (Get-Process -Id `$PID).ProcessorAffinity.ToString('X'))"
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        & (Join-Path $bin 'pint.bat') 3 powershell -NoProfile -File $probeFile
        (Get-Content $out).Trim() | Should Be '0x7'
        Remove-Item $out, $probeFile -ErrorAction SilentlyContinue
    }

    It 'pins the whole spawned process tree, not just the immediate child' {
        $out = New-TempFile
        $script = @'
$c = Start-Process cmd -ArgumentList "/c ping -n 3 127.0.0.1 >nul" -WindowStyle Hidden -PassThru
Start-Sleep -Milliseconds 500
$c.Refresh()
Set-Content -Path '{0}' -Value ('0x' + $c.ProcessorAffinity.ToString('X'))
$c.WaitForExit()
'@ -f $out
        $scriptFile = New-TempScript
        Set-Content -Path $scriptFile -Value $script
        & (Join-Path $bin 'pint.bat') 3 powershell -NoProfile -File $scriptFile
        (Get-Content $out).Trim() | Should Be '0x7'
        Remove-Item $out, $scriptFile -ErrorAction SilentlyContinue
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
        $r = Get-ForwardedArgs -Exe (Join-Path $bin 'admin.bat') -ProbeArgs @('A&B', 'A|B', '100%OFF')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be 'A&B|SEP|A|B|SEP|100%OFF'
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
    $fakeDir = Join-Path $env:TEMP ("win-nice-fakebin-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $fakeDir | Out-Null
    $out = Join-Path $fakeDir 'out.txt'
    $fakeTarget = Join-Path $fakeDir $FakeTargetName
    Set-Content -Path $fakeTarget -Value "@echo off`r`n(echo %*)>`"$out`"`r`n"
    $prevPath = $env:PATH
    $env:PATH = "$fakeDir;$env:PATH"
    try {
        & powershell -NoProfile -File $Ps1 @ExtraArgs | Out-Null
        $exitCode = $LASTEXITCODE
    } finally {
        $env:PATH = $prevPath
    }
    $result = if (Test-Path $out) { (Get-Content $out).Trim() } else { $null }
    Remove-Item $fakeDir -Recurse -ErrorAction SilentlyContinue
    return [PSCustomObject]@{ Output = $result; ExitCode = $exitCode }
}

Describe 'cy.ps1' {
    It 'prepends --dangerously-skip-permissions and forwards the rest, protecting metacharacters' {
        $r = Test-FakeLauncher -Ps1 (Join-Path $bin 'cy.ps1') -FakeTargetName 'claude.bat' -ExtraArgs @('-p', 'A&B')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '--dangerously-skip-permissions -p "A&B"'
    }
}

Describe 'cx.ps1' {
    It 'prepends --dangerously-bypass-approvals-and-sandbox and forwards the rest, protecting metacharacters' {
        $r = Test-FakeLauncher -Ps1 (Join-Path $bin 'cx.ps1') -FakeTargetName 'codex.bat' -ExtraArgs @('-p', 'A&B')
        $r.ExitCode | Should Be 0
        $r.Output | Should Be '--dangerously-bypass-approvals-and-sandbox -p "A&B"'
    }
}

Describe 'sequential invocation in one PowerShell session' {
    It 'runs idle/belownormal/abovenormal/high/realtime/cy/cx one after another without an Add-Type type-collision error' {
        # Regression test: bare-name resolution (idle args..., not idle.bat) runs the
        # .ps1 in the CURRENT process/AppDomain, not a new one - each of these used to
        # Add-Type an identically-named "Launcher" class, so calling a second one in the
        # same session threw "Cannot add type. The type name 'Launcher' already exists."
        # (and, for cy/cx's different Run() signature, could fail outright). Confirmed
        # empirically before the fix; each now has its own unique class name.
        $out = New-TempFile
        $probe = 'Set-Content -Path $env:WIN_NICE_TEST_OUT -Value "ok"'
        $probeFile = New-TempScript
        Set-Content -Path $probeFile -Value $probe
        $fakeDir = Join-Path $env:TEMP ("win-nice-fakebin-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $fakeDir | Out-Null
        Set-Content -Path (Join-Path $fakeDir 'claude.bat') -Value "@echo off`r`nexit /b 0`r`n"
        Set-Content -Path (Join-Path $fakeDir 'codex.bat') -Value "@echo off`r`nexit /b 0`r`n"

        $script = @"
`$env:WIN_NICE_TEST_OUT = '$out'
`$env:PATH = '$fakeDir;' + `$env:PATH
foreach (`$name in @('idle', 'belownormal', 'abovenormal', 'high', 'realtime')) {
    & (Join-Path '$bin' "`$name.ps1") powershell -NoProfile -File '$probeFile'
    if (`$LASTEXITCODE -ne 0) { throw "`$name failed with exit `$LASTEXITCODE" }
}
& (Join-Path '$bin' 'cy.ps1')
if (`$LASTEXITCODE -ne 0) { throw "cy failed with exit `$LASTEXITCODE" }
& (Join-Path '$bin' 'cx.ps1')
if (`$LASTEXITCODE -ne 0) { throw "cx failed with exit `$LASTEXITCODE" }
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
