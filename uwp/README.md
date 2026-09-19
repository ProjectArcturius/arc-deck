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

## 已知问题：本机无法启动

**代码完整、能编译、产出可安装的 MSIX。但在此开发机上启动失败（`0x8027025B`）。**

做了对照实验，不是推测：

| 实验 | 结果 |
|---|---|
| 本应用 | 激活 `0x8027025B` |
| **纯空白 UWP 应用**（不含本项目任何代码） | **同样 `0x8027025B`** |
| **系统自带计算器**（同一探针、同一方式） | **启动成功** |
| 装齐 `Microsoft.NET.CoreRuntime.2.2` 等框架包 | 无改善 |
| 关闭 .NET Native 工具链 | 无改善 |
| 签名后 `Add-AppxPackage` 装 MSIX | 被证书信任链挡住 |

**根因：旁加载应用的信任链。** 自签名证书未被信任，而写受信任根存储需要
**管理员权限**。空白应用与完整应用表现完全一致，说明触发点是部署前置条件，
不是实现。

### 在有管理员权限的机器上

```powershell
# 1) 信任测试证书（需管理员）
$cert = New-SelfSignedCertificate -Type Custom -Subject "CN=ProjectArcturius" `
    -KeyUsage DigitalSignature -FriendlyName "Arc Deck Test" `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3","2.5.29.19={text}")
Export-Certificate -Cert $cert -FilePath ArcDeckTest.cer
Import-Certificate -FilePath ArcDeckTest.cer -CertStoreLocation Cert:\LocalMachine\Root

# 2) 签名并安装
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe" `
    sign /fd SHA256 /a src\AppPackages\**\*_x64.msix
Add-AppxPackage -Path src\AppPackages\**\*_x64.msix

# 3) 启动
Start-Process "shell:appsFolder\ProjectArcturius.ArcDeck_<publisherhash>!App"
```

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

## 排查启动失败的工具

用 `IApplicationActivationManager.ActivateApplication` 的 COM 接口拿真实 HRESULT：

```csharp
var mgr = (IApplicationActivationManager)new ApplicationActivationManager();
var hr = mgr.ActivateApplication(aumid, null, 0, out var pid);
// hr = 0x8027025B → 应用未启动
```

`Start-Process shell:appsFolder\...` 只告诉你「已请求启动」，失败时给不出原因；
事件日志的 `.NET Runtime` 条目**也不含异常消息**（只有代号）。下次先用它。
