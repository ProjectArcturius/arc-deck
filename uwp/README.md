# Arc Deck · UWP 版

> DeepSeek Harness 的 Windows 客户端，用**微软自家的接口与套件**构建。

## 这是什么

一个**普通、地道的 UWP 应用**：

- **C# + XAML**，`.NET Native` 编译，产出 MSIX
- **WinUI 2**（`Microsoft.UI.Xaml` 2.8.6）提供的控件
- 外观**完全交给 Windows**：主题、强调色、圆角、动效全部跟系统走

## 有意为之的克制

`App.xaml` 里只合并 WinUI 2 的控件资源，**不覆盖任何外观**：

```xml
<ResourceDictionary.MergedDictionaries>
    <XamlControlsResource xmlns="using:Microsoft.UI.Xaml.Controls" />
</ResourceDictionary.MergedDictionaries>
```

控件长什么样由系统决定。用户换了主题或强调色，应用跟着变 ——
这是 UWP 应用该有的行为，也是「用平台套件」的实际含义。

**不做的事**（都曾短暂做过，已移除）：

| 移除 | 原因 |
|---|---|
| 覆盖 `SystemControl*` 画刷 | 那是给应用塞自定义配色，与「跟系统走」矛盾 |
| 覆盖 `ControlCornerRadius` / `OverlayCornerRadius` | 圆角由系统主题决定 |
| 重写 `ListViewItem` 模板 | 平台模板自带虚拟化、选中态、键盘导航、无障碍 —— 重写要自己维护这些 |
| `tools/make-icons` 生成彩色图标 | 图标是设计资产，不该由构建脚本合成 |

## 与 `../kanesumi/` 的关系

同一个客户端的两条实现路线，**代码不共享**：

| | `../kanesumi/` | **本目录** |
|---|---|---|
| 语言 | Rust | C# + XAML |
| 界面 | 自绘 `Scene` 渲染命令 | WinUI 2 控件 |
| 外壳 | 自研 Win32 事件循环 | UWP AppContainer |
| 平台 | Windows 优先，随后 Linux | **Windows 专属** |
| 文本编辑 / 滚动 / 命中测试 | 要自己写 | **平台免费给** |
| 无障碍 | 没有 | **平台免费给** |
| 跨平台 | ✅ | ❌ |

**要跨平台走 `kanesumi/`；只要 Windows、且想用平台现成的基础设施，走这里。**

## 界面结构

```
ShellPage          NavigationView 外壳（含响应式窗格模式 + 搜索框）
├── SessionsPage   会话列表（ListView + 搜索筛选）
├── ChatPage       对话页（CommandBar / 消息模板分派 / 工具卡片 / 输入区）
├── WorkspacesPage 工作区归类
├── ToolsPage      WinUI 2 控件一览（InfoBar / NumberBox / RatingControl /
│                   PipsPager / Expander / DropDownButton / SplitButton /
│                   BreadcrumbBar / ContentDialog / TeachingTip / Flyout）
└── SettingsPage   设置（主题切换 + 关于）
```

数据在 `Models/`，当前是示例数据 —— 接 DSH 协议是后续的事。

## 完整信任伴随进程（`uwp/host/`）

UWP 跑在 AppContainer 里，**起不了 `dsh` 子进程、读不到 `%APPDATA%`**（AGENTS P1 的三条限制，
改代码绕不过去）。所以执行与文件访问交给包内一个完整信任的伴随进程 `ArcDeck.Host.exe`，
界面只通过 AppService 向它发请求。

```
UWP 界面（AppContainer）── AppService `arcdeck.host` ──▶ ArcDeck.Host.exe（完整信任）
```

方向容易写反：**完整信任进程不能当 AppService 的 provider**。UWP 是 provider（服务必须由包声明），
伴随进程用自己的包身份 `OpenAsync` 连回来，界面再 `SendMessageAsync` 发请求、它 `SendResponseAsync` 应答。
进程由 `FullTrustProcessLauncher` 拉起。

### 构建

**先发布伴随进程，再构建 UWP 包** —— 顺序反了的话包能装上，但 `FullTrustProcessLauncher`
会报找不到可执行文件：

```powershell
powershell -File uwp\host\publish.ps1        # 产物进 uwp/src/Host/（.gitignore 排除）
& $msbuild uwp\src\ArcDeck.csproj /p:Configuration=Release /p:Platform=x64
```

### 三个非显然的坑

| 坑 | 现象 | 处置 |
|---|---|---|
| 没引用 `Windows.ApplicationModel.FullTrustAppContract` | `CS0103: 当前上下文中不存在名称 FullTrustProcessLauncher`，看起来像类名写错 | csproj 里显式 `<Reference>` 那个 winmd（UWP 的 NuGet 引用集停在 15138，此 API 是 1809 的） |
| AppService 不校验调用方 | 任何知道包族名与服务名的应用都能连上来 | `OnBackgroundActivated` 里比对 `CallerPackageFamilyName`，不是本包就拒绝并记日志 |
| 连接关闭后进程不退出 | 下一次装包被它锁住文件，部署报 `0x80073D02`「资源正在使用中」 | `ServiceClosed` 里 `Environment.Exit(0)`；实测关掉 UWP 后伴随进程收到 `服务关闭：Completed` 并退出 |

### 验收

设置页 →「伴随进程」→「拉起并检查」。正常时界面自己显示：

```
已连接
ping → {"packaged":true,"packageFamilyName":"ProjectArcturius.ArcDeck_…", …}
env  → {"appData":"C:\\Users\\<user>\\AppData\\Roaming", …}
probe → {"found":true,"command":"dsh --version","exitCode":0,"stdout":"…"}
```

`probe` 会真的起一个 `dsh --version` 子进程；`env` 读的是包外的真实路径。
两半的日志：`%TEMP%\arcdeck-host.log`（进程侧）与 `LocalState\host.log`（UWP 侧）。

**还没做**：界面上的会话/对话仍是示例数据。通道只到「拉起、探活、执行」这一步。

## 构建

**必须用 MSBuild**（UWP 不支持 `dotnet build`），**用 Release 配置**
（Debug 的 .NET Native 依赖未预装的调试运行时）：

```powershell
$msbuild = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\MSBuild.exe"

& $msbuild src\ArcDeck.csproj /t:Restore /p:Configuration=Release /p:Platform=x64
& $msbuild src\ArcDeck.csproj /p:Configuration=Release /p:Platform=x64
```

产物：

```
src\bin\Release\ArcDeck.exe
src\AppPackages\ArcDeck_0.1.0.0_x64_Test\ArcDeck_0.1.0.0_x64.msix
```

csproj 里两个非默认项**不要删**：

| 设置 | 为什么 |
|---|---|
| `<LangVersion>10.0</LangVersion>` | UWP 默认 C# 7.3，`is not` 与 switch 表达式会报 CS8370 |
| `<UseDotNetNativeToolchain>true</UseDotNetNativeToolchain>` | 让应用自包含，摆脱对 .NET CoreRuntime 框架包的依赖 |

## 部署：签名 → 安装 → 启动

已实测跑通。**一条命令搞定，并给出明确结论**：

```powershell
dotnet run --project tools/deploy-check
```

它按顺序做四件事并逐步报告结果：

```
1 · 包是否已签名          → 检查 AppxSignature.p7x 是否存在
2 · 签名证书是否被信任     → 检查 CurrentUser\Root / TrustedPeople
3 · 包是否已安装          → Get-AppxPackage
4 · 建证书 → 信任 → 签名 → 验证 → 给结论
```

结论是**三种明确状态之一**，不需要从输出里猜：

| 结论 | 含义 |
|---|---|
| 未完成 | 包还没签 |
| 做了一半 | 包签了，但证书没被信任 —— 安装会被 `0x800B0109` 挡住 |
| 完成 | 可以 `Add-AppxPackage` 了 |

只查不签：`dotnet run --project tools/deploy-check -- --check-only`

### 实测的完整链路

| 步骤 | 结果 |
|---|---|
| 生成 `CN=ProjectArcturius` 证书 | ✓ 主体必须与清单 `Publisher` **一致** |
| 写入 `CurrentUser\TrustedPeople` | ✓ |
| 写入 `CurrentUser\Root` | ✓ **不需要管理员** |
| `signtool sign /fd SHA256` | ✓ |
| 验证 `AppxSignature.p7x` | ✓ |
| `Add-AppxPackage` | ✓ `Status: Ok` |
| 启动 | ✓ 运行中，约 81 MB |

## 两个曾经卡住很久的坑

### 1 · `TrustedPeople` 不够，必须在 `Root`

只装 `TrustedPeople` 会在 `Add-AppxPackage` 报：

```
0x800B0109: 应用包或捆绑包中的签名的根证书必须是受信任的证书
```

**自签名证书是它自己的根**，所以要装进 `Root`。

### 2 · PowerShell 的 `Import-Certificate` 写 `Root` 会报 "UI is not allowed"

```
Import-Certificate -CertStoreLocation Cert:\CurrentUser\Root
→ UI is not allowed in this operation
```

**这不是权限问题。** 改用 .NET 的 `X509Store` API 直接写就成功了：

```csharp
using var s = new X509Store(StoreName.Root, StoreLocation.CurrentUser);
s.Open(OpenFlags.ReadWrite);
s.Add(new X509Certificate2(cert.Export(X509ContentType.Cert)));
```

我因为那条报错以为「需要管理员权限」，白绕了一圈。**区分「权限不足」和
「cmdlet 自身的限制」值得先花一分钟。**

### 3 · 证书主体必须与清单 `Publisher` 逐字一致

我一开始用旧项目留下的 `CN=TakahashiRinta` 去签 `CN=ProjectArcturius` 的清单，
签名"成功"了，但装不上。主体不匹配的签名是无效的。

## 排查启动失败

`Start-Process shell:appsFolder\...` 只告诉你「已请求启动」，失败时给不出原因；
事件日志的 `.NET Runtime` 条目**也不含异常消息**（只有代号）。

要拿真实 HRESULT 得直接调 COM：

```csharp
IApplicationActivationManager mgr = (IApplicationActivationManager)new ApplicationActivationManager();
var hr = mgr.ActivateApplication(aumid, null, 0, out var pid);
// hr = 0x8027025B → 应用未启动
```

**但先跑 `deploy-check`** —— 大部分「起不来」其实是没签名或证书不可信，
那是部署问题不是启动问题。

## 踩过的坑

| 坑 | 现象 | 处置 |
|---|---|---|
| WinUI 2 新增控件不在默认命名空间 | `WMC0001: Unknown type 'InfoBar'` | 加 `xmlns:muxc="using:Microsoft.UI.Xaml.Controls"` |
| **但系统控件在 `muxc:` 里也不存在** | `WMC0001: Unknown type 'AutoSuggestBox'` | 系统控件用默认命名空间。分界线只能靠编译器确认 |
| `NavigationView` 有两份 | 生成代码报「没有与委托匹配的重载」 | 两个命名空间都有，事件处理器参数必须完全限定用 `Microsoft.UI.Xaml.Controls` |
| UWP 默认 C# 7.3 | `CS8370: 「not 模式」在 C# 7.3 中不可用` | csproj 加 `<LangVersion>10.0</LangVersion>` |
| `RatingControl` / `PipsPager` 没有 `Header` | `WMC0011: Unknown member 'Header'` | 那是 `NumberBox` 的属性。自己给 `TextBlock` 标签 |
| `x:Bind` 不支持字符串索引 | `WMC1110: Unexpected array indexer` | 在模型上给一个属性（如 `Initial`） |
| `IAsyncOperation` 的 `await` 找不到 `GetAwaiter` | `CS4036` | 加 `using System;` |
| `Properties/Logo` 走 StoreLogo 校验 | `APPX1619: 必须为 50x50` | 与 `Square150x150Logo` 不是同一项 |
| 清单缺 `PhoneIdentity` | `APPX1673` | 桌面清单里也是必需元素 |
| `Add-AppxPackage -Register` 松散文件能注册但起不来 | 激活 `0x8027025B` | 走「签名 MSIX → 安装」才是正路 |

