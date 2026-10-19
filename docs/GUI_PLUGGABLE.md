# GUI 热插拔：研发与发布方案（Windows + Linux）

日期：2026-10-20 · 状态：**研究结论 + 第一步已实现**（`arcdeck serve --stdio`）。其余是设计，逐条标明。
上位文档：PRODUCT_PLAN §4（架构）、§8（GUI，冻结中）；宿主验证：[HOST_VALIDATION_2026-10-15.md](HOST_VALIDATION_2026-10-15.md)；
协议核对：[REMOTE_0.2.1.md](REMOTE_0.2.1.md)。

## 0. 一句话

**GUI 不链接后端，只说一份带版本的消息协议**（`arc_deck_core::service`）。后端是 `arcdeck` 本身，GUI 是另一个可独立安装、
升级、卸载的程序；它拉起 `arcdeck serve --stdio`（已实现），或连上一个共享的 `arcdeck serve` socket（设计，未实现）。
这份协议 **TUI 每天都在进程内用**，所以它不是纸面契约。

## 1. 「热插拔」在这里指什么 —— 五条可验收的性质

| 编号 | 性质 | 怎么验收 | 现状 |
|---|---|---|---|
| H1 | 后端不依赖任何 GUI；GUI 不链接后端代码，只经进程边界 | `arcdeck` 构建图里没有 GUI crate；GUI 的依赖里没有 `arc-deck-dsh` | ✅ 后端侧成立；GUI 侧在接线时由 Cargo / csproj 依赖守住 |
| H2 | GUI 独立构建、发布、升级、卸载，不重装后端，反之亦然 | 换一个版本的 GUI 连同一个 `arcdeck` 能用；协议主版本不同时明确拒绝 | ⚠️ 协议与握手已实现并有测试；两个真 GUI 还没接 |
| H3 | GUI 随时连上 / 断开，不丢会话 | 关掉 GUI 再开，`/sessions` 能打开刚才的会话、历史完整 | ✅ 会话在独立的 DSH home，**跨进程恢复已在 TUI 真终端实测**（M4_PLAN） |
| H4 | 多个前端（TUI + GUI）同时连同一个后端、看同一个会话 | 两个前端同时打开同一会话，一边发、另一边实时看到 | ❌ 需要共享 socket（§3 第二档），未实现；现在第二个后端会被会话目录锁**明确拒绝** |
| H5 | 版本不匹配时说清楚该升级谁，而不是乱码或挂住 | 前端发 `hello{protocol:N}`，后端回 `welcome{accepted:false, reason:"…请升级前端/后端"}` | ✅ `core::service::negotiate` + `serve` 的握手测试 |

H4 之所以重要：交互内核独占一份 DSH home（`launch::HomeLock`），两个后端写同一份会话日志正是 AGENTS P14 坏日志的候选成因。
所以「TUI 开着时再开 GUI」的正确形态是**共用一个后端**，不是各起一个。

## 2. 架构

```text
 TUI（ratatui，进程内）────────┐
                              │  arc_deck_core::service::{Command, Event}   ← 唯一契约，serde JSON
 Windows GUI ─ UWP 界面        │
     │ AppService（ValueSet）  │
     ▼                        │
   ArcDeck.Host.exe ──stdio──▶├─ arcdeck serve --stdio      （已实现）
                              │  arcdeck serve（共享 socket）（设计）
 Linux GUI（Kanesumi）─stdio──┘
                              ▼
          arc_deck_dsh::remote::service（独占一个交互内核、管 follow、合并流式推送）
                              ▼
          DSH web 内核（`--profile web`，独立 DSH home，不是 Desktop 的）
```

**为什么契约是我们自己的消息，而不是让 GUI 直接连 DSH 的 Remote 层**：DSH 的 web 内核本身就是一个本地 HTTP + WS 服务，
GUI 直连它最省事，但那样 GUI 就要懂 `assistant-stream`、权威全文替换、`source.kind`、翻页游标（P3–P16 全套），
两个 GUI 各写一遍、各错一遍；DSH 一升级（0.1.6 → 0.2.1 就重构过两次）三处一起改。PRODUCT_PLAN §8 早已定下
「页面不直接解释 DSH 私有事件」—— 这里把它落成一份可序列化、可版本化的契约。

**为什么用 JSON 行，不用 gRPC / protobuf**：流量小（整份会话视图在流式时最多 20 Hz）；C# 侧 Host.exe 是 .NET 9，
`System.Text.Json` 现成；Rust 侧 serde 现成；不给两边加代码生成与新依赖。以后真要换编码，那是一次协议主版本。

## 3. 传输：两档

### 第一档 · `arcdeck serve --stdio`（已实现，`crates/arc-deck-cli/src/serve.rs`）

GUI 进程拉起 `arcdeck serve --stdio [--permission L] [--workspace D]`，一行一条 JSON：

```text
→ {"op":"hello","protocol":1,"client":"uwp-0.1.18"}
← {"event":"welcome","protocol":1,"accepted":true,"backend":"0.1.0"}
← {"event":"backend","state":{"state":"starting"}}
← {"event":"backend","state":{"state":"ready","model":{"provider":"opencode-go","model":"deepseek-v4.1-flash"}}}
← {"event":"catalog",…}  {"event":"sessions",…}
→ {"op":"prompt","text":"…"}                     （session_id 省略 = 新建）
← {"event":"session_started","session_id":"session-…"}
← {"event":"conversation","conversation":{…整份视图…}}   （流式时最多每 50 ms 一份，一轮结束立即一份）
→ {"op":"cancel","session_id":"session-…"}
（stdin EOF）→ 后端关 follow → 收内核进程树 → 放锁 → 退出 0
```

| 规则 | 理由 |
|---|---|
| 第一行必须是 `hello`；主版本不同回 `accepted:false` 并以 64 退出 | H5 |
| 坏行回 `notice{level:error}`，**不断开** | 一行写坏不该让 GUI 丢掉正在跑的会话 |
| 未知 `op` 以 `Unknown` 收下并忽略；未知 `event` 由前端同样处理 | 新前端配旧后端、旧前端配新后端都能降级运行（`core::service` 的测试） |
| stdout 只写协议帧，诊断走 stderr；只有一个写端 | 帧不会交错成半行 |
| stdin EOF = 前端走了 = 收掉后端 | stdio 模式下后端只属于拉起它的那个前端；不留孤儿 |

**实测（2026-10-20，release，不耗配额）**：喂 `hello` / `list_sessions` / `catalog` / 一行坏 JSON / 一个未知 `op` 后 EOF ——
输出依次是 welcome、两条授权缺口留档、starting、坏行的 error 通知、ready（用户配置的路由而不是内核出厂默认）、目录、会话列表、
对应的两次应答、stopped；退出码 0；之后没有残留的 `--profile web` node 进程，会话目录锁已释放。

这一档的代价：GUI 关掉就收内核，再开要等内核冷启动（实测数秒）；与 TUI 不能同时开（锁会明确拒绝第二个）。

### 第二档 · 共享 `arcdeck serve`（设计，未实现）

同一份消息，换成每连接一条 socket；后端成为单例常驻进程，前端「先连，连不上再拉起」。

| 事 | Linux | Windows |
|---|---|---|
| 端点 | `$XDG_RUNTIME_DIR/arcdeck/service.sock`，目录 0700 | 命名管道 `\\.\pipe\arcdeck-<当前用户 SID 的短哈希>`，DACL 只给当前用户；`reject_remote_clients(true)` |
| 发现 | `<本机数据目录>/arcdeck/service.json`：`{pid, pid_started_at, endpoint, protocol, backend}` | 同左 |
| 存活判定 | 读者算：pid + 启动时刻（与 `arcdeck status` 同一规则，进程死了不能自称健康） | 同左 |
| 退出 | 最后一个前端断开、且没有进行中的一轮，空闲 N 分钟后退出 | 同左 |
| 单例 | 已有的 `HomeLock`（一份 DSH home 一个内核）继续是唯一的判据 | 同左 |

不用 TCP 回环 / HTTP 的理由：任何本机进程都能连（要另造令牌）、Windows 防火墙可能弹窗、还会被误当成「Arc Deck 有个 web 服务」。
Rust 侧 `tokio::net::windows::named_pipe` 与 `tokio::net::UnixListener` 都在已有依赖里；tokio 的命名管道曾有
「设了 `pipe_mode` 会把 `reject_remote_clients` 冲成 false」的缺陷（[RUSTSEC-2023-0001](https://osv.dev/vulnerability/RUSTSEC-2023-0001)），
本仓锁的 tokio 1.53.2 不在受影响范围，但实现时仍按通告的顺序先设 mode 再设 reject，并写测试钉住。

这一档做完，H4 成立：TUI 启动时先连共享后端，连不上才自己起；GUI 同理。**进程内的 TUI 路径保留**作为无 socket 环境的退路。

## 4. Windows：UWP 界面 + 完整信任伴随进程

结构沿用已实测的备选 A（HOST_VALIDATION §七）：UWP（AppContainer）只管显示，`ArcDeck.Host.exe`（包内完整信任，.NET 9）
起子进程、读包外路径。接后端的方式：

1. Host.exe 用 `Process`（`CreateNoWindow`、重定向三条流）拉起 `arcdeck serve --stdio`，**它**说协议（`System.Text.Json`），
   把 `Event` 经 AppService 转给 UWP：`ValueSet{ "frame": <JSON 字符串> }`，UWP 用 `Windows.Data.Json` 解析
   （.NET Native 里没有 `System.Text.Json`，HOST_VALIDATION §七第 3 条）。
2. **找 `arcdeck` 的顺序**：设置页里用户指定的路径 → `PATH` → 安装器的默认位置（`%LOCALAPPDATA%\Programs\arcdeck\arcdeck.exe`）。
   找不到或握手被拒，设置页给出可操作的原因（「安装 arcdeck」/「升级 arcdeck 到协议 N」），不显示示例数据冒充。
3. **MSIX 里不内置 `arcdeck.exe`**（建议，待裁定）：内置会把 GUI 与后端的版本绑死（违背 H2），也会在本机出现两份
   DSH 运行时的使用方；握手已经能处理版本不匹配。
4. 从命令行打开 GUI：清单里加 `uap5:AppExecutionAlias`（例如 `arcdeck-gui`）—— 执行别名就是 MSIX 应用被包外命令行
   拉起的正规入口（[MS Learn：WinApp CLI 打包指南](https://learn.microsoft.com/en-gb/windows/apps/dev-tools/winapp-cli/guides/packaging-cli)、
   [Advanced Installer：MSIX 环境变量与别名](https://www.advancedinstaller.com/msix-environment-variables.html)）。
   深链（「打开某个会话」）用 `uap:Protocol` 激活，URI 参数进激活事件（[MS Learn：App Installer 安装参数](https://learn.microsoft.com/en-us/windows/msix/app-installer/install-parameters)）。
5. 现有约束继续有效：U1–U18（尤其 U18 的「`ServiceClosed` 里退出」—— Host.exe 退出时要先关 stdin 让 `serve` 收内核）。

## 5. Linux（Arch）：Kanesumi

1. 单独的二进制（建议名 `arcdeck-gui`），只依赖 `arc-deck-core`（为了那几个 serde 类型）—— **不依赖 `arc-deck-dsh`**，
   由 Cargo 依赖图守住 H1。即便同为 Rust、能直接链接，也走进程边界（PRODUCT_PLAN §8 已定）。
2. 拉起 PATH 上的 `arcdeck serve --stdio`；第二档做完后先连 `$XDG_RUNTIME_DIR/arcdeck/service.sock`。
3. K1–K9 照旧（不用 winit、自管 IME……）。Kanesumi 的 Wayland 外壳与 IME 进展在 Ether 侧，接线排在它之后。
4. 打包：先做**原生包**（AUR 拆成 `arcdeck` 与 `arcdeck-gui` 两个包、或 tarball）。**Flatpak 暂不做**：第一档要拉起宿主上的
   `arcdeck`，在沙箱里只能靠 `flatpak-spawn --host`，那需要 `--talk-name=org.freedesktop.Flatpak`，等于开了沙箱逃逸口；
   第二档有了 socket 之后可以只开放 `xdg-run/arcdeck` 一个子目录（与 dconf 的 `--filesystem=xdg-run/dconf` 同一做法，
   [Flatpak 沙箱权限文档](https://docs.flatpak.org/es/latest/sandbox-permissions.html)），那时再评估。

## 6. 发现与启动：`arcdeck gui`（设计）

不建插件注册表：**PATH 上的约定名就是注册**。Linux 的 `arcdeck-gui` 在 PATH；Windows 的 MSIX 执行别名落在
`%LOCALAPPDATA%\Microsoft\WindowsApps`，那个目录本来就在 PATH 上。`arcdeck gui` 只做一件事：找到 `arcdeck-gui` 并拉起，
找不到就说怎么装。装 / 卸 GUI 不需要后端知道 —— 这就是「插」与「拔」。

## 7. 研发流程：GUI 怎么独立开发

| 需要 | 做法 | 状态 |
|---|---|---|
| 真后端 | `arcdeck serve --stdio`，与 TUI 同一份服务代码 | ✅ |
| 离线、确定的后端（UI 测试、截图对照、无配额） | `arcdeck serve --stdio --fixture <录制>`：回放录下来的 `Event` 序列，只在 `test-runtime` 构建里存在（与现有假 SDK 对端同一纪律） | ❌ 待做 |
| 契约夹具 | 从真实 `serve` 会话录 `tests/fixtures/service/*.jsonl`（一轮、流式、取消、失败、工具），C# 与 Kanesumi 的测试都读它 | ❌ 待做（会话层已有真实抓帧 `one-turn.jsonl` 可作起点） |
| 契约演进 | 只「加」：新字段 `#[serde(default)]`，新种类靠 `Unknown` 兜底；改义 / 删除 = 主版本 | ✅ 规则写在 `core::service` 顶部并有测试 |
| 改 GUI 不重启内核 | 第二档共享后端：GUI 进程随便重启，内核与会话不动 | ❌ 随第二档 |

## 8. 发布矩阵（建议，待裁定渠道）

| 产物 | Windows | Linux（Arch） | 说明 |
|---|---|---|---|
| `arcdeck`（后端 + TUI + headless） | 便携 zip / winget（portable） | AUR / tarball | 单一可执行文件；DSH 运行时仍由 `arcdeck runtime install` 装进用户数据目录（§9.2a） |
| GUI | MSIX（UWP + Host.exe），执行别名 `arcdeck-gui` | `arcdeck-gui`（Kanesumi） | 独立版本号；`Welcome` 里互报版本 |
| 兼容判据 | 协议主版本相同 | 同左 | 不按产品版本号判 |

## 9. 下一步（按顺序，每步可验收）

1. **契约夹具 + `--fixture` 回放后端**：GUI 不花配额也能跑通全部界面状态。
2. **Windows 接真实数据**：Host.exe 拉起 `serve --stdio` 并中继；`ChatPage` / 会话列表去掉示例数据；UIA 验收沿用 `SendCheck.ps1`，
   断言换成真实会话（新建、发送、流式、取消、失败、恢复），并与 TUI 看同一会话的结论一致（§8 原有验收）。
3. **共享 socket（第二档）** + TUI 先连后起 + 端点文件与存活判定：H4。
4. **Kanesumi 客户端**（等 Wayland 外壳与 IME）。
5. **打包与分发**：按 §8 的裁定。

## 10. 待用户裁定

1. MSIX 是否内置 `arcdeck.exe`（建议否：破坏 H2）。
2. Windows 分发渠道：Microsoft Store / 自签名侧载 / winget —— 影响证书与更新方式（现有装机是自签名侧载，U 系列构建说明）。
3. Linux 是否需要 Flatpak（建议先原生包，第二档之后再评估）。
4. GUI 发现 `arcdeck` 缺失或过旧时，是否允许它代为下载 / 升级后端（建议否：只给出命令，安装动作留给用户）。
