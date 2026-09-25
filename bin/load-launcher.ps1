# win-nice: managed-file
$script:WinNiceLauncherDirectory = $PSScriptRoot

function Import-WinNiceLauncherAssembly {
    param([Parameter(Mandatory = $true)][string]$Name)

    $path = Join-Path $script:WinNiceLauncherDirectory ($Name + '.dll')
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "win-nice: missing prebuilt helper assembly: $path"
    }

    foreach ($assembly in [AppDomain]::CurrentDomain.GetAssemblies()) {
        if ($assembly.GetName().Name -eq $Name) { return }
    }

    [void][Reflection.Assembly]::Load([IO.File]::ReadAllBytes($path))
}
