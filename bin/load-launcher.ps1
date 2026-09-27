# win-nice: managed-file
$script:WinNiceLauncherDirectory = $PSScriptRoot

# SHA-256 of a file's current bytes, as a lowercase hex string - used to detect
# an on-disk DLL that changed after this session already loaded it (Assembly.Load
# copies the bytes into the AppDomain; it never re-reads or locks the file, and an
# already-loaded assembly can't be unloaded, so a changed file on disk cannot take
# effect until a new PowerShell process starts).
function Get-WinNiceBytesSha256Hex {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        $hashBytes = $sha256.ComputeHash($Bytes)
    } finally {
        $sha256.Dispose()
    }
    return -join ($hashBytes | ForEach-Object { $_.ToString('x2') })
}

function Import-WinNiceLauncherAssembly {
    param([Parameter(Mandatory = $true)][string]$Name)

    $path = Join-Path $script:WinNiceLauncherDirectory ($Name + '.dll')
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "win-nice: missing prebuilt helper assembly: $path"
    }

    $hashDataKey = 'win-nice:sha256:' + $Name
    $warnedDataKey = 'win-nice:warned:' + $Name

    foreach ($assembly in [AppDomain]::CurrentDomain.GetAssemblies()) {
        if ($assembly.GetName().Name -ne $Name) { continue }

        # Recorded only when this loader itself loaded $Name earlier in this
        # session (see the load path below). No record means some other code
        # path loaded an assembly with this simple name - unchanged behavior:
        # trust it and return, same as before this hash check existed.
        $recordedHash = [AppDomain]::CurrentDomain.GetData($hashDataKey)
        if ($null -eq $recordedHash) { return }

        if ((Get-WinNiceBytesSha256Hex -Bytes ([IO.File]::ReadAllBytes($path))) -eq $recordedHash) { return }

        # File changed since this session loaded it (a probable win-nice
        # upgrade) - warn once per assembly per session, then keep running
        # the already-loaded version; it can't be swapped out without a new
        # PowerShell process.
        if (-not [AppDomain]::CurrentDomain.GetData($warnedDataKey)) {
            Write-Warning ("win-nice: $Name.dll changed on disk since it was loaded in this " +
                "PowerShell session (win-nice was probably upgraded); the previously loaded " +
                "version stays active until you start a new PowerShell session.")
            [AppDomain]::CurrentDomain.SetData($warnedDataKey, $true)
        }
        return
    }

    $bytes = [IO.File]::ReadAllBytes($path)
    [void][Reflection.Assembly]::Load($bytes)
    [AppDomain]::CurrentDomain.SetData($hashDataKey, (Get-WinNiceBytesSha256Hex -Bytes $bytes))
}
