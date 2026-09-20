<#
  跑协议层验收工具 —— **构建失败必须炸，不能静默跑旧的 dll**。

  ## 为什么需要这一层包装

  这是实测踩到的坑，不是防御性编程：

      dotnet build … | Select-Object -Last 5     # 构建**失败**
      dotnet run --no-build …                    # 却照常跑，用的是上一次的 dll

  `Select-Object -Last N` 把失败的几行截掉了，而 `--no-build` 不会校验
  产物是否比源码新。于是「我刚修好的 bug 还在」看起来像**修复无效**，
  实际是修好的那份根本没被编译进去 —— 为这个白花了一轮。

  所以这里显式检查 `$LASTEXITCODE`，并且**只在这一处**允许 `--no-build`
  （它紧接着一次成功的构建）。

  用法（5.1 与 7 都可以；脚本必须带 BOM，见 AGENTS.md U13b）：

      powershell -File uwp\tools\sdk\run-cli.ps1 --prompt "你好"
      powershell -File uwp\tools\sdk\run-cli.ps1 --trace --prompt "数到三"

  参数原样透传；开头的 `--` 会被丢掉。
#>

# 刻意**不用** param() 接参数：`[Parameter(ValueFromRemainingArguments)]`
# 在 PowerShell 5.1 的 `-File` 调用下会把 `--cwd C:\…` 里的路径当成位置参数，
# 报 "A positional parameter cannot be found"。用内置的 $args 最稳。
$ErrorActionPreference = 'Stop'

# $PSCommandPath = <repo>\uwp\tools\sdk\run-cli.ps1 → 往上**四**层才是仓根。
# 少一层得到的是 <repo>\uwp，Join-Path 之后仍是一个合法路径字符串，
# dotnet 只会报 MSB1009「项目文件不存在」—— 层数错了不会自己暴露。
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath)))
$proj = Join-Path $root 'arc-deck-dsh\src\ArcDeck.Dsh.Cli\ArcDeck.Dsh.Cli.csproj'

if (-not (Test-Path $proj)) {
  throw "找不到项目文件：$proj —— 脚本所在层级与仓结构对不上了"
}

Write-Host '── 构建 ────────────────────────────────────────'
& dotnet build $proj -v:q --nologo
if ($LASTEXITCODE -ne 0) {
  # 这里**不**吞输出：失败的那些行就是唯一有用的信息。
  throw "构建失败（exit $LASTEXITCODE）—— 不要拿旧 dll 继续跑"
}

Write-Host '── 运行 ────────────────────────────────────────'
$extra = @($args | Where-Object { $_ -ne '--' })

& dotnet run --no-build --project $proj -- @extra
exit $LASTEXITCODE
