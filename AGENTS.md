# AGENTS.md — Arc Deck

Arc Deck 是 DeepSeek Harness 的原生桌面客户端，有**两个并列实现**。

```
kanesumi/   Rust + 自绘 Scene（跨平台，无 WebView）   based on Kanesumi Design
uwp/        C# + WinUI 2（Windows 专属）              用微软自家的接口与套件
```

**两者代码不共享，外观也不共享。** 它们是同一个客户端的两条实现路线：

| | `kanesumi/` | `uwp/` |
|---|---|---|
| 外观 | 自定 —— Kanesumi Design | **跟系统走**，不覆盖任何主题资源 |
| 理由 | 跨平台一致性 | 用平台现成的基础设施 |

产品名 **Arc Deck**，仓名 `arc-deck`。
设计正典（**只与 `kanesumi/` 相关**）：
[Ether `docs/KANESUMI_DESIGN.md`](https://github.com/ProjectArcturius/Ether/blob/main/docs/KANESUMI_DESIGN.md)。

> 不要把 Kanesumi 的东西搬进 `uwp/`，也不要拿 `uwp/` 的平台行为去改
> Kanesumi 的规格。两者的**唯一**交集是「同一个客户端」这个目标。

## 提交规范

**Conventional Commits**：`feat:` / `fix:` / `docs:` / `chore:` / `refactor:` /
`test:` / `build:`。**正文用中文**，关键设计决策**必须记录原因** ——
让一年后读 `git log` 不需要开 PR。

**一个 section / 逻辑单元一个 commit**，无需等待用户要求。
**提交 ≠ 推送** —— 推送必须等用户明确指示。

正文惯用标点：`——` 引出根因或理由，`·` 分隔同一提交内的独立改动。
交叉引用用 `参 <doc> §N`。

**scope 用版本名或子系统**：`feat(kanesumi):` / `feat(uwp):` / `docs(perf):`。

## 通用铁律

### 1 · 注释写 why，不写 what

关键决策的注释要记录**促成它的那个 bug 或那次实测**，而不是复述代码。
中英混合可接受；标识符、类型、API 一律英文。

### 2 · 不假装做了

未实现的部分（缺的渲染命令、空的裁剪、占位的排版器、跑不起来的部署）
**必须显式记录**。缺失的诚实记录好过假装完整。

### 3 · 两条路线的取舍要如实写出来

写文档时不要把 `kanesumi/` 说成「更正统」或把 `uwp/` 说成「临时方案」。
它们各有明确的适用面：**跨平台一致性** vs **平台现成基础设施**。


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

### U1 · 用微软自家的接口与套件 —— 不往平台外观上动手

这是一个**普通、地道的 UWP 应用**：C# + XAML，WinUI 2 控件，
外观完全交给 Windows。

**`App.xaml` 只合并 WinUI 2 的控件资源，不覆盖任何主题资源。**
控件长什么样由系统决定 —— 用户换了主题或强调色，应用跟着变。

**不要做的事**（都曾短暂做过，已移除）：

| 不做 | 为什么 |
|---|---|
| 覆盖 `SystemControl*` 画刷 | 那是往平台外观上塞自定义配色，与「跟系统走」矛盾 |
| 覆盖 `ControlCornerRadius` / `OverlayCornerRadius` | 圆角由系统主题决定 |
| 重写 `ListViewItem` 模板 | 平台模板自带虚拟化、选中态、键盘导航、无障碍 —— 重写要自己维护这些 |
| 构建脚本合成彩色图标 | 图标是设计资产，不该由脚本生成 |

### U2 · 用 WinUI 2，不用 WinUI 3

本应用是 UWP 应用，对应 **WinUI 2**。不要为了某个只有 WinUI 3 才有的控件
而引入 Windows App SDK —— 那是另一套东西，会带来 `dotnet build`、
无 `XamlRoot` 兼容层、不同的打包模型等一系列连锁变化。

### U3 · 控件命名空间：新增控件走 `muxc:`，系统控件走默认

| 类别 | 命名空间 | 例子 |
|---|---|---|
| UWP 系统控件 | 默认 | `Button` `TextBox` `ListView` `AutoSuggestBox` `Slider` `CommandBar` |
| WinUI 2 新增 | `muxc:` | `InfoBar` `Expander` `DropDownButton` `NumberBox` `RatingControl` `PipsPager` `NavigationView` `TeachingTip` |

**分界线只能靠编译器确认**，两个方向的错法都存在（给系统控件加 `muxc:`
会报错，给新增控件用默认命名空间也会报错）。

**同类陷阱**：`NavigationView` 在两个命名空间里都有，事件处理器参数必须
完全限定用 `Microsoft.UI.Xaml.Controls`，否则报「没有与委托匹配的重载」。

### U4 · 用平台现成的控制件，不自造

对话页用 `CommandBar` 而不是自己摆按钮；对话框用 `ContentDialog` 而不是
自己拼浮层；列表用 `ListView` + `DataTemplateSelector` 而不是手写分派逻辑。

**理由**：这些控件自带溢出菜单、键盘提示、焦点管理、无障碍语义与触摸手势。
自己重造要维护的正是这些东西 —— 而「用平台套件」的全部好处就在这里。

### U5 · 用 Release 配置构建

Debug 的 .NET Native 依赖 `Microsoft.NET.Native.Framework.**Debug**.2.2`
（调试运行时通常未预装）。Release 才是 UWP 的发布形态。

csproj 里两个非默认项**不要删**：`<LangVersion>10.0</LangVersion>`
（UWP 默认 C# 7.3）与 `<UseDotNetNativeToolchain>`（让应用自包含）。

### U6 · UWP 不能直接运行 exe

必须经包注册/安装后由 Activator 启动。排查启动失败时用
`IApplicationActivationManager.ActivateApplication` 的 COM 接口拿真实 HRESULT ——
`Start-Process shell:appsFolder\...` 只说「已请求启动」，失败时给不出原因；
事件日志的 `.NET Runtime` 条目也不含异常消息。

### U7 · 本目录与 Kanesumi 无关

UWP 版**不是** Kanesumi Design 的实现。它用微软的控件、微软的配色
（系统主题 + 强调色）、微软的排版刻度。

`../kanesumi/` 才是 `based on Kanesumi Design` 的那一支。
两者的关系只是「同一个客户端的两条实现路线」，**代码与外观都不共享**。
不要把 Kanesumi 的色值、圆角策略、控件规格搬到这里来。

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
& $msbuild uwp\src\ArcDeck.csproj /t:Restore /p:Configuration=Release /p:Platform=x64
& $msbuild uwp\src\ArcDeck.csproj /p:Configuration=Release /p:Platform=x64
```

工具链注意：**Rust 默认工具链是 `gnu`**，而 `--target x86_64-pc-windows-msvc`
在该工具链下不可用（`error[E0463]: can't find crate for 'std'`）。用
`cargo +stable-x86_64-pc-windows-msvc` 或改默认工具链。
