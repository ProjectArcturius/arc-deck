# AGENTS.md — Arc Deck

Arc Deck 是 DeepSeek Harness 的原生桌面客户端，有**两个并列实现**。

```
kanesumi/   Rust + 自绘 Scene（跨平台，无 WebView）   based on Kanesumi Design
uwp/        C# + WinUI 2（Windows 专属）              用微软自家的接口与套件
```

**两者代码不共享，外观也不共享。** 它们是同一个客户端的两条实现路线：

| | `kanesumi/` | `uwp/` |
|---|---|---|
| 外观 | 自定 —— Kanesumi Design | **跟系统走**（唯一例外：圆角归零，见 U3） |
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

**`App.xaml` 只合并 WinUI 2 的控件资源 + 一个 `Themes/Flat.xaml`。**
`Flat.xaml` 只做一件事：把 `ControlCornerRadius` / `OverlayCornerRadius` 归零。
这是**唯一**允许的外观覆盖，理由见 U3。

**不要做的事**（都曾短暂做过，已移除）：

| 不做 | 为什么 |
|---|---|
| 覆盖 `SystemControl*` 画刷 | 那是往平台外观上塞自定义配色，与「跟系统走」矛盾。也别拿 Acrylic 换纯色 —— 那是在放弃平台材质，方向正好反了 |
| 重写 `ListViewItem` 模板 | 平台模板自带虚拟化、选中态、键盘导航、无障碍 —— 重写要自己维护这些 |
| 覆盖图标字体 | 码点语义不通用，只会让写死的字形指错图。见 U3 |
| 构建脚本合成彩色图标 | 图标是设计资产，不该由脚本生成 |

### U2 · 用 WinUI 2，不用 WinUI 3

本应用是 UWP 应用，对应 **WinUI 2**。不要为了某个只有 WinUI 3 才有的控件
而引入 Windows App SDK —— 那是另一套东西，会带来 `dotnet build`、
无 `XamlRoot` 兼容层、不同的打包模型等一系列连锁变化。

### U3 · 外观的边界是「令牌值」—— 不是「模板形状」

这一条是实测出来的，写下来省得下次再试一遍。

**能改的**：控件模板通过 `{ThemeResource}` 引用到的那些令牌。把
`ControlCornerRadius` / `OverlayCornerRadius` 归零就属于这一类 —— 它们是
XAML 框架的**内建默认值**，在 Windows SDK 的 `themeresources.xaml` 与 WinUI 2
的资源里都**查不到定义**，模板只是引用它们，所以只能在应用资源层覆盖。

**改不了的**：WinUI 2 的控件模板本身。它编译进包里，Win10 那份无法在应用层
替换。能改的只有模板引用的令牌 —— **只能改「值」，不能改「形状」**。

**反直觉的一条实测**：Windows 11 SDK 的 `themeresources.xaml` 里配色
**本来就是旧的**。`NavigationView` 系列仍指向 2015 年那套 legacy UWP 令牌：

```
NavigationViewItemBackgroundSelected -> SystemControlHighlightListLowRevealBackgroundBrush
  -> SystemListLowColor = #19FFFFFF
NavigationViewDefaultPaneBackground  -> SystemControlChromeMediumLowAcrylicElementMediumBrush
NavigationViewSelectionIndicatorForeground -> SystemControlForegroundAccentBrush
```

这些值在 Win10 与 Win11 上**逐字相同**。所以「Fluent 味」不来自配色表，
而来自控件模板本身（圆角、间距、图标字形、材质）。**改色改不动观感**，
还会破坏主题跟随 —— 不要往这个方向做。

**图标字体也不能覆盖。** 曾把 `SymbolThemeFontFamily` 改成
`"Segoe MDL2 Assets"`（Win10 图标字体），是**错的**，已撤销：

- 两套字体的**码点语义不通用**。实测 U+E8B7 在 Segoe Fluent Icons 里是
  「标签文件夹」，在 Segoe MDL2 Assets 里**不是文件夹**。一旦换字体，
  代码里每个写死的 `Glyph="&#xE8NN;"` 都会悄悄指向另一张图
- `SymbolIcon Symbol="Back"` 由框架在运行时解析码点，字体只是渲染层，
  覆盖 `SymbolThemeFontFamily` 对它们**无效**。即使覆盖生效也只能改到一部分
  图标，结果是两套字形混在一起，比不覆盖更难看

**据此的写法约定**：图标优先用 `SymbolIcon` + 枚举名（`Symbol="Folder"`），
**不要**用 `FontIcon` + 码点 —— 前者与字体解耦，后者换字体就指错图。

### U4 · 控件命名空间：新增控件走 `muxc:`，系统控件走默认

| 类别 | 命名空间 | 例子 |
|---|---|---|
| UWP 系统控件 | 默认 | `Button` `TextBox` `ListView` `AutoSuggestBox` `Slider` `CommandBar` |
| WinUI 2 新增 | `muxc:` | `InfoBar` `Expander` `DropDownButton` `NumberBox` `RatingControl` `PipsPager` `NavigationView` `TeachingTip` |

**分界线只能靠编译器确认**，两个方向的错法都存在（给系统控件加 `muxc:`
会报错，给新增控件用默认命名空间也会报错）。

**同类陷阱**：`NavigationView` 在两个命名空间里都有，事件处理器参数必须
完全限定用 `Microsoft.UI.Xaml.Controls`，否则报「没有与委托匹配的重载」。

### U5 · 用平台现成的控制件，不自造

对话页用 `ContentDialog` 而不是自己拼浮层；列表用 `ListView` +
`DataTemplateSelector` 而不是手写分派逻辑；右键菜单用 `MenuFlyout` 而不是
自己画。

**理由**：这些控件自带键盘提示、焦点管理、无障碍语义与触摸手势。
自己重造要维护的正是这些东西 —— 而「用平台套件」的全部好处就在这里。

**例外见 U9**：`CommandBar` 在这个布局下会把按钮排出窗口，已改用
`StackPanel` + `Button`。那是有实测依据的例外，不是「不想用平台控件」。

### U6 · 用 Release 配置构建

Debug 的 .NET Native 依赖 `Microsoft.NET.Native.Framework.**Debug**.2.2`
（调试运行时通常未预装）。Release 才是 UWP 的发布形态。

csproj 里两个非默认项**不要删**：`<LangVersion>10.0</LangVersion>`
（UWP 默认 C# 7.3）与 `<UseDotNetNativeToolchain>`（让应用自包含）。

### U7 · UWP 不能直接运行 exe

必须经包注册/安装后由 Activator 启动。排查启动失败时用
`IApplicationActivationManager.ActivateApplication` 的 COM 接口拿真实 HRESULT ——
`Start-Process shell:appsFolder\...` 只说「已请求启动」，失败时给不出原因；
事件日志的 `.NET Runtime` 条目也不含异常消息。

### U8 · 本目录与 Kanesumi 无关

UWP 版**不是** Kanesumi Design 的实现。它用微软的控件、微软的配色
（系统主题 + 强调色）、微软的排版刻度。

`../kanesumi/` 才是 `based on Kanesumi Design` 的那一支。
两者的关系只是「同一个客户端的两条实现路线」，**代码与外观都不共享**。
不要把 Kanesumi 的色值、圆角策略、控件规格搬到这里来。

### U9 · 不要用 `CommandBar` 承载本应用的页面工具条

**实测**：两页的工具按钮被排到窗口外，点不到。窗口 1038..1550 宽时的元素坐标：

```
会话页   新会话 1309 / 刷新 1394 / 全选 1479 / 溢出 1539    窗口止于 1278
对话页   复制 1359 / 重载 1444                              窗口止于 1278
```

把标题放进 `CommandBar.Content` 时如此；**去掉 Content、只留 PrimaryCommands
也一样**被排到 x=1689 起。用 `uwp/tools/shot/Region.ps1` 量过那两块区域：
亮像素数为 **0**，确认不是抓图伪影。

**最阴的一点**：UIA 对它们仍报 `visible`，所以只看自动化会把「没画出来」
判成「正常」。这类「树里有、屏幕上没有」的偏差只能靠像素测量发现。

**做法**：用一排 `Button`（图标 + `ToolTipService.ToolTip`）+ 一个带
`MenuFlyout` 的溢出按钮，标题另起一行。每个按钮都有文本 `Label` 语义
（用 `TextBlock` 或 tooltip），键盘可达性与无障碍都不缺。

以后要 `DefaultLabelPosition` 之类的 `CommandBar` 专属行为时再回到它，
但**先按上面的方式量一遍坐标**。

### U10 · 主题的已知局限：`NavigationView` 窗格背景不跟随

`RequestedTheme` 的应用时机与作用对象，两处都踩过：

- 应用到 `Window.Content`（那个 `Frame`）无效 —— `NavigationView` 是它的
  **父级**，而 `RequestedTheme` 在视觉树上只向下传播
- 必须在 `InitializeComponent()` **之前**设，新页面才能带正确主题创建

**实测结果**：内容区正确切换，并且**跨重启持久化**（浅色 `#808080` 保持）。
但 `NavigationView` 的**窗格背景不跟随**（仍是 `#000000`）—— 它是平台在加载期
解析的 `AcrylicBrush`（`NavigationViewDefaultPaneBackground`，SDK 的
themeresources.xaml 用 `StaticResource` 指向），事后改 `RequestedTheme` 不会让
它重新解析。

**不修，并明确记为局限而非已完成的功能。** 唯一的修法是覆盖那个平台画刷，
而 U1 禁止往平台外观上塞自定义配色 —— 那会把「跟系统走」这条路走死。
浅色主题下窗格仍暗，是当前可接受的取舍。

---

## 当前状态

**区分「实测通过」与「只是编译通过」。** 两者都写了才算如实 —— 编译通过不等于
行为正确，而下面带 ⚠️ 的那些恰好是最容易出错的交互与持久化。

| | 状态 |
|---|---|
| `kanesumi/` 渲染契约 + Win32 外壳 | ✅ 21 个测试通过 |
| `kanesumi/` IME 端到端实测 | ✅ **通过**（微软拼音组合 `zhong` 成功） |
| `kanesumi/` 流式排版性能 | ✅ 已实测，见 `docs/PERF_FINDINGS.md` |
| `kanesumi/` GDI 参考渲染器 | ⚠️ 缺 `Arc`/`Triangle`/`Image`，裁剪为空 |
| `kanesumi/` 文本排版 | ⚠️ `HeuristicTextEngine` 是占位，不得用于最终排版决策 |
| `kanesumi/` 无障碍 | ❌ 完全没有（`Scene` 是纯绘制命令，没有无障碍树） |
| `uwp/` 编译与打包 | ✅ 从零 Rebuild 无错误无警告，产出 MSIX（0.1.1.0） |
| `uwp/` 启动运行 | ✅ **已实测**（签名 → 覆盖安装 → 启动，无 crash.log） |
| `uwp/` 对话页块渲染（段落/代码/工具卡） | ✅ **已实测**，逐块截图核对 |
| `uwp/` 会话列表 + 新建会话 | ✅ **已实测**（点「新会话」正确进入新会话，标题/轮数正确） |
| `uwp/` 会话重命名 / 置顶 / 删除 | ⚠️ 仅编译通过 —— 右键与多选路径**未实测** |
| `uwp/` 工具页（工具清单 + 详情） | ✅ **已实测**（10 个工具、4 个「需确认」、详情三行 + 说明齐备） |
| `uwp/` 设置持久化（主题） | ✅ **已实测**：内容区切换 + 跨重启保持（浅色 `#808080`） |
| `uwp/` 主题对导航窗格 | ⚠️ **已知局限**：窗格背景不跟随（U10） |
| `uwp/` 搜索框筛选会话 | ⚠️ 未实测 |
| `uwp/` 外观 | ⚠️ 只做到圆角归零。图标字形与控件模板形状改不动，见 U3 |
| 无障碍 | `kanesumi/` ❌ 没有（自绘路线最大缺口）· `uwp/` ✅ 平台免费提供 |
| `arc-deck-dsh`（协议客户端） | ❌ 未建立 |
| `uwp/` 数据 | ⚠️ 全部是示例数据（`Models/Store.cs`、`Store.Tools`），未接协议 |

### 还没实测的部分

上表标 ⚠️ 的项，含义是**代码写完了、能编译，但没有任何一次运行证明它行为正确**。
目前还没跑到的：

- 会话**右键菜单**是否命中被右键的那一行（`SessionListView` 存在的唯一理由 ——
  这个错只有在「先点 A 再右键 B」时才会暴露，必须专门测）
- 重命名 / 置顶 / 删除 / 多选批量操作
- 搜索框筛选是否实时生效
- 对话页发送消息后列表摘要与轮数是否更新

要确认它们，按「构建」一节部署后逐项走一遍。**在有人做过之前，
不要把这些写成「已跑通」。**

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

# 不跑程序时，唯一可用的检查是从零重建 —— 它会重新编译 XAML，
# 增量构建可能漏掉 XAML 层面的错误。
& $msbuild uwp\src\ArcDeck.csproj /t:Rebuild /p:Configuration=Release /p:Platform=x64

# 部署：建证书 → 信任 → 签名 → 验证，一条命令给明确结论
dotnet run --project uwp\tools\deploy-check
Add-AppxPackage -Path uwp\src\AppPackages\**\*_x64.msix
```

**改完代码要提版本号再装。** 同版本号但内容不同的 MSIX 会被部署引擎拒装
（`0x80073CFB`：「提供的程序包已安装，且禁止重新安装该程序包」）。
如果每次都卸载重装，`LocalSettings` 会一并清掉 —— 于是「设置是否持久化」
这类测试**永远测不出来**（会一直显示默认值，误判成没实现）。

所以流程是：改 `Package.appxmanifest` 的 `Version` → 构建 → 签名 →
**直接 `Add-AppxPackage` 覆盖安装**（不要先 `Remove-AppxPackage`）。

```powershell
# 看界面：驱动 + 截图（必须用 Windows PowerShell 5.1，不是 pwsh）
$ps5 = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
& $ps5 -File uwp\tools\shot\Print.ps1 -Out shot.png     # 首选：不看焦点不看 Z 序
& $ps5 -File uwp\tools\shot\Uia.ps1 -List               # 元素树
& $ps5 -File uwp\tools\shot\Uia.ps1 -Invoke "会话" -Out shot.png
```

**别用肉眼判断界面改没改。** `uwp/tools/shot/` 下的脚本是为了把这类判断做成
可测量的：`ImgDiff.ps1` 做对照实验的差异分布，`GlyphDiff.ps1` 比字形，
`ZoomRect.ps1` 放大真实像素。**声明「改好了」之前先出证据。**

抓图的坑（都在脚本注释里）：全屏截图会抓到前台的程序，而**抓错窗口是静默失败** ——
你会得到另一张应用的截图，然后据此判断「界面改坏了」。首选 `Print.ps1`
（`PrintWindow` + `PW_RENDERFULLCONTENT`，不看焦点也不看 Z 序）。

**别用 `Add-AppxPackage -Register` 装松散文件** —— 能注册成功但启动报
`0x8027025B`。走「签名 MSIX → 安装」才是正路。

签名有两处易错，都记在 `uwp/README.md`：证书主体必须与清单 `Publisher`
**逐字一致**；证书要装进 `CurrentUser\Root`（只装 `TrustedPeople` 不够）。
另注：PowerShell 的 `Import-Certificate` 写 `Root` 会报 "UI is not allowed"，
但 **.NET 的 `X509Store` API 直接写就成功，不需要管理员** —— 那条报错是
cmdlet 自身的限制而非权限问题。

工具链注意：**Rust 默认工具链是 `gnu`**，而 `--target x86_64-pc-windows-msvc`
在该工具链下不可用（`error[E0463]: can't find crate for 'std'`）。用
`cargo +stable-x86_64-pc-windows-msvc` 或改默认工具链。
