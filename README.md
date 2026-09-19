# Arc Deck

> **based on [Kanesumi Design](https://github.com/ProjectArcturius/Ether/blob/main/docs/KANESUMI_DESIGN.md)** —— 以直角丈量边缘。

DeepSeek Harness 的**原生**桌面客户端。不自研协议，不内嵌浏览器 —— 自绘 `Scene`
渲染命令，跑在自己的事件循环上。

- **平台**：Windows 优先，随后 Linux
- **语言**：Rust（edition 2024）
- **License**：MIT

## 为什么不用 WebView

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
│   └── arc-deck-shell/          # 渲染契约 + 平台外壳（已建立）
│       └── src/
│           ├── scene.rs         # SceneCommand（八个变体，无渐变/阴影/圆角）
│           ├── input.rs         # 平台无关输入事件
│           ├── app.rs           # App trait + TextEngine 度量契约
│           ├── ime.rs           # IME 状态机（纯逻辑，可单测）
│           ├── platform/
│           │   ├── mod.rs       # Win32 事件循环 / 消息泵 / 事件翻译
│           │   └── win32.rs     # 全仓唯一允许裸 FFI 的地方
│           └── bin/
│               └── arc-deck-demo.rs   # 骨架 demo（含 IME 输入行）
├── probes/
│   └── ime-probe/               # 否决性验证：最小 IMM32 探针
└── docs/
    ├── ARCHITECTURE.md
    └── DESIGN_SYSTEM.md
```

```bash
cargo test                     # 21 个测试
cargo run --bin arc-deck-demo  # 骨架 demo（含中文输入行）
cargo run -p ime-probe         # 最小 IMM32 探针
```

## 探针

### IME 可行性（否决性）

**这是唯一能杀死本项目的风险。**

2026 年，所有纯 Rust GUI 框架的中文 IME 都是未完成的：egui 的 macOS IME 卡在上游
`winit#4432`（PR 自己的描述是 *"was buggy, still buggy"*）；iced 输入 3 个中文即 panic，
修复 PR 挂了半年；Makepad 在 Windows 上「中文显示为方块，然后软件冻结」。

但这些框架的问题是**它们不拥有那一层** —— IME 归 winit/SCTK 管，框架只能等。

Kanesumi 的 Linux 外壳用 `smithay-client-toolkit` + `calloop` **自研事件循环**，
不用 winit；Windows 侧照此办理，直接写 Win32 消息泵。**IME 完全归自己管。**

`arc-deck-demo` 是可以端到端验证 IME 的载体：窗口里有可输入的行，
组合串（带下划线 + 强调色）与提交结果分别可见，状态行还会显示组合串原文。

已用测试钉死的 IME 纪律（`ime.rs`）：

1. 组合串**永远不进入文档**
2. 提交串是唯一产出 `InputEvent::Commit` 的路径
3. 重新组合清空上一轮，不累积
4. 取消组合清空但不提交
5. `GCS_CURSORPOS` 给的是**字节**偏移（「中」占 3 字节）—— 换算时在多字节字符
   中间向外夹紧，不得切坏 UTF-8

