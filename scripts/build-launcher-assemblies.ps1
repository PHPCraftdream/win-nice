param(
    # Rebuild every assembly regardless of sidecar hash state.
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repoRoot 'bin'

# Canonical script<->class list also lives in install/install.js
# (HELPER_ASSEMBLIES) - test/assembly-list-sync.test.js asserts this table
# matches that export exactly.
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

# The legacy .NET Framework csc.exe (4.8.4084, "C# 5") rejects /deterministic
# (CS2007), so the same source compiles to different bytes every run (MVID +
# PE timestamp). Rather than chase determinism, make freshness verifiable
# without a compiler: hash the extracted C# source (scripts/check-assemblies.js
# does the same thing in Node, with no csc dependency) and skip recompiling
# when the sidecar's hash already matches. The recipe prefix ties the hash to
# the build inputs that affect output bytes beyond the source text itself -
# bump it to force every assembly to rebuild (e.g. after changing csc flags).
$buildRecipe = "win-nice-helper-v1|csc4|/target:library|/reference:System.dll`n"

function Get-SourceHash([string]$sourceText) {
    $normalized = $sourceText -replace "`r`n", "`n"
    $bytes = $utf8.GetBytes($buildRecipe + $normalized)
    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hashBytes = $hasher.ComputeHash($bytes)
        return -join ($hashBytes | ForEach-Object { $_.ToString('x2') })
    } finally {
        $hasher.Dispose()
    }
}

function Get-SidecarHash([string]$sidecarPath) {
    if (-not (Test-Path -LiteralPath $sidecarPath)) { return $null }
    $text = [IO.File]::ReadAllText($sidecarPath)
    $match = [regex]::Match($text, '(?m)^# source-sha256: (?<hash>[0-9a-f]{64})$')
    if ($match.Success) { return $match.Groups['hash'].Value }
    return $null
}

function Write-Sidecar([string]$sidecarPath, [string]$hash) {
    [IO.File]::WriteAllText($sidecarPath, "# win-nice: managed-file`n# source-sha256: $hash`n", $utf8)
}

foreach ($launcher in $launcherSources) {
    $scriptPath = Join-Path $bin $launcher.Script
    $sourceText = [IO.File]::ReadAllText($scriptPath)
    $match = [regex]::Match($sourceText, '(?s)\$source = @"\r?\n(?<source>.*?)\r?\n"@')
    if (-not $match.Success) { throw "Could not extract helper source from bin/$($launcher.Script)" }

    $classMatch = [regex]::Match($match.Groups['source'].Value, 'public static class (?<name>\w+)')
    if (-not $classMatch.Success -or $classMatch.Groups['name'].Value -ne $launcher.Class) {
        throw "Unexpected helper class in bin/$($launcher.Script); expected $($launcher.Class)"
    }

    $helperSource = $match.Groups['source'].Value
    $hash = Get-SourceHash $helperSource
    $outputPath = Join-Path $bin ($launcher.Class + '.dll')
    $sidecarPath = $outputPath + '.managed'

    if (-not $Force -and (Test-Path -LiteralPath $outputPath) -and (Get-SidecarHash $sidecarPath) -eq $hash) {
        Write-Host "skipped: $($launcher.Class) (up to date)"
        continue
    }

    $sourcePath = Join-Path $PSScriptRoot ($launcher.Class + '.build.cs')
    try {
        [IO.File]::WriteAllText($sourcePath, $helperSource, $utf8)
        & $compiler /nologo /target:library "/out:$outputPath" /reference:System.dll $sourcePath
        if ($LASTEXITCODE -ne 0) { throw "csc.exe failed for $($launcher.Class) with exit code $LASTEXITCODE" }
        Write-Sidecar $sidecarPath $hash
        Write-Host "built: $($launcher.Class)"
    } finally {
        Remove-Item -LiteralPath $sourcePath -Force -ErrorAction SilentlyContinue
    }
}

$notifierSource = Join-Path $PSScriptRoot 'EnvironmentNotifier.cs'
$notifierText = [IO.File]::ReadAllText($notifierSource)
$notifierHash = Get-SourceHash $notifierText
$notifierPath = Join-Path $bin 'EnvironmentNotifier.dll'
$notifierSidecar = $notifierPath + '.managed'

if (-not $Force -and (Test-Path -LiteralPath $notifierPath) -and (Get-SidecarHash $notifierSidecar) -eq $notifierHash) {
    Write-Host "skipped: EnvironmentNotifier (up to date)"
} else {
    & $compiler /nologo /target:library "/out:$notifierPath" /reference:System.dll $notifierSource
    if ($LASTEXITCODE -ne 0) { throw "csc.exe failed for EnvironmentNotifier with exit code $LASTEXITCODE" }
    Write-Sidecar $notifierSidecar $notifierHash
    Write-Host "built: EnvironmentNotifier"
}
