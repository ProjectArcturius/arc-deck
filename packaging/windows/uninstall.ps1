$ErrorActionPreference = 'Stop'
$directory = [IO.Path]::GetFullPath($PSScriptRoot)
$statePath = Join-Path $directory 'install-state.json'
$state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
if ($state.directory -ne $directory) { throw 'Installation marker does not match this directory' }
# Refuse a running/locked binary before changing PATH or removing the installation marker.
if (Test-Path -LiteralPath "$directory/saiku.exe") { Remove-Item -LiteralPath "$directory/saiku.exe" }
if ($state.added_path) {
    $current = [Environment]::GetEnvironmentVariable('Path','User')
    $restored = if ($current -eq $state.written_path) { $state.original_path } else { ($current -split ';' | Where-Object { $_ -ne $directory }) -join ';' }
    [Environment]::SetEnvironmentVariable('Path',$restored,'User')
}
# Remove only the fixed package inventory. User data and unrelated files are preserved.
foreach ($name in @('saiku.exe','BUILD-INFO.json','INSTALL.md','LICENSE','NOTICE','LICENSE-OpenCode','install.ps1','install.cmd','uninstall.ps1','uninstall.cmd','install-state.json')) {
    $path = Join-Path $directory $name
    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path }
}
if (@(Get-ChildItem -LiteralPath $directory -Force).Count -eq 0) { Remove-Item -LiteralPath $directory }
Write-Host 'Saiku removed. Configuration, credentials and sessions were preserved.'
