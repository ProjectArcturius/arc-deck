# 把伴随进程发布进 UWP 包的 content 目录（uwp/src/Host/）。
#
# 为什么要单独一步：UWP 的 csproj 是旧式 MSBuild 工程，不能在里面直接
# `dotnet publish` 一个 net9.0 项目；而 `Host\**` 是打包的 Content。
# 忘了这一步的表现是「包能装上，但 FullTrustProcessLauncher 报找不到可执行文件」。
param(
    [string]$Configuration = "Release"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root "src\Host"

dotnet publish (Join-Path $PSScriptRoot "ArcDeck.Host.csproj") `
    -c $Configuration -r win-x64 --self-contained false -o $out

if ($LASTEXITCODE -ne 0) {
    throw "dotnet publish 失败（退出码 $LASTEXITCODE）"
}

Write-Host "伴随进程已发布到 $out"
