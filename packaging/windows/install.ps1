param([string]$InstallDir = "$env:LOCALAPPDATA\Programs\Arc Deck")
$ErrorActionPreference = 'Stop'
$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$source = [IO.Path]::GetFullPath($PSScriptRoot)
if ($InstallDir -eq $source) { throw 'Extract the portable archive elsewhere before installing' }
if (-not (Test-Path -LiteralPath "$source/arcdeck.exe")) { throw 'arcdeck.exe must be next to this script' }
$files = @('arcdeck.exe','BUILD-INFO.json','INSTALL.md','LICENSE','NOTICE','LICENSE-OpenCode','install.ps1','install.cmd','uninstall.ps1','uninstall.cmd')
$statePath = Join-Path $InstallDir 'install-state.json'
$oldState = if (Test-Path -LiteralPath $statePath) { Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json } else { $null }
if ((Test-Path -LiteralPath "$InstallDir/arcdeck.exe") -and -not $oldState) { throw 'An existing installation is present; use its original upgrade method' }
$original = [Environment]::GetEnvironmentVariable('Path','User')
$added = $false
$written = $original
if (($original -split ';') -notcontains $InstallDir) {
    # An older cargo-installed arcdeck may already be on PATH. The installed version must win.
    $written = if ([string]::IsNullOrEmpty($original)) { $InstallDir } else { $InstallDir + ';' + $original }
    $added = $true
} elseif ($oldState) {
    $added = $oldState.added_path
    if ($added) {
        # Preserve PATH edits made between installs; the old snapshot is valid only while
        # the current value still equals what that installation wrote.
        $original = if ($original -eq $oldState.written_path) { $oldState.original_path } else { ($original -split ';' | Where-Object { $_ -ne $InstallDir }) -join ';' }
        $written = if ([string]::IsNullOrEmpty($original)) { $InstallDir } else { $InstallDir + ';' + $original }
    }
}
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
foreach ($file in $files) { Copy-Item -LiteralPath (Join-Path $source $file) -Destination (Join-Path $InstallDir $file) -Force }
@{ directory = $InstallDir; added_path = $added; original_path = $original; written_path = $written; files = $files } |
    ConvertTo-Json | Set-Content -LiteralPath $statePath -Encoding UTF8
if ($added) { [Environment]::SetEnvironmentVariable('Path',$written,'User') }
Write-Host "Installed to $InstallDir. Open a new terminal and run arcdeck --version."
