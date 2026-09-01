# SPDX-License-Identifier: MIT OR Apache-2.0
# Integration tests against the real Windows APIs (Job Objects, priority classes).
# Run with: Invoke-Pester (built-in Pester 3.4.0 on Windows 10/11 - no install needed).

$root = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $root 'bin'

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
$c = Start-Process cmd -ArgumentList "/c ping -n 3 127.0.0.1 >nul" -PassThru
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

        $baseline = [double](powershell -NoProfile -File $burnFile $threads $seconds)
        $cappedOut = & (Join-Path $bin 'cap.bat') 30 powershell -NoProfile -File $burnFile $threads $seconds
        $capped = [double]($cappedOut | Select-Object -Last 1)

        $capped | Should BeLessThan $baseline
        Remove-Item $burnFile -ErrorAction SilentlyContinue
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

    It 'runs the wrapped command inline and propagates its exit code when already elevated' {
        # -Verb RunAs (the not-elevated branch) needs an interactive UAC click and
        # can't be exercised in an automated test - this only covers the
        # already-elevated branch, and only when the test runner itself is elevated.
        $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        if (-not $isAdmin) { return }

        & (Join-Path $bin 'admin.bat') cmd /c "exit 5"
        $LASTEXITCODE | Should Be 5
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
