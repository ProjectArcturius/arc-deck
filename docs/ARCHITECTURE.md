# Arc Deck · 早期 Kanesumi 架构记录

> 本文是早期 Rust 自绘 GUI 骨架的设计快照，不是当前全产品架构。新的 TUI/headless + WinUI GUI 分工、Echo、工具与 worker 需求以 [PRODUCT_PLAN.md](PRODUCT_PLAN.md) 为准。下文「协议层尚未建立」等状态只记录当时，不能拿来判断现有 C# 协议层或新 Rust 规划的完成度。

## 分层

```text
┌─────────────────────────────────────────────────────────┐
│ arc-deck-ui       会话 / 工具卡片 / 轨迹 / 文件树        │  ← 尚未建立
├─────────────────────────────────────────────────────────┤
│ arc-deck-dsh      DSH 协议客户端（HTTP + WebSocket）      │  ← 尚未建立
│                   只消费 App trait，不碰平台层            │
├─────────────────────────────────────────────────────────┤
│ arc-deck-shell    App trait / Scene / Input / ImeState   │  ← 已建立
│                   平台无关 + cfg(windows) 外壳            │
└─────────────────────────────────────────────────────────┘
```

**依赖方向严格单向。** 尤其：`arc-deck-dsh` **不碰平台层任何东西** —— 它只认识
`Scene` 与 `InputEvent`。这带来两个直接好处：

1. DSH 协议客户端的改动**不可能**破坏 Windows 外壳
2. 上游 DSH 协议再次重构时，改动被隔离在一个 crate 里

## 与 Kanesumi 的关系

Arc Deck 的 `Scene` / `SceneCommand` / `App` / `TextEngine` **刻意镜像**
Kanesumi 的对应物，逐变体一一对应。这样将来若要直接消费上游
`kanesumi-canvas` + `kanesumi-controls`（约 48 个控件），替换是无痛的。

上游现状（已核实）：

| Crate | 行数 | `cfg(target_os)` | Windows 可用 |
|---|---:|---:|---|
| `kanesumi-core` | 725 | 0 | ✅ |
| `kanesumi-anim` | 301 | 0 | ✅ |
| `kanesumi-canvas` | 2,568 | 0 | ✅ |
| `kanesumi-structure` | 1,232 | 0 | ✅ |
| `kanesumi-controls` | 21,914 | 0 | ✅ |
| `kanesumi-harness` | 7,821 | 6 | ❌ 平台外壳是 Linux 专属 |

**结论：UI 层完全可用，缺的只有外壳** —— 而外壳正是 Arc Deck 在造的东西。

## 为什么不用 winit

**这是本项目最重要的一个架构决策。**

2026 年，**所有**纯 Rust GUI 框架的中文 IME 都是未完成的：

| 框架 | 症状 |
|---|---|
| egui | macOS IME 卡在上游 `rust-windowing/winit#4432`；PR 自己的描述是 *"was buggy, still buggy"* |
| iced | 输入 3 个中文即 panic（#3232）；修复 PR 自 2026-03-22 起未合并 |
| Slint | 组合期间失焦丢最后一个字（#10861） |
| GPUI | 69 个 IME 标题 issue；Zed 至今不支持读屏 |
| Makepad | Windows 上「中文显示为方块，然后软件冻结」（#756） |
| Xilem | 剩余 CJK IME 修复为「已关闭未合并」或未审阅草稿 |

**根因不是那些框架写得差，而是 IME 归 winit/SCTK 管，框架只能等。**

Kanesumi 的 Linux 外壳用 `smithay-client-toolkit` + `calloop` **自研事件循环**，
不用 winit。Windows 侧照此办理：**自己写 Win32 消息泵，IME 完全归自己管。**
于是上面整张表都与 Arc Deck 无关。

## 重绘门控（中心化）

`App::needs_redraw()` 是**唯一**的重绘判据，外壳在消息循环末尾统一检查。

这是从 PezMax-One 的实测教训来的：它曾经让每个动画各自调用
`ctx.request_repaint()`，结果**一个 GIF 头像就导致空闲时全屏 60fps 空转**，
成为后台 CPU 的主要来源。修法是把手写调用收敛成一处判断。

结构上的收益：**「新增了一个动画却忘了请求重绘」这类缺陷不可能发生** ——
因为不存在「请求重绘」这个动作，只有一个布尔状态。

## IME 纪律（已可测）

`ImeState` 是**纯逻辑状态机**，零平台依赖。四条不变量各有对应测试：

1. 组合串只存在于 `preedit`，**永远不进入文档**
2. 提交串是唯一会产出 `InputEvent::Commit` 的路径
3. 重新开始组合会清空上一轮（不累积）
4. 取消组合（失焦 / Esc）清空组合串但**不产出提交**

另有一条真实的工程陷阱已用测试钉死：**`GCS_CURSORPOS` 给的是字节偏移，
不是字符偏移。** 组合串含中文时两者不等（「中」占 3 字节）——
`byte_to_char_index` 负责换算并在多字节字符中间向外夹紧，这正是多数框架
丢字/乱码的根因。

把这一层做成纯逻辑而非窗口绑定，是因为那些缺陷多半出在**生命周期纪律**上，
与操作系统无关 —— 所以能用测试钉死，而不必靠手工点窗口发现。

## 依赖结构

对上游设计系统与内核一律用 **Cargo git 依赖 + `rev` 钉死**，**不使用 git submodule**：

```toml
[dependencies]
kanesumi-canvas = { git = "https://github.com/ProjectArcturius/Kanesumi",
                    rev = "<sha>", subdirectory = "kanesumi-canvas" }
```

Cargo 的 git 依赖天然**按 crate 选择**且支持 `subdirectory`。submodule 是
「整仓一个检出点」的工具 —— 用错工具才会产生「不能选择性 sub」的错觉。

本地开发改动上游时用 `[patch]` 换成本地路径（Ether monorepo 已在用同一招
把 sokuou 收敛到 `shared/sokuou`）。

**可行性前提已具备**：kanesumi 每个 crate 的 manifest 都自带
`version`/`edition`/`license`/`authors`，上游注释明说这是为了「跨 workspace
被 path 依赖消费」—— 它就是为跨仓消费设计的。

## DSH 侧的关键约束

**协议无版本号、无能力协商。** 上游在 0.1.2 → 0.1.6 之间重构了两次
（`packages/host/apiproxy` 已删除，取而代之是 `dsh-api-gateway` +
`dsh-client-connection`）。归档笔记里唯一的决策记录写着：

> *"No protocol version: client and host release bound together; introduce one
> when an independently released client appears."*

**Arc Deck 就是那个「独立发布的客户端」**，而那个字段尚不存在。

因此：`dsh` 版本必须**钉死**；升级是刻意的工程动作，不是自动跟随。

协议面（0.1.6 已核实）：

- 一元：`POST /api/<namespace>/<method>`，信封 `{type:'client-request', rpcId, method, payload:{args}}`
  → `{type:'server-response', rpcId, result}` —— **必须校验 rpcId 回显**
- 流：**全产品唯一一条** WebSocket `GET /api/remote.mux`
- 服务端发起：WS `waterfall` 帧进来，用 HTTP 一元 `POST /api/$events/result` 应答

## 已知缺口

诚实记录，避免「假装画了」：

| 项 | 状态 |
|---|---|
| `Arc` / `Triangle` / `Image` 命令 | GDI 参考渲染器**未实现**，静默跳过并计数 |
| `PushClip` / `PopClip` | **语义已定，实装为空** —— 裁剪应由软件光栅化器实现 |
| 文本排版 | `HeuristicTextEngine` 是**占位**：按字号线性估算，不经字体。**不得用于最终排版决策** |
| 光标闪烁 | 常亮。应由 sokuou 的 `Progress` 驱动，不在外壳里手搓定时器 |
| 无障碍 | **完全没有**。`Scene` 是纯绘制命令，没有无障碍树 —— 这是正典铁律 7 在桌面端的空白，也是全项目最大的缺口 |

## 后续

1. **IME 端到端实测**（进行中）—— 唯一能杀死项目的风险
2. **流式排版性能实测** —— 长消息逐 token 追加时的每帧耗时
3. 软件光栅化器（替掉 GDI 参考实现，顺带实装裁剪与 `Arc`/`Image`）
4. 接入 `kanesumi-canvas` 的 `TextEngine`（真实塑形 / 断行 / BiDi）
5. 接入 `kanesumi-controls`（48 个控件）
6. `arc-deck-dsh`：DSH 协议客户端
