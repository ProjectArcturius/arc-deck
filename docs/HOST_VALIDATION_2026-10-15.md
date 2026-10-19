# 完整信任桌面宿主 + WinUI 2 / XAML Islands：宿主验证（2026-10-15）

PRODUCT_PLAN §8 把这件事列为 GUI 的前置验证：**先证明完整信任的桌面宿主能用 WinUI 2 的控件**，
再谈把 `uwp/` 的界面接上去。本文件只记这次验证的**做法、原文证据与结论**，不写计划。

一句话：**「完整信任宿主」成立，「系统 XAML island」成立，「WinUI 2 控件在那个 island 里」不成立**
（至少在不引入 Windows App SDK 的前提下不成立）。用户已裁定走 §8 的备选 A
（UWP 界面 + 完整信任伴随进程），通道已实现并实测，见第七节。

## 一、验证环境

| 项 | 值 |
|---|---|
| OS | Windows 11（`Microsoft Windows NT 10.0.26100`） |
| .NET SDK | 9.0.316；运行时 6/8/9 |
| .NET Framework | 4.8 运行时在；**targeting pack 未安装**，用 `Microsoft.NETFramework.ReferenceAssemblies` NuGet 补 |
| Windows SDK | 10.0.22621.0、10.0.26100.0 |
| MSBuild | VS 2022 BuildTools 17.14（**必须用它，不能用 `dotnet build`**，见 三·4） |
| WinUI 2 框架包 | `Microsoft.UI.Xaml.2.8` x64 `8.2511.26001.0` 已安装（UWP 版带进来的） |
| 探针工程 | 验证时在 `D:\arcdeck\spike-host`（**仓库外**，验证后已删除）；本文给出复现所需的全部非默认设置 |

## 二、结论表

| 问题 | 结果 | 证据 |
|---|---|---|
| 完整信任宿主能起子进程吗 | ✅ | WPF 按钮起 `cmd.exe`，经 UIA 点按钮后状态行读到 `child: full-trust-ok` |
| 能读包外路径吗（P1 的三条里第二条） | ✅ | `Environment.SpecialFolder.ApplicationData` → `C:\Users\mc158\AppData\Roaming`，`exists=True` |
| UIA 能看见 island 里的 XAML 内容吗 | ✅ | 树里出现 `Pane/Windows.UI.Input.InputSite.WindowClass` → `Edit/TextBox`（在 `HwndHost` 下） |
| 系统 XAML（`Windows.UI.Xaml`）的 island 能起来吗 | ✅ | 上面那棵 UIA 树就是 system XAML 的 `TextBox`；窗口存活、无未处理异常 |
| **WinUI 2 控件能在 island 里激活吗** | ❌ | `XamlApplication..ctor` 抛 `FileNotFoundException: 找不到指定的模块 (0x8007007E)` |
| 现代 .NET（5+）能引用 toolkit 的 XamlHost 吗 | ❌ | 编译期 `NETSDK1149`（原文见三·1） |
| 像素证据（截图） | ⚠️ 没拿到 | 全屏游戏挡住屏幕（`SetForegroundWindow` 被忽略），`PrintWindow` 抓不到 island 的合成层 —— 仓库 `uwp/tools/shot/Print.ps1` 早已记录过前者 |

## 三、四条硬结论（都带原文）

### 1 · .NET 5+ 不能引用 Windows Community Toolkit 的 XamlHost

`net9.0-windows10.0.22621.0` + `Microsoft.Toolkit.Wpf.UI.XamlHost 6.1.2`：

```
error NETSDK1149: 无法引用 Microsoft.Toolkit.Wpf.UI.XamlHost.dll，
因为它使用了对 WinRT 的内置支持，而 .NET 5 和更高版本中不再支持它。
```

包本身只有 `net462` 与 `netcoreapp3.0` 两个 lib；`netcoreapp3.0` 那份依赖的
「内置 WinRT」在 .NET 5 被移除。**所以「宿主 = 现代 .NET + WinUI 2 island」这条路在编译期就断了**，
不是运行时调参能救的。可用的宿主 TFM 只剩 `net48`。

### 2 · `DesktopWindowXamlSource` 要求清单里有 `maxversiontested`

没有时的报错极具误导性（堆栈里只有 WinRT 调用，看起来像 DLL 缺失）：

```
灾难性故障
WindowsXamlManager and DesktopWindowXamlSource are supported for apps targeting
Windows version 10.0.18226.0 and later. Please check either the application manifest
or package manifest and ensure the MaxTestedVersion property is updated.
```

`app.manifest` 里加 `<maxversiontested Id="10.0.22621.0"/>` 即解决。

### 3 · 清单里**不能**再写一份 DPI 设置

.NET SDK 已经注入 `dpiAware` / `dpiAwareness`；manifest 里再写一份会得到重复元素，
**链接成功、启动时报**「应用程序的并行配置不正确」（`0x800736B1`）。
这一条踩过两次：第一次以为是 WinRT 组件缺失，其实是清单非法。

### 4 · WinUI 2 控件需要 `XamlApplication`，而它在未打包宿主里起不来

toolkit 的 `XamlApplication` 构造函数是 `protected`（`CS0122`），必须派生：

```csharp
public sealed class HostXamlApp : Microsoft.Toolkit.Win32.UI.XamlHost.XamlApplication { }
```

派生后编译通过，运行时在 `XamlApplication..ctor` 里失败：

```
System.IO.FileNotFoundException: 找不到指定的模块。 (异常来自 HRESULT:0x8007007E)
   在 System.StubHelpers.StubHelpers.GetWinRTFactoryObject(IntPtr pCPCMD)
   在 Microsoft.Toolkit.Win32.UI.XamlHost.XamlApplication..ctor()
```

已排除的、**不是**原因的三件事：

- `Microsoft.UI.Xaml.dll` 与 `resources.pri` 已从 NuGet 的 `Microsoft.UI.Xaml.2.8.appx` 里取出来放到 exe 旁；
- `Microsoft.Toolkit.Win32.UI.XamlHost.dll`（native）与其 `.pri` 已从其 NuGet 的 `runtimes\win10-x64\native` 复制到 exe 旁；
- 已按 framework 包的 `AppxManifest.xml` 生成了 **136 条** `activatableClass`，写进 `app.manifest`
  （否则激活错误会是 `REGDB_E_CLASSNOTREG`，不是 `0x8007007E`）。

`0x8007007E` 指向 native 依赖链里还有一环没满足（framework 包自带的依赖 / 注册式 WinRT 的激活上下文）。
**要在未打包的 .NET Framework 宿主里补齐它，等于自己维护 WinUI 2 的部署清单**：
上游没有为「未打包桌面宿主」提供这条路径，toolkit 的 targets 只为打包场景生成 `AppxPackageRegistration`。

## 四、复现方式（仓库外探针）

```powershell
# 1) 工程：net48 + TPV，两个包 + WinUI 2 的 winmd 引用（见 Spike.csproj）
# 2) 必须用完整 MSBuild，不能用 dotnet build：
#    dotnet build 走 MSBuild Core，toolkit 的 CodeTaskFactory 任务会报 MSB4801。
$msbuild = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\MSBuild.exe"
& $msbuild Spike.csproj /t:Rebuild /p:Configuration=Release /p:Platform=AnyCPU /p:_DisableAppxCopy=true

# 3) 系统 XAML（Windows.UI.Xaml.Controls.TextBox）：只需 app.manifest 的 maxversiontested
# 4) WinUI 2：再复制 Microsoft.UI.Xaml.dll / resources.pri / XamlHost native dll，
#    生成 136 条 activatableClass，并在 App.OnStartup 里派生 XamlApplication
```

UIA 检查（PowerShell 5.1，`UIAutomationClient`）：从 `MainWindowHandle` 取
`AutomationElement.FromHandle`，向下遍历 `ControlViewWalker`。
系统 XAML island 的树形是：

```
Window / MainWindowHandle
└ Pane | Windows.UI.Input.InputSite.WindowClass
  └ Edit | TextBox          ← island 里的内容
```

## 五、对 §8 的影响与建议

§8 的先决验证要求「关键控件、主题、中文输入、UIA、资源、打包和启动均须运行检查」。
本次只走到「UIA + 完整信任 + island 生命周期」，**WinUI 2 控件这一项失败**，所以：

1. **原方案（完整信任宿主 + WinUI 2 / XAML Islands）不可行** —— 除非接受：
   - 宿主固定 `net48`（现代 .NET 不行）；
   - 自己维护 WinUI 2 的未打包部署（136 条激活项 + native 依赖），且目前还没补齐；
   - 长期看上游已把这条线标为 legacy（微软现行文档的 XAML Islands 指 **Windows App SDK / WinUI 3**）。
2. **备选 A（§8 已写好的那条）**：保留 `uwp/` 的 UWP 界面，加一个**完整信任伴随进程 / AppService**。
   界面资产与实测经验全部保留，代价是 IPC 与进程生命周期；UWP 侧仍受 P1 的三条沙箱限制，
   但伴随进程负责起 `dsh`、读 `%APPDATA%`，界面只消费它给的数据。
3. **备选 B**：宿主改用 **Windows App SDK / WinUI 3**（完整信任、现代 .NET、官方维护的 islands）。
   代价与 U2「不擅自换 WinUI 3」冲突，**必须由用户裁定**；控件外观也要重新按 WinUI 3 做一遍。

我的建议顺序：**先做备选 A**（不推翻已实测的 UWP 界面，也不动 U2），
把 WinUI 3 当作「A 走不通时」的备选。**用户 2026-10-15 已裁定备选 A**，见第七节。

**结论**：§8 的原方案（完整信任宿主 + WinUI 2 / XAML Islands）**不可行**。
用户随后裁定走 **§8 的备选 A：保留 UWP 界面 + 完整信任伴随进程**；通道已实现并实测，
见第七节。

## 六、这次**没有**验证到的（指 WinUI 2 / XAML Islands 这条线）

- **像素**：island 真的画出来了吗 —— 只有 UIA 树与存活证据，没有截图（原因见二表）。
- **中文输入法**：没有在 island 的 TextBox 里真打中文。
- **主题跟随 / 资源 / MSIX 打包 / 启动**：都没做。
- **WinUI 3 islands**：完全没试。

在用户裁定之前，这些缺口不应当被读成「已经通过」。

## 七、备选 A：UWP 界面 + 完整信任伴随进程（已实现并实测）

### 结构

```
UWP 界面（AppContainer，窗格/输入/Markdown 全部保留）
   │  AppService `arcdeck.host`（UWP 是 provider —— 服务必须由包自己声明）
   ▼
ArcDeck.Host.exe（包内完整信任进程：起 dsh、读 %APPDATA%、以后的本地服务都在这半边）
```

方向容易写反：**完整信任进程不能当 AppService 的 provider**。所以 UWP 是 provider，
伴随进程用自己的包身份 `OpenAsync` 连回来；之后界面 `SendMessageAsync` 发请求，
它 `SendResponseAsync` 应答。连接由 `FullTrustProcessLauncher` 拉起进程、进程反向连上来。

### 证据（2026-10-15，装机实测）

设置页点「拉起并检查」后，UWP 界面自己显示：

```
已连接
ping → {"version":"0.1.0","pid":23180,"packaged":true,
        "packageFamilyName":"ProjectArcturius.ArcDeck_yk0f0xf11xns4",
        "packageFullName":"ProjectArcturius.ArcDeck_0.1.18.0_x64__yk0f0xf11xns4", ...}
env  → {"appData":"C:\\Users\\mc158\\AppData\\Roaming", ...}
probe → {"found":true,"command":"dsh --version","exitCode":0,"stdout":"0.1.7-rc.2"}
```

`probe` 是**真的起了一个子进程**（`dsh --version`），`env` 是**真的读到了包外的
`%APPDATA%`** —— P1 里挡死 UWP 的两条限制都由这半边绕开了。进程侧日志（`%TEMP%\arcdeck-host.log`）
与 UWP 侧日志（`LocalState\host.log`）一一对得上。

生命周期也实测过：关掉 UWP 后伴随进程收到 `服务关闭：Completed` 并退出。
（第一版不退，结果下一次装包被它锁文件，部署报 `0x80073D02`。）

### 实现里三个非显然的点

1. **`FullTrustProcessLauncher` 要显式引用 `Windows.ApplicationModel.FullTrustAppContract`**。
   UWP 的 NuGet 引用集停在 15138，而这个 API 是 1809（17763）才有的；不引用就报
   `CS0103: 当前上下文中不存在名称 FullTrustProcessLauncher`，看起来像拼错了类名。
2. **AppService 不保证调用方是本包**。`OnBackgroundActivated` 里校验
   `CallerPackageFamilyName`，不是本包就记崩溃日志并拒绝 —— 这个服务能起进程、读包外路径。
3. **线协议不要用 `System.Text.Json`**：UWP 侧没有这个库（.NET Native 的框架里没有）。
   走 `ValueSet` 的扁平键（`id` / `op` / `payload` 进，`ok` / `data` / `error` 出），
   `data` 是 JSON 字符串，UWP 用 `Windows.Data.Json` 解析。

### 现在还没接的

界面上的会话/对话仍然是示例数据。通道与「拉起、探活、执行」已经实测，
但把真实会话、流式正文、取消接到 `Store` / `ChatPage` 是下一步，不是已完成。

