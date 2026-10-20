param([string]$Dist = "$PSScriptRoot/../../target/dist")
$ErrorActionPreference = 'Stop'
$Dist = [IO.Path]::GetFullPath($Dist)
$registry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{231F89C8-49B7-47AD-8817-47C41DE88684}_is1'
if (Test-Path $registry) { throw 'Arc Deck is already installed; refusing to overwrite an existing installation in a smoke test' }
$destination = [IO.Path]::GetFullPath("$Dist/../install-smoke")
if (Test-Path $destination) { throw 'Smoke destination already exists; choose a fresh build directory' }
$beforePath = [Environment]::GetEnvironmentVariable('Path', 'User')
$testTemp = [IO.Path]::GetFullPath("$Dist/../installer-temp")
New-Item -ItemType Directory -Path $testTemp -Force | Out-Null
$setup = Join-Path $Dist 'arcdeck-1.0.0-windows-x86_64-setup.exe'
function Run-Installer([string]$File, [string[]]$Arguments) {
    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $File
    $info.Arguments = $Arguments -join ' '
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    # 受限的 CI/agent 环境不一定允许安装器在用户 TEMP 解压；测试只用工作区临时目录。
    $info.EnvironmentVariables['TEMP'] = $testTemp
    $info.EnvironmentVariables['TMP'] = $testTemp
    $child = [Diagnostics.Process]::Start($info)
    if (-not $child.WaitForExit(30000)) {
        $child.Kill()
        throw 'Installer did not exit within 30 seconds'
    }
    if ($child.ExitCode -ne 0) { throw "Installer exited $($child.ExitCode)" }
}
Run-Installer $setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/TASKS=addtopath', ('/DIR="' + $destination + '"'), ('/LOG="' + "$Dist/install-test.log" + '"'))
try {
    $reported = & "$destination/arcdeck.exe" --version
    if ($LASTEXITCODE -ne 0 -or $reported -ne 'arcdeck 1.0.0') { throw 'Installed binary is not 1.0.0' }
    if (([Environment]::GetEnvironmentVariable('Path', 'User') -split ';') -notcontains $destination) { throw 'Installer did not add user PATH' }
    if (-not (Test-Path $registry)) { throw 'Uninstall entry missing' }
    foreach ($file in @('LICENSE','NOTICE','LICENSE-OpenCode','INSTALL.md','BUILD-INFO.json')) {
        if (-not (Test-Path "$destination/$file")) { throw "Missing packaged file: $file" }
    }
    'Installed version, user PATH, uninstall entry and notices verified'
} finally {
    Run-Installer "$destination/unins000.exe" @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
}
if ([Environment]::GetEnvironmentVariable('Path', 'User') -ne $beforePath) { throw 'Uninstall did not restore user PATH' }
if (Test-Path "$destination/arcdeck.exe") { throw 'Binary remains after uninstall' }
'Uninstall and original user PATH verified'
