$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repoRoot 'bin'
$launcherSources = @(
    @{ Script = 'abovenormal.ps1'; Class = 'AboveNormalLauncher' },
    @{ Script = 'admin.ps1'; Class = 'AdminLauncher' },
    @{ Script = 'belownormal.ps1'; Class = 'BelowNormalLauncher' },
    @{ Script = 'capc.ps1'; Class = 'CapcLauncher' },
    @{ Script = 'capm.ps1'; Class = 'CapmLauncher' },
    @{ Script = 'capn.ps1'; Class = 'CapnLauncher' },
    @{ Script = 'caps.ps1'; Class = 'CapsLauncher' },
    @{ Script = 'capt.ps1'; Class = 'CaptLauncher' },
    @{ Script = 'cx.ps1'; Class = 'CxLauncher' },
    @{ Script = 'cy.ps1'; Class = 'CyLauncher' },
    @{ Script = 'high.ps1'; Class = 'HighLauncher' },
    @{ Script = 'idle.ps1'; Class = 'IdleLauncher' },
    @{ Script = 'realtime.ps1'; Class = 'RealtimeLauncher' }
)
$compilerPaths = @(
    (Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'),
    (Join-Path $env:WINDIR 'Microsoft.NET\Framework\v4.0.30319\csc.exe')
)
$compiler = $compilerPaths | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $compiler) { throw 'Could not find the .NET Framework C# compiler.' }
$utf8 = [System.Text.UTF8Encoding]::new($false)

foreach ($launcher in $launcherSources) {
    $scriptPath = Join-Path $bin $launcher.Script
    $sourceText = [IO.File]::ReadAllText($scriptPath)
    $match = [regex]::Match($sourceText, '(?s)\$source = @"\r?\n(?<source>.*?)\r?\n"@')
    if (-not $match.Success) { throw "Could not extract helper source from bin/$($launcher.Script)" }

    $classMatch = [regex]::Match($match.Groups['source'].Value, 'public static class (?<name>\w+)')
    if (-not $classMatch.Success -or $classMatch.Groups['name'].Value -ne $launcher.Class) {
        throw "Unexpected helper class in bin/$($launcher.Script); expected $($launcher.Class)"
    }

    $sourcePath = Join-Path $PSScriptRoot ($launcher.Class + '.build.cs')
    $outputPath = Join-Path $bin ($launcher.Class + '.dll')
    try {
        [IO.File]::WriteAllText($sourcePath, $match.Groups['source'].Value, $utf8)
        & $compiler /nologo /target:library "/out:$outputPath" /reference:System.dll $sourcePath
        if ($LASTEXITCODE -ne 0) { throw "csc.exe failed for $($launcher.Class) with exit code $LASTEXITCODE" }
        [IO.File]::WriteAllText($outputPath + '.managed', "# win-nice: managed-file`n", $utf8)
    } finally {
        Remove-Item -LiteralPath $sourcePath -Force -ErrorAction SilentlyContinue
    }
}

$notifierSource = Join-Path $PSScriptRoot 'EnvironmentNotifier.cs'
$notifierPath = Join-Path $bin 'EnvironmentNotifier.dll'
& $compiler /nologo /target:library "/out:$notifierPath" /reference:System.dll $notifierSource
if ($LASTEXITCODE -ne 0) { throw "csc.exe failed for EnvironmentNotifier with exit code $LASTEXITCODE" }
[IO.File]::WriteAllText($notifierPath + '.managed', "# win-nice: managed-file`n", $utf8)
