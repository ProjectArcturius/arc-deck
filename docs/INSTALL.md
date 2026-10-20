# 安装 Arc Deck 1.0

本版发布 `arcdeck`（TUI + headless），不含冻结中的桌面 GUI。

## Windows x64

推荐下载 `arcdeck-1.0.0-windows-x86_64-setup.exe`，双击安装。默认装到
`%LOCALAPPDATA%\Programs\Arc Deck`，只改当前用户的 PATH，无需管理员权限。
安装后**重新打开终端**，运行 `arcdeck --version`。也可用 Windows「已安装的应用」卸载；
卸载只移除程序和安装器添加的 PATH，保留配置、密钥、运行时和会话。
新安装目录放在用户 PATH 首位，避免继续启动 Cargo 目录中的旧 `arcdeck`。

ZIP 版：解压 `arcdeck-1.0.0-windows-x86_64.zip`，双击 `install.cmd` 即可安装到同一用户目录并加入 PATH，
卸载时运行安装目录里的 `uninstall.cmd`。也可直接用 `./arcdeck.exe`，作为便携程序使用。
不要把它解压到项目目录；两种安装方式选一种，不要混用升级。

使用 Windows Terminal。旧 conhost 中文代码页对 `■`、`·` 等字符的宽度处理可能错位。
安装包未做代码签名；校验 release 的 `SHA256SUMS-windows.txt` 后再安装。
本轮安装环境对 Setup 子进程的用户 TEMP / 开始菜单 / HKCU 写入返回「拒绝访问」；
Setup 已编译并验证解压，完整向导安装仍待普通桌面环境验收。ZIP 的脚本安装、PATH 更新与卸载恢复已实测。

## 首次配置（所有平台）

先安装 **Node.js 24 或更高版本（含 npm）**、Git，再运行：

```text
arcdeck config init
arcdeck runtime install
arcdeck auth set opencode-go
```

`auth set` 后粘贴密钥，回车后发送 EOF：Windows 用 Ctrl+Z 再 Enter；Linux 用 Ctrl+D。
密钥保存在用户的 `auth.json`，不要写进项目或 `arcdeck.json`。如果已有 OpenCode 凭据，
也可用 `arcdeck auth import-opencode`。

默认配置使用 `opencode-go/deepseek-v4.1-flash`，需要对应的订阅/密钥。
使用 DeepSeek 官方时，编辑 `arcdeck config path` 输出的文件，把 `model` 改成
`deepseek/deepseek-flash`，再用 `arcdeck auth set deepseek` 写入官方密钥。
其他供应商配置见 [PRODUCT_PLAN §9.2b](https://github.com/ProjectArcturius/arc-deck/blob/main/docs/PRODUCT_PLAN.md)。

`runtime install` 联网安装钉住的 DSH `0.2.1-alpha.2`（数百 MB）；它不随安装包分发。
默认关闭会话日志上传与个性化。升级程序不会覆盖已有配置；不要为升级使用 `config init --force`。

在你的项目目录打开终端，运行 `arcdeck`。Enter 发送，Shift+Enter / Ctrl+J 换行，Esc 取消，
Ctrl+P 命令，`/sessions` 恢复会话，`/reconnect` 重连内核，Ctrl+C 退出。

## Linux

本轮没有 Linux 构建机，Linux 二进制尚未构建或验证。
维护者在 Arch 构建 AUR/deb 与补传资产的步骤见 [LINUX_RELEASE.md](https://github.com/ProjectArcturius/arc-deck/blob/main/docs/LINUX_RELEASE.md)。

Arch 本地安装：在生成的 AUR 包目录运行 `makepkg -si`。AUR recipe 准备就绪不代表已经发布到 AUR。
Debian/Ubuntu：在满足 Node.js >=24 的系统里运行 `sudo apt install ./arc-deck_1.0.0-1_amd64.deb`。
deb 必须在 Debian 构建容器中生成，不要直接打包 Arch 的动态链接二进制。

## 排障与数据位置

```text
arcdeck config path
arcdeck runtime status
arcdeck auth ls
```

Windows：配置/密钥在 `%APPDATA%\arcdeck`，运行时/会话/运行记录在 `%LOCALAPPDATA%\arcdeck`。
Linux：配置在 `~/.config/arcdeck`，密钥/运行时/会话在 `~/.local/share/arcdeck`，遵守 XDG 环境变量。
缺运行时运行 `runtime install`；缺凭据运行 `auth set <provider>`；磁盘写满先腾空间。
每份交互会话目录只允许一个内核写入；启动时提示占用，先退出正在使用该目录的 Arc Deck。
