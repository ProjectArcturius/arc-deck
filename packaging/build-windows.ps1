param(
    [string]$OutDir = "$PSScriptRoot/../target/dist",
    [string]$Iscc = "${env:ProgramFiles(x86)}/Inno Setup 6/ISCC.exe",
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot/..").Path
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
Push-Location $repo
try {
    if (-not $SkipBuild) {
        cargo build -p arc-deck-cli --release --locked --no-default-features
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    }
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $version = ($meta.packages | Where-Object name -eq 'arc-deck-cli').version
    $binary = Join-Path $meta.target_directory 'release/arcdeck.exe'
    $reported = & $binary --version
    if ($LASTEXITCODE -ne 0 -or $reported -ne "arcdeck $version") { throw 'binary version mismatch' }
    $stage = Join-Path $OutDir "arcdeck-$version-windows-x86_64"
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Copy-Item -LiteralPath $binary -Destination "$stage/arcdeck.exe" -Force
    foreach ($file in @('LICENSE', 'NOTICE')) { Copy-Item -LiteralPath "$repo/$file" -Destination $stage -Force }
    Copy-Item -LiteralPath "$repo/crates/arc-deck-tui/LICENSE-OpenCode" -Destination $stage -Force
    Copy-Item -LiteralPath "$repo/docs/INSTALL.md" -Destination $stage -Force
    foreach ($script in @('install.ps1','install.cmd','uninstall.ps1','uninstall.cmd')) {
        Copy-Item -LiteralPath "$PSScriptRoot/windows/$script" -Destination $stage -Force
    }
    $commit = git rev-parse HEAD
    $dirty = [bool](git status --porcelain)
    $hostTarget = ((rustc -vV | Select-String '^host: ').ToString() -replace '^host: ', '')
    @{ version = $version; commit = $commit; dirty = $dirty; target = $hostTarget; built_at = [DateTime]::UtcNow.ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath "$stage/BUILD-INFO.json" -Encoding UTF8
    Compress-Archive -Path "$stage/*" -DestinationPath "$OutDir/arcdeck-$version-windows-x86_64.zip" -Force
    if (-not (Test-Path -LiteralPath $Iscc)) { throw "Inno Setup 6 compiler missing: $Iscc" }
    & $Iscc "/DSourceDir=$stage" "/DOutputDir=$OutDir" "/DVersion=$version" "$PSScriptRoot/windows/arcdeck.iss"
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
    Get-ChildItem -LiteralPath $OutDir -File | Where-Object { $_.Name -match '\.(exe|zip)$' } |
        ForEach-Object { $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256; "$($hash.Hash.ToLower())  $($_.Name)" } |
        Set-Content -LiteralPath "$OutDir/SHA256SUMS-windows.txt" -Encoding ASCII
} finally { Pop-Location }
