# AGENTS.md — Arc Deck

协作约定。详细契约见 `docs/ARCHITECTURE.md`（架构）与 `docs/DESIGN_SYSTEM.md`（设计系统）。

## 这是什么

DeepSeek Harness 的**原生**桌面客户端。**不自研协议，不内嵌 WebView。**

名字：产品名 **Arc Deck**，仓名 `arc-deck`，附文 `based on Kanesumi Design`。

## 提交规范

**Conventional Commits**，可用前缀：

| 前缀 | 用途 |
|---|---|
| `feat:` | 用户可见的新能力 |
| `fix:` | 修 bug，不引入新行为 |
| `docs:` | 纯文档 |
| `chore:` | 删文件、改名、gitignore |
| `refactor:` | 不改行为的重构 |
| `test:` | 只改测试 |
| `build:` | 构建系统 / 依赖 / 版本 |

**正文用中文。** 关键设计决策**必须记录原因** —— 让一年后读 `git log` 不需要开 PR。

**一个 section / 逻辑单元一个 commit**：每完成一个 section 就提交一次，保持历史清晰，
无需等待用户要求。

**提交 ≠ 推送。** 提交可自主进行；**推送必须等用户明确指示**，不要「提交一次推一次」。

正文惯用标点：`——` 引出根因或理由（大量使用），`·` 分隔同一提交内的独立改动。
交叉引用用 `参 <doc> §N`。

示例：

```
fix(platform): IME 组合串不触发重绘 —— dirty 标志被误用为重绘请求

WM_IME_COMPOSITION 置 s.dirty = true，而消息循环的判据是
`needs_redraw() && !s.dirty`，于是 InvalidateRect 永远发不出去。
表现为「能唤起输入法但屏幕不更新」。改为 painting（绘制中）与
重绘请求分离，二者语义不同，不可共用一个字段。
```

## 铁律

### 1 · 不用 winit —— IME 必须归自己管

2026 年**所有**纯 Rust GUI 框架的中文 IME 都未完成，而根因是**它们不拥有那一层**
（IME 归 winit/SCTK 管，框架只能等；egui 的 macOS IME 至今卡在上游
`rust-windowing/winit#4432`）。Kanesumi 的 Linux 外壳用
`smithay-client-toolkit` + `calloop` 自研事件循环；Windows 侧照此办理，
**自己写 Win32 消息泵**。不要引入 winit，也不要引入任何自管 IME 的框架。

### 2 · 全仓唯一允许裸 FFI 的地方是 `platform/win32.rs`

新增 Win32 绑定时加在那里。`platform/mod.rs` 里只应有事件翻译与生命周期。
理由：让消息泵逻辑保持可读，FFI 表面收敛到一处可审计。

### 3 · `App` 不持有任何平台句柄

`App` 是纯逻辑，能在没有窗口的环境下单测。这是它能在 Win32 外壳与将来的
Wayland 外壳下复用同一份实现的**唯一前提**。不要为了图方便把 HWND / HDC /
设备指针塞进 `App`。

### 4 · 重绘门控必须中心化

`App::needs_redraw()` 是**唯一**的重绘判据，外壳在消息循环末尾统一检查。
**不要在组件里各自请求重绘** —— 那正是 PezMax-One 踩过的坑（每个动画各自
`request_repaint`，结果一个 GIF 头像就让空闲时全屏 60fps 空转）。

对应地：`ShellState::painting`（当前是否在绘制中）与「是否已排程重绘」是
**两个不同的概念，不可共用一个字段**。早期版本混用二者，导致重绘请求发不出去。

### 5 · 组合串永远不进入文档

`ImeState` 的四条不变量，每条都有测试，改动时不要破坏：

1. 组合串只存在于 `preedit`，**永远不进入文档**
2. 提交串是唯一会产出 `InputEvent::Commit` 的路径
3. 重新开始组合会清空上一轮（不累积）
4. 取消组合清空组合串但**不产出提交**

另有一条真实陷阱：**`GCS_CURSORPOS` 给的是字节偏移，不是字符偏移。**
组合串含中文时两者不等（「中」占 3 字节）。`byte_to_char_index` 负责换算并在
多字节字符中间向外夹紧 —— 这正是多数框架丢字/乱码的根因。

### 6 · IME 消息必须交回 `DefWindowProcW`，且组合串必须转发给 App

`WM_IME_STARTCOMPOSITION` / `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION`
读完自己要的内容后**必须**交给 `DefWindowProcW`。IME 在组合窗口上有自己的簿记，
吞掉会导致组合无法推进（表现为「能唤起输入法但打不出字」）。

**外壳持有 `ImeState` ≠ App 看得见组合串。** 两件事必须显式打通：更新完
`ImeState` 后要调 `handle_input(Preedit)`；组合结束时发一条**空 preedit**
收起显示，否则界面会残留永不消失的下划线。只更新外壳内部状态（标题、候选窗
定位都对）而忘了转发，会让 App 一无所知 —— 这是本项目实测踩过的坑。

### 7 · 诊断计数必须对应被断言的代码路径

`committed` 来自 `ImeState`，而 `WM_CHAR` 路径**不经过** `ImeState`（它直接调
`handle_input`）。用 `committed` 去断言那条路径会得到**假阴性** ——
第一版 `input-probe` 就是这么错的，把一条完好的链路判成「断」。

诊断计数分三层，才能区分「消息没到」「到了没转发」「转发了但没生效」：
`chars_received`（到达 `wnd_proc`）→ `app_events`（转发给 `App`）→ 应用自身状态。

### 8 · `SceneCommand` 只有八个变体，且刻意只有八个

无渐变、无阴影、无圆角：

- **无渐变** —— 正典铁律 6 在命令模型里**结构性**成立（不存在画笔/渐变变体）
- **无阴影** —— 纵深靠明暗，不靠阴影
- **无圆角** —— 直角是几何身份。`FillRect`/`StrokeRect` **没有 radius 字段**；
  若确需 2px，由调用方在 `FillRect` 之上自行叠加，而不是给命令加一个
  99% 调用点都恒为 0 的参数

描边约定**向内**（inside）—— 与「贴边」一致：不侵占相邻元素的像素。

### 9 · 配色跟随桌面扇区，且分歧必须显式记录

正典写「OLED 纯黑 `#000000`」，**Android 扇区严格遵守，Ether 桌面扇区用 `#1A1A1A`**。
**没有文档解释这个分歧。** Arc Deck 跟随桌面扇区，并把它写进
`docs/DESIGN_SYSTEM.md` —— 一个未记录的取值分歧，半年后会变成
「为什么这里跟文档不一样」的困惑。

### 9 · 不假装画了

未实现的命令（当前 GDI 参考渲染器缺 `Arc` / `Triangle` / `Image`；`PushClip`
语义已定但实装为空）**必须显式计数并记录**，不得静默忽略。缺失的诚实记录
好过假装完整。

### 10 · 依赖用 Cargo git 依赖 + `rev` 钉死，不用 submodule

Cargo 的 git 依赖天然**按 crate 选择**且支持 `subdirectory`。submodule 是
「整仓一个检出点」的工具 —— 用错工具才会产生「不能选择性 sub」的错觉。
本地改上游时用 `[patch]` 换成本地路径。

**`dsh` 内核版本必须钉死。** 上游协议**无版本号、无能力协商**，且在
0.1.2 → 0.1.6 之间重构了两次。升级是刻意的工程动作，不是自动跟随。

### 11 · 注释写 why，不写 what

关键决策的注释要记录**促成它的那个 bug**，而不是复述代码。中英混合可接受；
标识符、类型、API 一律英文。

## 当前状态与已知缺口

| 项 | 状态 |
|---|---|
| `Scene` / `App` / 输入 / IME 状态机 | ✅ 已建立，21 个测试 |
| Win32 外壳（窗口 / 消息泵 / 事件翻译 / 重绘门控） | ✅ 已建立 |
| IME 端到端实测 | ⏳ **进行中 —— 唯一能杀死项目的风险** |
| GDI 参考渲染器 | ⚠️ 参考实现，缺 `Arc`/`Triangle`/`Image`，裁剪为空 |
| 文本排版 | ⚠️ `HeuristicTextEngine` 是**占位**，按字号线性估算，不经字体。**不得用于最终排版决策** |
| 无障碍 | ❌ **完全没有**。`Scene` 是纯绘制命令，没有无障碍树 —— 正典铁律 7 在桌面端是空白，也是全项目最大缺口 |
| `arc-deck-dsh` / `arc-deck-ui` | ❌ 未建立 |

## 构建

```bash
cargo test                     # 21 个测试
cargo run --bin arc-deck-demo  # 骨架 demo（含中文输入行）
cargo run -p ime-probe         # 最小 IMM32 探针
```

工具链注意：**默认工具链是 `gnu`**，而 `--target x86_64-pc-windows-msvc` 在该
工具链下不可用（`error[E0463]: can't find crate for 'std'`）。用
`cargo +stable-x86_64-pc-windows-msvc` 或改默认工具链。
