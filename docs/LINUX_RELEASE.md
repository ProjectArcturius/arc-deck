# 1.0 Linux 发行交接（Arch）

用户 2026-10-20 确认当前没有 Linux 机器连接；本轮只准备打包文件，略过 Linux 资产上传。
下面是待在 Linux **实际执行**的步骤，不是已通过的测试记录。

## 源码

优先把本轮工作区完整带去 Arch，或下载草稿 release 附带的源码资产（包含本轮修改）。
正式发布前确认 v1.0.0 指向这些修改所在的提交，不能用旧 main 的源码替代本轮源码包。

```bash
gh release download v1.0.0 --repo ProjectArcturius/arc-deck --pattern '*source.tar.gz' --pattern '*aur.tar.gz' --pattern 'SHA256SUMS-source.txt'
sha256sum -c SHA256SUMS-source.txt
tar xf arc-deck-1.0.0-source.tar.gz
tar xf arc-deck-1.0.0-aur.tar.gz
```

草稿资产需有仓库访问权限的 `gh` 下载；正式发布后 recipe 的公开下载 URL 才可用。

## Arch / AUR

```bash
sudo pacman -S --needed base-devel rust nodejs npm git
cp arc-deck-1.0.0-source.tar.gz arc-deck-aur/
cd arc-deck-aur
makepkg --verifysource
makepkg -s
makepkg --printsrcinfo > .SRCINFO
sudo pacman -U ./arc-deck-1.0.0-1-*.pkg.tar.zst
arcdeck --version
```

recipe 带源码 SHA256，不使用 SKIP。`check()` 跑完整四 crate 离线测试（live 默认忽略），
`prepare()` 联网 `cargo fetch --locked`。若已有旧目录，应在空构建目录解压，避免旧产物干扰。
当前 recipe 声明 x86_64/aarch64；**只把实际构建测试过的架构当作已支持**。

真终端日用验收：两轮上下文、流式时 Esc 后立即发新问题、工具运行中 Esc 后再聊、
`/permission confirm` 允许/拒绝一次、Plan 拒绝写入、切模型/Agent 后继续同一会话、
退出重开恢复/继续、`/search` `/diff` `/output`、粘贴、中文、Shift+Enter、退出后 shell 正常。
真实模型调用会耗配额，执行前按用户授权控制范围；零配额的真内核取消测试可运行：

```bash
cd ../arc-deck-1.0.0
arcdeck runtime install
cargo test --locked -p arc-deck-dsh --features test-runtime remote_live_cancel_tool_then_immediately_prompt -- --ignored --nocapture
```

上传 release 的 Arch 二进制包前，记录 `uname -m`、glibc 与 rustc 版本及测试结果。
AUR 上架是单独一步：确认命名空闲后，把生成的 PKGBUILD 与 `.SRCINFO` 放入 AUR 的
`arc-deck` 仓库，按用户授权提交推送；**本轮没有向 AUR 服务器发布**。

## Debian deb：在 Debian 容器内构建

Arch 的 glibc 可能比 Debian 新；**不能把 Arch 编出的 ELF 直接封成兼容 Debian 的 deb**。
使用本仓 `packaging/debian/Dockerfile`，基于 Rust 1.95.0 + Debian bookworm；先确认该镜像可拉取。
产物目标 Debian 12+ / Ubuntu 24.04+，系统仍须单独配置 Node.js >=24 的来源。

```bash
cd arc-deck-1.0.0
mkdir -p target/dist
docker build -f packaging/debian/Dockerfile -t arcdeck-deb:1.0.0 .
docker run --rm -v "$PWD/target/dist:/out" arcdeck-deb:1.0.0
dpkg-deb --info target/dist/arc-deck_1.0.0-1_amd64.deb
dpkg-deb --contents target/dist/arc-deck_1.0.0-1_amd64.deb
```

Dockerfile 中跑四 crate 测试，打包脚本用 `dpkg-shlibdeps` 从真实 ELF 推导依赖。
再在干净 Debian/Ubuntu 容器里安装 deb（准备 Node.js >=24、npm、Git），检查
`arcdeck --version`、`config init`、`runtime install`、headless；TUI 则在真终端中验收。
`dpkg-deb --info` 成功不等于安装/运行已验证。

## 补传草稿 release

实际构建和检查通过后才上传。文件名按实际架构取，不使用无匹配时的原样 glob。

```bash
sha256sum arc-deck-1.0.0-1-x86_64.pkg.tar.zst arc-deck_1.0.0-1_amd64.deb > SHA256SUMS-linux.txt
gh release upload v1.0.0 arc-deck-1.0.0-1-x86_64.pkg.tar.zst arc-deck_1.0.0-1_amd64.deb SHA256SUMS-linux.txt --repo ProjectArcturius/arc-deck
gh release view v1.0.0 --repo ProjectArcturius/arc-deck
```

Linux 实测结果补进 release notes，再由用户决定发布。参考：
[Arch PKGBUILD 手册](https://man.archlinux.org/man/PKGBUILD.5.en)、
[Debian control 字段](https://www.debian.org/doc/debian-policy/ch-controlfields.html)。
