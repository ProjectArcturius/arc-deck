# Arc Deck

**1.0.0 发布准备**：[安装](docs/INSTALL.md) · [修复与验收记录](docs/releases/v1.0.0.md) ·
[Arch 上构建 AUR/deb 与补传 Linux 资产](docs/LINUX_RELEASE.md)。本版范围是 TUI + headless；Linux 本轮尚未构建，桌面 GUI 继续冻结。

基于 DeepSeek Harness（DSH）的编码 Agent 产品，目标分为两部分：

| 部分 | 编码方向 | 使用者 | 状态 |
|---|---|---|---|
| Rust 后端：`arcdeck` | 无参数进 TUI（视觉与交互基线来自 OpenCode v1.19.0，MIT），带子命令进 headless | 终端用户，以及 Claude 等外部主控派发的 workers（首先是 Ether） | **headless 仍是焦点；TUI 主流程已完成，等待人工视觉对照** |
| 桌面 GUI | Windows：C# + WinUI 2；Linux：Rust + Kanesumi；都经本地 IPC 接入后端 | 桌面用户 | 冻结 |

执行内核只用 DSH：DeepSeek 走官方路由，非 DS 模型走 DSH 的 pi-ai 路由（调查见 [docs/DSH_FINDINGS.md](docs/DSH_FINDINGS.md)）。

默认 Agent 为 Echo：不仅定义语言风格，也定义角色、工作纪律、工具与权限、验证及交付要求。Build / Plan 参考 OpenCode；角色与工具体验参考 Claude Code 公开扩展，执行内核继续使用 DSH。

需求、架构与 roadmap：[产品需求与编码方向](docs/PRODUCT_PLAN.md)（先读 §0）。**M2a/M2b 与 M2c 的桌面部分已落地并实测**：离线套件、夹具端到端，以及**真实 DSH 运行**（派工 / 探活 / 取消 / 杀监工 / 孤儿回收 / worktree 真提交）在 Windows 与 Arch 上都跑过。逐项证据见 [PRODUCT_PLAN §10.2b](docs/PRODUCT_PLAN.md)。

- `crates/arc-deck-core/` 产品模型（任务/配置/结果/事件/Agent/授权）
- `crates/arc-deck-dsh/` DSH 适配（行式 JSON-RPC、事件投影、假 SDK 对端）
- `crates/arc-deck-cli/` `arcdeck` 二进制：headless 命令面（run / status / wait / report / usage / logs / cancel / ls / auth / runtime / config）
- `crates/arc-deck-tui/` 无参数终端入口：Remote 交互会话（多轮、流式、取消、跨进程恢复）、命令系统、工具视图（diff / 完整输出）、主题与模型对话框；视觉与交互基线是 OpenCode v1.19.0（MIT，见「来源与许可」）
- `agents/`、`prompts/` 默认 Agent 与提示词；`schemas/` 契约示例

```bash
# 全部离线测试；test-runtime 是必需的，发布构建里没有夹具
cargo test -p arc-deck-core -p arc-deck-dsh -p arc-deck-cli -p arc-deck-tui \
    --features arc-deck-cli/test-runtime --offline

# 端到端离线验收：不发任何模型请求，退出码 0 表示 completed
cargo run -p arc-deck-cli --bin arcdeck --features test-runtime -- worker run \
    --config tests/fixtures/worker-offline.json \
    --task tests/fixtures/worker-task.json --format jsonl --result result.json
```

- **headless 已可用**（Windows 与 Arch 均实测）：`arcdeck runtime install` → `arcdeck run <任务书> --by <谁> --accept "<验收命令>" --detach` → `arcdeck status` / `report`。用法见 `arcdeck --help` 与 [PRODUCT_PLAN §10.2b](docs/PRODUCT_PLAN.md)；Ether 接入示例在 `examples/ether/`。
- **TUI 主流程已完成**：Remote 多轮与流式、Esc 取消、跨进程恢复、命令系统、`/sessions` `/runs` `/search` `/subagents` `/timeline`、工具视图（`/diff` `/output`）、Plan / confirm 审批、输入历史与 stash、OSC 52 复制。离线 **227** 个 TUI 测试，真终端探针有证据（[docs/M4_PLAN.md](docs/M4_PLAN.md)）。**唯一未完成的是 §7.5 与 OpenCode 的同终端视觉对照**（要人看）。
- **授权落到执行层（M3）**：只读 / Plan 由 DSH 的 OS 沙箱强制，关 shell 即摘工具，表达不了的组合拒绝启动；`confirm` 经 `$events` 审批通道（[docs/M3_GUARD.md](docs/M3_GUARD.md)）。
- **TUI 入口**：`cargo run -p arc-deck-cli --bin arcdeck --offline` 无参数进入（release 二进制直接 `arcdeck`）。`Enter` 发送，`Shift+Enter` / `Ctrl+J` 换行；Windows 的原生多行粘贴按同一次输入批处理识别，不再需要 F6；`Ctrl+P` 查看命令。若启动环境带 `NO_COLOR=1`，通知行会明确说明颜色被关闭。
- 尚未实现 / 未验证：macOS、真实运行时卡死续跑、Arch TUI 真终端、GUI 接线、M2d 对照门；§7.5 TUI 视觉对照待人工。core 算出的路径 / 提交限制尚未完整落实到真实工具执行，不以提示词或策略留档宣称隔离。
- `REFS/` 是被 Git 忽略的本地参考；构建和发布不能依赖它。OpenCode 为 MIT；TUI 中直接移植或逐字取用的部分保留 MIT notice（`crates/arc-deck-tui/LICENSE-OpenCode`）。Claude Code 参考仓的根许可证不是 MIT，不默认复制其实现或提示词。
- 现有 `kanesumi/`、`uwp/` 与 C# `arc-deck-dsh/` 保留，不进行删除或真实数据迁移。

## 来源与许可

- **TUI 来源于 OpenCode。** `crates/arc-deck-tui/` 的视觉与交互基线是 OpenCode `v1.19.0`（TUI），主题资产取自 `v2.0.27`；颜色 / 主题解析、模型目录、补全行为等直接移植或逐字取用的部分，在文件头标注了来源。OpenCode 以 **MIT** 发布，Copyright (c) 2025 opencode；完整 notice 见 [`crates/arc-deck-tui/LICENSE-OpenCode`](crates/arc-deck-tui/LICENSE-OpenCode)，主题清单与上游路径见 [`crates/arc-deck-tui/themes/README.md`](crates/arc-deck-tui/themes/README.md)。
- OpenCode 只作为 TUI 与 Agent 行为的设计与移植来源；执行内核仍然只有 DSH。
- Arc Deck 以 **Apache-2.0** 发布，完整文本见根 [`LICENSE`](LICENSE)，第三方归属见 [`NOTICE`](NOTICE)；`Cargo.toml` 的 `workspace.package.license` 同步为 `Apache-2.0`。TUI 中来自 OpenCode 的 MIT 部分继续按 MIT 保留 notice，不因仓库主许可证改变。

## 历史实现与验收记录

以下内容记录新方向确立前的两条 GUI 路线、构建入口和既往测量，不再作为新增功能的路线选择。相关测量没有在本轮复跑；当前需求、优先级和未验证项以 [PRODUCT_PLAN.md](docs/PRODUCT_PLAN.md) 为准。

此前 Arc Deck 是 DeepSeek Harness 的原生桌面客户端，两个实现并列：

| | `kanesumi/` | `uwp/` |
|---|---|---|
| 语言 | **Rust**（edition 2024） | **C# + XAML** |
| 界面 | 自绘 `Scene` 渲染命令 | **WinUI 2** 控件 |
| 外壳 | 自研 Win32 事件循环 | UWP AppContainer |
| 平台 | Windows 优先，随后 Linux | **Windows 专属** |
| 构建 | `cargo` | **MSBuild** |
| 外观 | 自定（`based on Kanesumi Design`） | **跟系统走** |
| 状态 | 骨架 + IME 实测通过 | **完整应用，已跑通** |

**两者是同一个客户端的两条实现路线，代码不共享。**

- `kanesumi/` —— 自绘渲染，为跨平台一致性。（`based on [Kanesumi Design](https://github.com/ProjectArcturius/Ether/blob/main/docs/KANESUMI_DESIGN.md)`）
- `uwp/` —— 用微软自家的接口与套件。外观完全交给 Windows，不覆盖任何主题资源。

## 为什么两个都做

它们不是「主选 + 备选」，而是两条各自成立的路，取舍点很明确：

| | `kanesumi/`（自绘） | `uwp/`（WinUI 2） |
|---|---|---|
| 文本编辑 / 滚动 / 命中测试 | **要自己写** ← 真实工期风险 | **平台免费给，且成熟** |
| 无障碍 | **完全没有** ← 最大缺口 | **平台免费给** |
| IME | 自己实现（**已实测通过**） | 平台免费给 |
| 跨平台 | ✅ 同一套 App 逻辑换外壳 | ❌ 只在 Windows |
| 外观控制 | 类型级强制 | 跟系统走（刻意不做覆盖） |
| 分发 | 裸 exe，随便发 | MSIX + 签名 |

**结论**：要跨平台一致性走 `kanesumi/`；只要 Windows、且想用平台现成的
基础设施，走 `uwp/`。

## 共同的出发点：不用 WebView

现有的 DSH 桌面外壳都是同一招：把内核自己吐的 HTML 塞进 WebView。

| 项目 | 外壳 | 版本 |
|---|---|---|
| `dataelement/dsh-desktop` | Electron（完整 Chromium） | 0.8.2 |
| `dsh-tauri/deepseek-harness-desktop` | Tauri 2（系统 WebView） | v0.15.5 |

实测当前 Electron 外壳的占用（同一台机器、同一时刻）：

| 进程 | 工作集 |
|---|---:|
| Chromium（5 个进程） | ≈ 1.0 GB |
| node（DSH harness 本体） | 1,462 MB |
| **合计** | **≈ 2.5 GB** |

其中 1,462 MB 是 harness 本体，与外壳无关；**可省的是那 ≈ 1.0 GB Chromium，以及它
累计 1,937 秒的 CPU 时间**。

Arc Deck 走第三条路：**没有 WebView**。

## 设计基础

设计语言正典是 **Kanesumi Design**，跨平台唯一权威定义见 Ether monorepo 的
`docs/KANESUMI_DESIGN.md`。四根支柱：

1. **几何：直角** —— 无圆角、无曲线
2. **布局：贴边** —— 无边框、信息优先
3. **色彩：深底 + 单一强调色**
4. **运动：受控混合** —— 弹簧只用于主运动，次属性走 UWP 缓动

正典在代码里是**类型级强制**的，不是纪律：

- `CornerRadius` 只有三个合法值：`Square`(0.0，默认) / `Slight`(2.0) / `Capsule`
- `SceneCommand` 只有八个变体（`FillRect` / `StrokeRect` / `Text` / `Arc` /
  `PushClip` / `PopClip` / `Image` / `Triangle`）—— **没有渐变变体**，
  「无渐变」在命令模型里结构性不可能
- 弹簧求解器是**解析闭式**（非欧拉积分）：帧率无关、无条件稳定、可中断

## 依赖结构

对设计系统与内核一律用 **Cargo git 依赖 + `rev` 钉死**，**不使用 git submodule**：

```toml
[dependencies]
kanesumi-core     = { git = "...", rev = "<sha>", subdirectory = "kanesumi-core" }
kanesumi-canvas   = { git = "...", rev = "<sha>", subdirectory = "kanesumi-canvas" }
kanesumi-controls = { git = "...", rev = "<sha>", subdirectory = "kanesumi-controls" }
```

理由：Cargo 的 git 依赖天然**按 crate 选择**且支持 `subdirectory`。submodule 是
「整仓一个检出点」的工具 —— 用错工具才会有「不能选择性 sub」的错觉。

本地开发改动上游时，用 `[patch]` 换成本地检出（Ether monorepo 已在用同一招）。

## 关键约束

**DSH 侧协议无版本号、无能力协商。** 上游在 0.1.2 → 0.1.6 之间把协议重构了两次
（`packages/host/apiproxy` 已删除，取而代之是 `dsh-api-gateway` +
`dsh-client-connection`）。归档笔记里唯一的决策记录写着：

> *"No protocol version: client and host release bound together; introduce one
> when an independently released client appears."*

**Arc Deck 就是那个「独立发布的客户端」**，而那个字段尚不存在。因此：

- `dsh` 版本必须**钉死**，升级是刻意的工程动作，不是自动跟随
- 协议面：`POST /api/<namespace>/<method>` + 单条 WebSocket `GET /api/remote.mux`

## 目录

```
arc-deck/
├── crates/
│   ├── arc-deck-core/             产品模型（任务/配置/结果/事件/Agent/授权）
│   ├── arc-deck-dsh/              DSH 适配（行式 JSON-RPC、事件投影、Remote 会话）
│   ├── arc-deck-cli/              arcdeck 二进制：headless 命令 + 无参数 TUI 入口
│   └── arc-deck-tui/              终端界面（视觉/交互基线来自 OpenCode，上游 MIT）
├── kanesumi/                      早期自绘 GUI（已暂停新增功能）
│   ├── crates/arc-deck-shell/
│   │   └── src/
│   │       ├── scene.rs           SceneCommand（八变体，无渐变/阴影/圆角）
│   │       ├── input.rs           平台无关输入事件
│   │       ├── app.rs             App trait + TextEngine 度量契约
│   │       ├── ime.rs             IME 状态机（纯逻辑，可单测）
│   │       ├── platform/
│   │       │   ├── mod.rs         Win32 事件循环 / 消息泵 / 事件翻译
│   │       │   └── win32.rs       全仓唯一允许裸 FFI 的地方
│   │       └── bin/arc-deck-demo.rs
│   └── probes/
│       ├── ime-probe/             IMM32 探针 + 输入链路验证 + 窗口截图
│       └── layout-bench/          流式排版性能
├── uwp/                           C# + WinUI 2 版本
│   └── src/                       UWP 应用（ArcDeck.csproj）
│       ├── ShellPage.xaml         NavigationView 外壳（响应式窗格 + 搜索框）
│       ├── SessionsPage.xaml      会话列表
│       ├── ChatPage.xaml          对话页（CommandBar / 消息模板分派 / 工具卡片）
│       ├── WorkspacesPage.xaml    工作区归类
│       ├── ToolsPage.xaml         WinUI 2 控件一览
│       ├── SettingsPage.xaml      设置
│       └── Models/                数据与模板选择器
├── arc-deck-dsh/                  旧 C# 共用协议层（历史，界面未接线）
├── docs/
│   ├── PRODUCT_PLAN.md            产品需求与编码方向（**入口，先读 §0**）
│   ├── STATE_2026-10-14.md        交接快照：此刻的状态、仓外的东西、换机器步骤
│   ├── M4_PLAN.md                 TUI 计划、状态与真终端证据
│   ├── M3_GUARD.md                授权策略 → DSH 执行层的映射表、缺口与负例清单
│   ├── ARCDECK_WORKERS_REVIEW_2026-10-14.md  用 arcdeck 工人替代 OpenCode 工人：实测、成本、对 Ether 的意义
│   ├── CORE_HEADLESS_AUDIT_2026-10-14.md     core / headless 完备性审计与 M4a（历史，含 10-20 更正）
│   ├── DSH_FINDINGS.md            DSH 对非 DS 模型是否适用
│   ├── PERSONA_FINDINGS.md        人格与风格怎么装（Claude Code 的做法）
│   ├── ARCHITECTURE.md            Rust 侧早期架构快照（历史）
│   ├── DESIGN_SYSTEM.md           正典落地取值（**只与 kanesumi/ 相关**）
│   └── PERF_FINDINGS.md           流式排版实测
├── prompts/constitution.md        宪法草案（不可让渡层；**尚未接进装配**）
├── tasks/m4/                      M4 任务书队列与派工手册（README.md：依赖、派工命令、收工七步、踩过的坑）
├── LICENSE                        仓库主许可证（Apache-2.0）
├── NOTICE                         第三方归属（含 OpenCode 的 MIT notice 指引）
└── AGENTS.md                      协作约定：通用 + kanesumi 侧 K1–K9 + uwp 侧 U1–U18
```

### 构建 Rust 版本

产品 crate（core / dsh / cli / tui）：

```bash
# 全部离线测试；test-runtime 是必需的（假 SDK 对端与测试子命令不在发布构建里）
cargo test -p arc-deck-core -p arc-deck-dsh -p arc-deck-cli -p arc-deck-tui \
    --features arc-deck-cli/test-runtime --offline

cargo run -p arc-deck-cli --bin arcdeck --offline   # 无参数进 TUI
```

早期自绘 GUI（`kanesumi/`，已暂停新增功能）：

```bash
cargo test                       # 21 个测试
cargo run --bin arc-deck-demo    # 骨架 demo（含中文输入行）
cargo run -p ime-probe           # IMM32 探针
cargo run -p layout-bench --release
```

### 构建 UWP 版本

见 [`uwp/README.md`](uwp/README.md)。要点：**必须用 MSBuild**（UWP 不支持 `dotnet build`），
用 **Release** 配置（Debug 依赖未预装的调试运行时）。

```powershell
$msbuild = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\MSBuild.exe"
& $msbuild uwp\src\ArcDeck.csproj /t:Restore /p:Configuration=Release /p:Platform=x64
& $msbuild uwp\src\ArcDeck.csproj /p:Configuration=Release /p:Platform=x64
```

## Rust 侧的探针

### IME 可行性（否决性）

**这是唯一能杀死本项目的风险。**

2026 年，所有纯 Rust GUI 框架的中文 IME 都是未完成的：egui 的 macOS IME 卡在上游
`winit#4432`（PR 自己的描述是 *"was buggy, still buggy"*）；iced 输入 3 个中文即 panic，
修复 PR 挂了半年；Makepad 在 Windows 上「中文显示为方块，然后软件冻结」。

但这些框架的问题是**它们不拥有那一层** —— IME 归 winit/SCTK 管，框架只能等。

Kanesumi 的 Linux 外壳用 `smithay-client-toolkit` + `calloop` **自研事件循环**，
不用 winit；Windows 侧照此办理，直接写 Win32 消息泵。**IME 完全归自己管。**

**实测结论：通路成立。** 微软拼音在自研事件循环里逐键推进
`z → zh → zho → zhon → zhong`，组合串带橙色下划线正确渲染，与已提交文本视觉分明。

`arc-deck-demo` 是端到端验证载体：窗口里有可输入的行，组合串与提交结果分别可见，
状态行显示组合串原文。

已用测试钉死的 IME 纪律（`ime.rs`）：

1. 组合串**永远不进入文档**
2. 提交串是唯一产出 `InputEvent::Commit` 的路径
3. 重新组合清空上一轮，不累积
4. 取消组合清空但不提交
5. `GCS_CURSORPOS` 给的是**字节**偏移（「中」占 3 字节）—— 换算时在多字节字符
   中间向外夹紧，不得切坏 UTF-8
