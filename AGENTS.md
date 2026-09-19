# AGENTS.md — Arc Deck

Arc Deck 是 DeepSeek Harness 的原生桌面客户端，有**两个并列实现**。

```
kanesumi/   Rust + 自绘 Scene（跨平台，无 WebView）
uwp/        C# + WinUI 2（Windows 专属）
```

**共享**：设计正典 · 控件规格 · Sokuou 曲线 · 命名与 API 惯例。
**不共享**：任何一行实现代码。这是 Kanesumi 的扇区模型 —— 各平台为扇区，共圆心。

设计正典：[Ether `docs/KANESUMI_DESIGN.md`](https://github.com/ProjectArcturius/Ether/blob/main/docs/KANESUMI_DESIGN.md)。
产品名 **Arc Deck**，仓名 `arc-deck`，附文 `based on Kanesumi Design`。

## 提交规范

**Conventional Commits**：`feat:` / `fix:` / `docs:` / `chore:` / `refactor:` /
`test:` / `build:`。**正文用中文**，关键设计决策**必须记录原因** ——
让一年后读 `git log` 不需要开 PR。

**一个 section / 逻辑单元一个 commit**，无需等待用户要求。
**提交 ≠ 推送** —— 推送必须等用户明确指示。

正文惯用标点：`——` 引出根因或理由，`·` 分隔同一提交内的独立改动。
交叉引用用 `参 <doc> §N`，两个版本各自的规则加前缀（如 `参 kanesumi/AGENTS.md`）。

**scope 用版本名或子系统**：`feat(kanesumi):` / `feat(uwp):` / `docs(perf):`。

## 通用铁律

### 1 · 配色与几何从正典取值，不在控件里散写

**注意一处平台分歧，必须显式记录**：正典写「OLED 纯黑 `#000000`」，
Android 扇区严格遵守，而 Ether 桌面扇区用 `#1A1A1A`；强调色 likewise
（`#2E67B5` 蓝 vs `#E57812` 橙金）。**没有任何文档解释这个分歧。**

Arc Deck 两个版本都跟随**桌面扇区**（`#1A1A1A` / `#E57812`）——
一致比各自为政重要。参 `docs/DESIGN_SYSTEM.md`。

### 2 · 正典未覆盖的规格，实测后补进正典

两个版本都会发现正典没写的东西（如 IME 组合串的视觉规格）。
**发现即记录**，落到 `kanesumi/docs/CONTROL_SPEC.md` / `ANIMATION_SPEC.md`，
而不是留在各自的实现里。

### 3 · 处置分歧时以正典为准，不以平台默认为准

平台的默认观感带 Fluent 时代残留，而 Kanesumi 与它的偏离是**刻意**的。
差异分两种，处置不同：

- **正典刻意偏离平台** → 保留正典取值，**补一句偏离理由**
- **正典未覆盖** → 采用平台实测值

> 第一类差异若没有记录，半年后会被当成笔误改掉。

### 4 · 注释写 why，不写 what

关键决策的注释要记录**促成它的那个 bug 或那次实测**，而不是复述代码。
中英混合可接受；标识符、类型、API 一律英文。

### 5 · 不假装做了

未实现的部分（缺的渲染命令、空的裁剪、占位的排版器、跑不起来的部署）
**必须显式记录**。缺失的诚实记录好过假装完整。

---

## `kanesumi/` —— Rust 版本

### K1 · 不用 winit —— IME 必须归自己管

2026 年**所有**纯 Rust GUI 框架的中文 IME 都未完成，根因是**它们不拥有那一层**
（IME 归 winit/SCTK 管，框架只能等；egui 的 macOS IME 至今卡在上游
`rust-windowing/winit#4432`）。Kanesumi 的 Linux 外壳用
`smithay-client-toolkit` + `calloop` 自研事件循环；Windows 侧照此办理，
**自己写 Win32 消息泵**。不要引入 winit，也不要引入任何自管 IME 的框架。

### K2 · 全仓唯一允许裸 FFI 的地方是 `kanesumi/crates/arc-deck-shell/src/platform/win32.rs`

新增 Win32 绑定时加在那里。`platform/mod.rs` 里只应有事件翻译与生命周期 ——
让消息泵逻辑保持可读，FFI 表面收敛到一处可审计。

### K3 · `App` 不持有任何平台句柄

`App` 是纯逻辑，能在没有窗口的环境下单测。这是它能在 Win32 外壳与将来的
Wayland 外壳下复用同一份实现的**唯一前提**。不要为了图方便把 HWND / HDC /
设备指针塞进 `App`。

### K4 · 重绘门控必须中心化

`App::needs_redraw()` 是**唯一**的重绘判据，外壳在消息循环末尾统一检查。
**不要在组件里各自请求重绘** —— 那正是 PezMax-One 踩过的坑（每个动画各自
`request_repaint`，结果一个 GIF 头像就让空闲时全屏 60fps 空转）。

对应地：`ShellState::painting`（当前是否在绘制中）与「是否已排程重绘」是
**两个不同的概念，不可共用一个字段**。早期版本混用二者，导致重绘请求发不出去，
表现为「能唤起输入法但屏幕不更新」。

### K5 · 组合串永远不进入文档

`ImeState` 的四条不变量，每条都有测试，改动时不要破坏：

1. 组合串只存在于 `preedit`，**永远不进入文档**
2. 提交串是唯一会产出 `InputEvent::Commit` 的路径
3. 重新开始组合会清空上一轮（不累积）
4. 取消组合清空组合串但**不产出提交**

另有一条真实陷阱：**`GCS_CURSORPOS` 给的是字节偏移，不是字符偏移。**
组合串含中文时两者不等（「中」占 3 字节）。`byte_to_char_index` 负责换算并在
多字节字符中间向外夹紧 —— 这正是多数框架丢字/乱码的根因。

### K6 · IME 消息必须交回 `DefWindowProcW`，且组合串必须转发给 App

三个 `WM_IME_*` 读完自己要的内容后**必须**交给 `DefWindowProcW`。IME 在组合窗口上
有自己的簿记，吞掉会导致组合无法推进（表现为「能唤起输入法但打不出字」）。

**外壳持有 `ImeState` ≠ App 看得见组合串。** 两件事必须显式打通：更新完
`ImeState` 后要调 `handle_input(Preedit)`；组合结束时发一条**空 preedit**
收起显示，否则界面会残留永不消失的下划线。

### K7 · 诊断计数必须对应被断言的代码路径

`committed` 来自 `ImeState`，而 `WM_CHAR` 路径**不经过** `ImeState`（它直接调
`handle_input`）。用 `committed` 去断言那条路径会得到**假阴性** ——
第一版 `input-probe` 就是这么错的，把一条完好的链路判成「断」。

诊断计数分三层，才能区分「消息没到」「到了没转发」「转发了但没生效」：
`chars_received`（到达 `wnd_proc`）→ `app_events`（转发给 `App`）→ 应用自身状态。

### K8 · `SceneCommand` 只有八个变体，且刻意只有八个

无渐变、无阴影、无圆角：

- **无渐变** —— 正典铁律 6 在命令模型里**结构性**成立（不存在画笔/渐变变体）
- **无阴影** —— 纵深靠明暗，不靠阴影
- **无圆角** —— 直角是几何身份。`FillRect`/`StrokeRect` **没有 radius 字段**；
  若确需 2px，由调用方在 `FillRect` 之上自行叠加，而不是给命令加一个
  99% 调用点都恒为 0 的参数

描边约定**向内**（inside）—— 与「贴边」一致：不侵占相邻元素的像素。

### K9 · 依赖用 Cargo git 依赖 + `rev` 钉死，不用 submodule

Cargo 的 git 依赖天然**按 crate 选择**且支持 `subdirectory`。submodule 是
「整仓一个检出点」的工具 —— 用错工具才会产生「不能选择性 sub」的错觉。
本地改上游时用 `[patch]` 换成本地路径。

**`dsh` 内核版本必须钉死。** 上游协议**无版本号、无能力协商**，且在
0.1.2 → 0.1.6 之间重构了两次。升级是刻意的工程动作，不是自动跟随。

---

## `uwp/` —— C# + WinUI 2 版本

### U1 · 用 WinUI 2，不用 WinUI 3

WinUI 3 是 **Fluent** —— 圆角、Mica/Acrylic、阴影，与 Kanesumi Design **对立**
（正典原话：Fluent 仅作反面教材）。Kanesumi 的技术参考是 **Metro / UWP 时代
（Win8 – Win10 1709）**，对应的是 **WinUI 2**。

若某个控件只在 WinUI 3 里存在，**不要为了它引入 WinUI 3** —— 按 Kanesumi 的
方式自己实现，并把规格记进 `CONTROL_SPEC.md`。

### U2 · 圆角归零在 `App.xaml` 完成，不在控件级零散处理

`ControlCornerRadius` + `OverlayCornerRadius` 在 `App.xaml` 一次性归零。
逐控件设 `CornerRadius="0"` 是重复劳动且必然遗漏 —— 新增控件时会忘。

### U3 · 覆盖系统画刷，而不是另起一套让控件去引用

这样系统控件默认就长成 Kanesumi 的样子，不需要给每个控件写 Style ——
对应正典铁律 1「默认即语言」。键名从 Windows SDK 的 `themeresources.xaml` 查
（`SystemControl*` 系列，**不是** WinUI 3 的 `ControlFillColor*`）。

`ThemeDictionaries` 必须为**每个存在的主题**给出同一组键（深 / 浅 / 高对比），
缺一个会在 XAML 解析期抛异常，表现为启动即崩且日志无消息。
**高对比度刻意不覆盖** —— 那是无障碍设置，优先于视觉一致性。

### U4 · WinUI 2 新增控件走 `muxc:`，系统控件走默认命名空间

| 类别 | 命名空间 | 例子 |
|---|---|---|
| UWP 系统控件 | 默认 | `Button` `TextBox` `ListView` **`AutoSuggestBox`** `Slider` |
| WinUI 2 新增 | `muxc:` | `InfoBar` `Expander` `DropDownButton` `NumberBox` `PipsPager` `NavigationView` |

**分界线只能靠编译器确认**，两个方向的错法都存在。

**另一处同类陷阱**：`NavigationView` 在两个命名空间里都有，事件处理器参数
必须完全限定用 `Microsoft.UI.Xaml.Controls`，否则报「没有与委托匹配的重载」。

### U5 · 用 Release 配置构建

Debug 的 .NET Native 依赖 `Microsoft.NET.Native.Framework.**Debug**.2.2`
（调试运行时通常未预装）。Release 才是 UWP 的发布形态。

平台清单里的两个非默认项**不要删**：`<LangVersion>10.0</LangVersion>`
（UWP 默认 7.3）与 `<UseDotNetNativeToolchain>`（让应用自包含）。

### U6 · UWP 不能直接运行 exe

必须经包注册/安装后由 Activator 启动。排查启动失败时用
`IApplicationActivationManager.ActivateApplication` 的 COM 接口拿真实 HRESULT ——
`Start-Process shell:appsFolder\...` 只说「已请求启动」，失败时给不出原因。

---

## 当前状态

| | 状态 |
|---|---|
| `kanesumi/` 渲染契约 + Win32 外壳 | ✅ 21 个测试通过 |
| `kanesumi/` IME 端到端实测 | ✅ **通过**（微软拼音组合 `zhong` 成功） |
| `kanesumi/` 流式排版性能 | ✅ 已实测，见 `docs/PERF_FINDINGS.md` |
| `kanesumi/` GDI 参考渲染器 | ⚠️ 缺 `Arc`/`Triangle`/`Image`，裁剪为空 |
| `kanesumi/` 文本排版 | ⚠️ `HeuristicTextEngine` 是占位，不得用于最终排版决策 |
| `kanesumi/` 无障碍 | ❌ 完全没有（`Scene` 是纯绘制命令，没有无障碍树） |
| `uwp/` 完整应用（5 页 + NavigationView） | ✅ 编译通过，产出 MSIX |
| `uwp/` 启动运行 | ❌ 受信任链限制，需管理员权限签名部署 |
| 两个版本的无障碍 | ❌ **都是空白** —— 全项目最大缺口 |
| `arc-deck-dsh`（协议客户端） | ❌ 未建立 |

## 构建

```bash
# Rust 版本
cargo test
cargo run --bin arc-deck-demo
cargo run -p layout-bench --release
```

```powershell
# UWP 版本（必须 MSBuild + Release）
$msbuild = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\MSBuild.exe"
& $msbuild uwp\src\Kanesumi.SecW\Kanesumi.SecW.csproj /p:Configuration=Release /p:Platform=x64
```

工具链注意：**Rust 默认工具链是 `gnu`**，而 `--target x86_64-pc-windows-msvc`
在该工具链下不可用（`error[E0463]: can't find crate for 'std'`）。用
`cargo +stable-x86_64-pc-windows-msvc` 或改默认工具链。
