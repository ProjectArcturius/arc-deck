# AGENTS.md — Arc Deck

## 当前产品方向（2026-10-14 定型）

需求、架构与 roadmap 的入口：[docs/PRODUCT_PLAN.md](docs/PRODUCT_PLAN.md)，先读它的 §0。
此刻的状态、仓外的东西与换机器步骤：[docs/STATE_2026-10-14.md](docs/STATE_2026-10-14.md)。
DSH 对非 DS 模型是否适用的调查：[docs/DSH_FINDINGS.md](docs/DSH_FINDINGS.md)。
人格与风格怎么装（Claude Code 的做法）：[docs/PERSONA_FINDINGS.md](docs/PERSONA_FINDINGS.md)。
不可让渡的规则与「什么算个性化」：[prompts/constitution.md](prompts/constitution.md)（草案）。
TUI 派工计划与状态（**队列 2026-10-14 起暂停**）：[docs/M4_PLAN.md](docs/M4_PLAN.md)；派工手册：[tasks/m4/README.md](tasks/m4/README.md)。
授权 → DSH 执行层的映射与缺口：[docs/M3_GUARD.md](docs/M3_GUARD.md)。DSH 0.2.1 Remote 层的协议核对：[docs/REMOTE_0.2.1.md](docs/REMOTE_0.2.1.md)。
GUI 怎么热插拔地研发与发布（Windows + Linux）：[docs/GUI_PLUGGABLE.md](docs/GUI_PLUGGABLE.md)。用 arcdeck 工人替代 OpenCode 工人的复盘：[docs/ARCDECK_WORKERS_REVIEW_2026-10-14.md](docs/ARCDECK_WORKERS_REVIEW_2026-10-14.md)。

- **2026-10-14 补记**：用户随后要求用 arcdeck 工人把 TUI 做到日常可用，W1（Markdown / 主题 / 模型对话框）与 M3 第一段已合入；同日暂停派工，下一步重心转 GUI，剩余 TUI 队列可由替班按手册继续。**本机 C 盘已满：worktree、TEMP、构建输出都在 D 盘**（见 tasks/m4/README.md）。
- **当前焦点只有一件事：Rust 后端的 headless 在 Ether 上可用**（PRODUCT_PLAN M2a–M2c）。TUI 主流程已可用（M4 只差 §7.5 的人工视觉对照）；两个桌面 GUI 冻结。
- 一个可执行文件 `arcdeck`：工作目录下无参数运行进 TUI，带子命令进 headless。headless 的主用法是 Ether 的工人任务书；调度者靠 `arcdeck status` 一行 + 退出码判断「完成 / 在跑 / 卡死 / 出错」，不读长日志。健康由读者按时间戳计算，进程死了不能自称健康。
- **底层只用 DSH 一个内核**：DS 走 `deepseek-official`（Messages 协议，DS 优势只在官方路由上成立），非 DS 走 DSH 的 pi-ai 路由。不再造 OpenCode/Claude 的执行内核；它们只作 TUI、Agent、工具定义的设计参考。「非 DS 用 DSH 不劣于 OpenCode」未测，是 M2d 的对照门。
- **仓库主许可证是 Apache-2.0**（根 `LICENSE`，第三方归属见 `NOTICE`）。**TUI 的视觉与交互基线来自 OpenCode**（v1.19.0；主题资产取自 v2.0.27），不是原创视觉。直接移植或逐字取用的代码、主题必须保留 MIT notice（`crates/arc-deck-tui/LICENSE-OpenCode`），并在文件头标明来源。OpenCode 只是参考与移植来源，不是第二执行内核。
- 桌面版两块：Windows = C# + WinUI 2（不擅自换 WinUI 3 或 WebView），Linux = Rust + Kanesumi。都在后端本地服务（M5）之后经同一 IPC 接入。
- 默认 Agent 是 Echo，具备完整工程角色、工具/权限、验证/交付纪律与用户提供的风格；Build/Plan 参考 OpenCode。提示词不能替代实际权限。
- DSH 默认把会话日志随请求上传 DeepSeek（`session-log-deepseek`）。用户裁定前，Arc Deck 生成的 profile 关闭它；不吃 DSH 默认值。
- headless 第一个可用版本已在 Windows 上实测（PRODUCT_PLAN §10.2b）；Linux 一侧的目标是**本机 Arch**（不是 Debian，2026-10-14 修订）。Arch 上已实测：构建、离线套件、夹具端到端，以及**真实 DSH 运行**（真派工、取消、杀监工→dead、孤儿回收、worktree 真提交）。DSH 不进仓、不进二进制：`arcdeck runtime install` 把钉住的版本装进用户数据目录。
- workers 缺省路由 `opencode-go/deepseek-v4.1-flash`（用户 2026-10-14 指定）。密钥在 `%APPDATA%\arcdeck\auth.json`，**不得写进仓库或任务状态目录**。
- 跑会消耗模型配额的测试前先问用户；用户已允许「少量用 4.1 flash 测试」。
- `/REFS/` 只作本地参考，不入 Git，也不能成为生产构建依赖。复制 OpenCode 的实质部分保留 MIT notice；Claude Code 根许可证为商业条款，不默认复制源码或提示词。
- 旧代码与数据保留。K1–K9 只约束既有 `kanesumi/`，U1–U18 继续约束既有 `uwp/`；旧协议的事件词汇、版本结论与实测只对旧组合成立，新 DSH 必须重新核对。

下文保留旧实现约束与历史实测；其中「两条 GUI」「C# 协议层作为新宿主引用」及原分线安排不再支配新开发。若历史路线与当前产品方向冲突，以 PRODUCT_PLAN 为准；非冲突的工程/证据纪律继续执行。

## 派工：谁把任务书递给谁

本仓在整条链上**几乎只做工人**。所以派工这一侧只有一条规矩要记：**派工时把「谁派的」带上。**

| 座位 | 谁 | 跑在哪 |
|---|---|---|
| 绝对高位调度者（老板） | Claude，用自己的 Claude Code 发任务书 | 本仓之外 |
| 组长 | Sol，用自己的 Codex；可替班，也可小做修改与 feat | 本仓之外 |
| 工人 | `arcdeck` worker | 本机 / Ether |

派一件工人任务，跟踪都在这三行里：

```bash
arcdeck run <任务书.md> --id <id> --by <谁> --worktree dsh/<id> --accept "<验收命令>" --detach
arcdeck status <id>          # 每次检查只读这一行；退出码见 PRODUCT_PLAN §6.5.3
arcdeck report <id>          # 终态后：Arc Deck 采集的事实与模型的陈述分开
```

- **`--by` 记的是派工关系的边，不是授权。** 它落成 `status.json` 里的 `assigned_by`，供外部监控程序（`cute-agents-monitor`）画任务书流向。缺席就是没记，不改变任何判定。上两层不是 Arc Deck 任务，所以它是自由文本（`claude` / `sol`），不是任务 id。
- **一般模式**：`arcdeck run "问句话"` 就是普通 agent —— 没有任务书、不带工人参数时 stdout 是给人看的回答，不是 JSONL。**Arc Deck 不是一个工地**，工人那套（任务书、验收、工人守则、worktree）只在你带上那些参数时才生效。两种模式都落状态目录，监控程序一视同仁。
- **人格默认关**：装不装 Arc Deck 的人格由 `personalization.enabled` 决定，缺省关闭 —— 关的时候一般模式**一段提示词都不注入**，按模型本体行事。工人模式永远只拿功能契约，**不带风格**（PRODUCT_PLAN §5.3.1）。
- **总是带 `--accept`。** 没有验收命令时 `completed` 只表示内核正常结束 —— §10.2b 里实测出现过「提交被拒但 completed」。
- Ether 侧的派工脚本抄 `examples/ether/ad.sh`。
- **命名**：授权说的是**级别**（`readonly` / `workspace` / `full`，§5.6），不是 DSH 的沙箱档名；模型强度叫 **`effort`**，不是 opencode 的 `variant`。派工给别人看的东西一律用产品名。

## 旧实现与历史记录

此前 Arc Deck 是 DeepSeek Harness 的原生桌面客户端，有两个并列实现，
外加**一层两者共用的协议客户端**。

```
kanesumi/       Rust + 自绘 Scene（跨平台，无 WebView）   based on Kanesumi Design
uwp/            C# + WinUI 2（Windows 专属）              用微软自家的接口与套件
arc-deck-dsh/   与内核说话的协议层（C# / .NET 9，桌面进程）  两个实现都要用它
```

**两个实现代码不共享，外观也不共享。** 它们是同一个客户端的两条实现路线：

| | `kanesumi/` | `uwp/` |
|---|---|---|
| 外观 | 自定 —— Kanesumi Design | **跟系统走**（唯一例外：圆角归零，见 U3） |
| 理由 | 跨平台一致性 | 用平台现成的基础设施 |

产品名 **Arc Deck**，仓名 `arc-deck`。
设计正典（**只与 `kanesumi/` 相关**）：
[Ether `docs/KANESUMI_DESIGN.md`](https://github.com/ProjectArcturius/Ether/blob/main/docs/KANESUMI_DESIGN.md)。

> 不要把 Kanesumi 的东西搬进 `uwp/`，也不要拿 `uwp/` 的平台行为去改
> Kanesumi 的规格。两者的**唯一**交集是「同一个客户端」这个目标。

### P · `arc-deck-dsh/` —— 既有 C# 共用协议层的历史约束

界面两条路线不共享，但**协议只有一份**：事件词汇、翻译规则、上游版本钉法，
两边读到的东西必须逐字一致。所以它单独成目录，而不是塞进任一实现里。

#### P1 · 界面那侧**必须**是完整信任的桌面进程 —— 不能是 AppContainer

这一条是 UWP 版的**结构性**约束，不是取舍：

| 需要的能力 | `uwp/src/ArcDeck.csproj`（`OutputType=AppContainerExe`） |
|---|---|
| 起 `dsh` 子进程 | ❌ AppContainer 无 `CreateProcess` |
| 读 `%APPDATA%\dsh-desktop\harness`（凭据、会话日志） | ❌ 包外路径不可读 |
| 连 `127.0.0.1` 上已在跑的内核 | ❌ 回环被沙箱挡住（需 `CheckNetIsolation` 提权） |

三条都被堵死，所以「UWP 界面 + 内核」这条路**不能靠改代码走通**。
UWP 版目前的界面本身是好的（会话列表、Markdown、工具页、主题都已实测），
但它无法自己拿到数据 —— 这是两件独立的事，不要混为一谈。

**旧路线的下一步**（未实施，现由 PRODUCT_PLAN 取代）：界面外壳换成完整信任的桌面进程，
`arc-deck-dsh` 作为它的引用；UWP 版作为「无内核也能看的界面」保留。

#### P2 · 上游版本只能**钉**，问不出来

协议**没有版本协商**：握手只回 `serverInfo.version = "0.0.1"`，客户端不校验。
所以「对侧是什么」这个问题在协议里没有答案，只能靠 `DshVersion` 里的常量钉住
一个已验证的组合。升级上游是刻意的工程动作 —— 与 K9 对 Rust 侧的要求同源。

#### P3 · `provider` 与 `model` 不是一回事，且**校验发生在轮次内部**

实测：把 `model` 填成 provider 的名字（`deepseek-official`）时，
`initialize` **成功**、`turn/start` 也照发，错误只在
`assistant/chunk` 的 `finish` 块里：

```
The supported API model names are deepseek-flash, deepseek-v4-pro,
but you passed deepseek-official.
```

界面上表现为「消息发出去以后什么都没有」。**所以 UI 必须消费 `finish` 块的
`reason.failure`** —— 那是模型失败的唯一出口，协议层不会抛错。

#### P4 · 助手的正文在事件流里出现两次，必须区分

- `assistant/chunk` 的 `text-delta` —— 逐 token 增量，用来做流式观感
- `assistant/message` 的 `message.content[]` —— 该步的**权威全文**

两条都用而不区分，界面会出现**正文写两遍**（实测过）。做法见
`DshProjection` 的 `Authoritative` 标记与 `FindStreamedEntry`。

#### P5 · 内核的内部事件不是对话内容

一次普通 `session/prompt` 就会带上 `permission/preset`、`sandbox/mode`、
`approval/policy`、`agent/inbox/spliced`、`request/context` 等事件。
第一次跑通时它们全被画成了原始 JSON，对话里涌出七条，真正的内容被埋掉。
**内部簿记进 `DshProjection.Recorded`，不进 `Messages`。**

#### P6 · 只有 `source.kind == "user"` 的 `user/message` 才是人说的话

内核会把系统提示词、工作区指令、技能清单都写成 `user/message`
（一次 prompt 实测产生 4 条）。全画出来界面会被几百行英文系统提示词淹没。
另外那几条还带 `<system-reminder>` 之类的固定标记，也按标记排掉 ——
**别按长度猜**。

#### P7 · 会话日志是 zstd，而且正文大多被压成「打包行」

落盘路径：`<DSH_HOME>/sessions/<工作区分组>/<会话 id>/session.jsonl.zstd`。
文件名里写着 `jsonl`，**但它不是文本** —— 直接按文本读会得到二进制噪声，
报错是「JSON 解析失败」，指向的方向完全不对。

解压后的第 1 行是会话头（`{"type":"session","id":…,"cwd":…}`，**没有 seq**），
之后每行一帧事件或一行**打包编码**：

| 行类型 | 是什么 | 重放时 |
|---|---|---|
| `assistant/chunk` | 真实事件 | 认 |
| `text-chunks` / `reasoning-chunks` / `tool-call-chunks` | 连续同类增量块的**压缩行** | **跳过** |

实测一份 5127 行的日志：真正的 `text-delta` 只有 **1** 帧，2840 行是打包行。
**展开打包行会把正文写两遍**，而那件事在终端输出里看不出来 ——
所以 `DshProjection` 有一个 `replay` 开关，重放只认权威全文。

两条附带经验：

- 解压必须用**能读多帧拼接流**的 API。日志是追加写的，每追加一段就是一个
  zstd 帧；只解第一帧的 API 会**静默丢掉后半段**。库选了 `ZstdSharp.Port`
  （纯托管、无本机依赖）—— 决定只影响 `DshSessionLogFile` 一个文件
- 一个正在被写的会话，末尾可能是**半行**。判据只看「末尾有没有换行」，
  不要猜内容（`{...}` 与 `{...` 从形状上分不出来）。这不是错误，但要报出来

#### P8 · 一条消息 = 一轮，不是一步

`turn` 是「用户说一次话」，`step` 是这一轮里模型的一次调用。一次 agentic 的
运行有**几十到几百个 step**（实测：1 轮 238 步）。按 step 建消息会得到几百个
气泡、绝大多数只有一个工具卡。所以按 `turn` 归并，工具卡与思考块进同一条消息。

**`turn` 从 1 开始**（实测日志里只有 `turn:1` 与 `turn:2`，没有 `turn:0`），
不能拿它当索引再加一 —— 那样会多报一轮，而「轮数偏一」在界面上看不出来。

#### P9 · 「重放是对的」需要证据，不能只有论证

论证（两条路径共用同一个 `DshProjection`）是对的，但论证不等于证据。
现在有三层：

| 层 | 工具 | 证明什么 |
|---|---|---|
| 解压 | `tools/zstd-probe` | 与 zstd 命令行**逐字节一致**（多帧拼接） |
| 重放不变式 | `tools/log-probe --check-all <sessions 根>` | **全部真实日志**上：正文不重复 / 工具配对 / 无空消息 / 轮次一致 / 打包行确实跳过 / 重放后无残留流式段落 |
| 实时 vs 重放 | `dsh-cli --verify-reload` | **同一条会话**两条路径逐块比对一致 |

**一处必须写下来的失败尝试**：`log-probe --no-replay`（展开打包行、并按最朴素
的方式消费增量）**本来是用来做对照实验的** —— 当时的说法是「它必须让断言失败」。
实测下来它**没有产生任何差异**：流式段落照常被权威全文替换掉，帧数与正文块数
两边完全一样。也就是说那个开关当时什么都没证明，而文档里已经写上了漂亮话。

现在的处理是让 `log-probe` 自己把它量出来并**明确报「没有差异」**
（见检查项 ⑥），而不是继续假装。要真正证明那道判断有牙齿，需要一份
「权威全文缺席」的日志 —— 现在手上没有。

结论：**跳过打包行是论证正确、但尚未被对照实验证伪过的判断。**
按这条规矩来：论证可以写，但要标明它还没被证伪过。

另外：不变式要按真实数据的分布来定，不能按「应该怎样」定。
实测一份 315 次工具调用的日志里稳定地有 **1** 个没有结果的调用，
所以「工具必须全部配对」是过严的断言，判据得是「孤儿 > 1」。

#### P10 · sdk profile 下工具**真的会执行** —— 「审批策略 ask」是虚的

`sdk` profile 的事件流里 `approval/policy = ask`，但组合里**没有应答者**，
而上游规定没有应答者时请求以 `unavailable` **失败关闭**。

实测：一条 `session/prompt` 明确要求跑 PowerShell，内核直接执行并返回了结果
（`isError: false`，内容就是那两条 `.md` 文件名）。也就是说 sdk profile 下
工具调用不会卡在等审批上 —— 本客户端**不需要**实现审批 UI 才能用。

但要注意这条的**边界**：它说明的是「这条链路能跑通」，不是「审批语义不存在」。
审批事件（`approval/asked` / `approval/decided`）仍然会写进日志，
将来要做「让用户看见哪些操作被放行 / 被拒」时从日志里读。

#### P11 · `follow` 的 opening **就是快照那一帧** —— 之后全是 live

契约来自上游源码（`dsh-api-session-controller/lib/types/history.js`）：

```
@returns a complete opening snapshot followed by gap-free event frames
```

发完快照后，服务端把游标设为 `cursor + 1`，之后每帧的 `seq` 严格连续。
**没有「opening 结束」的标记帧** —— 快照是 opening 的全部。

所以「读到快照为止」是字面意思：折完那一帧就跳出读循环。这条写错过一次，
代价是一整轮卡死：旧代码收下快照后继续等下一帧（一个不再有人说话的会话当然
没有下一帧），一路等到 60s 上限，报出来的还是「没给出快照」——
而线上那一帧 73806 字节的快照早就到了（`probe-harness7.log`）。
**「收到了却报没收到」比「没收到」难查得多。**

顺带记一条同源的经验：快照与调用方第一次 `PumpAsync` 之间到达的帧不会丢，
mux 的 channel 是无界的。

#### P12 · WebSocket 分块边界会**撕开多字节字符**

`ClientWebSocket.ReceiveAsync` 给的是一段一段的字节，**不能在每段上各自
`Encoding.UTF8.GetString`** —— 一个跨边界的汉字会被两边各解出半个，
变成两个 `U+FFFD`，而且字符串还多出一个字符。必须攒完整条消息再解码一次。

实测抓法不是读代码，是两条路对账（参 P9 的思路）：`--harness-check-all`
在 71 个会话里报出**唯一**一处不一致 —— 某个 147 块的消息里只有一个思考块
长度 7799 而重放是 7798：

```
实时 …帧（Delay → 隐\uFFFD\uFFFD）\nassert!(
重放 …帧（Delay → 隐藏）\nassert!(r
```

**影响面比这一处大**：快照动辄几十上百 KB、一定跨分块，而中文字符占 3 字节 ——
所以中文内容在长消息里会被静默损坏。短会话上没暴露，只是那个边界恰好没落在
字符中间。这类「静默偶发」只有对账能抓到。

#### P13 · 快照是**一页**，不是全部历史 —— 而且子 agent 的地址形状不一样

两条都是 `--harness-check-all` 在 71 个真实会话上量出来的：

**一页 —— 所以要往回翻。** 大会话上线上只给最后一页（实测约 2500 条记录 / 2 千多帧），
`hasMore = true`。拿一页去比整份日志会得到**假不一致** —— 第一版就这么报了
11 条「消息条数 线上 10 ≠ 重放 136」。

`session/page` 的契约（从上游 `types/history.js` 的 `page` 读出）有三处反直觉：

- **`throughSeq` 是必需字段**，且 `-1` 表示「到头了」（返回**空页**），
  **不是**「整条日志」—— 所以往回翻不能靠 -1 收尾
- 下一页的游标是**本页第一条记录的 seq 减一**。上游 `paginate` 返回
  `events.slice(cut, end)`，`cut` 就是第一条事件的 seq（日志 seq 是稠密零基前缀，
  位置即 seq）；而 `page` 会校验 `sourceLog[throughSeq].seq === throughSeq`，
  差一个就报 `session log does not contain through seq N`
- 形参名是 `request`，**不是** `session/list` 那个 `_request`

**已实现**（`OpenAsync(loadAllHistory: true)`）：翻到底再按时间顺序折叠 ——
快照那一页必须先攒着不折，因为往回翻的页比它更早。翻页循环有**无进展就停**的闸
（页取空、或游标没往回走），上限 500 页只是保险。

**子 agent 要父地址**（**已实现**）。对子 agent 用 `{kind:'session', sessionId}`
会被内核拒绝：`subagent Sessions require their durable parent address`。
地址是另一种形状：

```
{ kind: 'subagent', parentSessionId, childSessionId, mode: 'one-shot' | 'continuable' }
```

**`mode` 来自 `projections.values.subagent`** —— 注意这个位置：
上游 schema 打印出来的缩进让人以为描述符是列表项的**顶层**字段（`items[].subagent`），
照那个读法 `o["subagent"]` 永远是 null，44 个会话会一直用普通地址去开、
一直报同一句错。**是原文纠正了这次误读**：`--harness-list-raw` 把真正解析的
位置打出来，一眼看到连一个**空会话**的 values 里都有 `"subagent": null`。

另一条同源的：**`projections.values.title` 就是真标题**。
「列表上拿不到标题」那句话说的是**扫磁盘**那条路（`DshSessionReader.Enumerate`
只读会话头）；内核给的列表**带标题**，harness 那条路的列表可以有真标题。

`HarnessSessionInfo.Raw` 保留了列表项原文 —— 协议无版本协商（P2），
而「schema 有、实际的位置却不是那样」已经栽过一次。

#### P14 · 两份真实日志坏了 —— 报错形状指向**两个写者**（成因未证实）

`--harness-check-all` 的「打不开」桶里稳定是两个（跑了六七轮，**没有变多**）：

| 会话 | 内核报的 |
|---|---|
| `session-45cb1b3c…` | 起先是 `torn JSONL record`，后来变成 `seq gap … at line 1686 (expected 32358, got 32354)` |
| `session-7d8b4422…` | `seq gap in committed region at line 8881 (expected 170089, got 170085)` |

三条观察，都比「坏了」这个结论有用：

1. **第一个会话的报错会变** —— 因为它是**正在被写**的那个（DSH Desktop 自己那场对话）。
   每读一次，尾部又长了一点
2. **两个报错都是「倒退 4」而不是「少了一段」**：`expected 170089, got 170085`
   说的是下一条记录的 seq 比期望的小 4 —— 那是**回写/重复覆盖**，不是空洞。
   两个不同会话都恰好差 **4**，这不像随机损坏
3. **SDK 那条路的 `log-probe --check-all` 也独立判 7d8b4422 失败**（`轮次计数 7 ≠
   实际最大轮 3`）—— 两份读数都指向它，所以不是本客户端的 bug

**成因没有查证。** 一个能解释 2 的候选是**两个内核同时写同一份日志** ——
而见 P15：我们的探针**恰好会制造第二个写者**。**这是推测，不是证据**：
要证实得能按需复现，而按需复现意味着再弄坏一份真实日志，不做。

能做的、代价为零的那件事已经做了：探针现在会**明确警告**
「follow 会 promote 冷会话，DSH Desktop 正开着时建议先关掉」。

#### P15 · `session/follow` 不是只读 —— 它**激活**会话

上游 `history.js` 的 `follow` 在发完快照后：

```js
if (address.kind === 'session' && source.source === 'prepared') {
    const promotion = source.retain();
    this.promote(promotion);   // "starts ordinary Session activation after snapshot delivery"
}
```

也就是说对一个**冷会话**（在磁盘上、进程里没有活动实例）follow 一下，
这个内核就**接管**它了，之后它会往同一份日志里写。

**这条对工具的性质有影响**：`--harness-check-all` 打开**每一个**会话（现在是 73 个），
就等于让第二个内核接管 73 个会话 —— 它不是「扫一遍日志」那种只读检查。
DSH Desktop 正在跑时，两个内核会碰同一份 `DSH_HOME`。

要「只读地」看一眼历史，用 `--load <id>`（那条路直接解 zstd，不经过内核）；
`--harness-*` 那几条都会起内核、都会 promote。参 P14。

#### P16 · 实时帧也带包装 —— 而且 `onEvent` **只在投影变了时才回调**

两条都是第一次**真发消息**（A3）时当场踩出来的，而它们此前一直在代码里躺着。

**一、实时帧与快照 record **同形**：`{ type:'event', event:{…} }`。**
`PumpAsync` 曾经把这个**包装**直接喂给 `ToEvent`（它读的是事件本身：
`type`/`seq`/`time`/`data`），于是每一帧都被读成 `type = "event"`、
没有 seq、没有 data。症状极具误导性：

```
投影收下 16 帧，一条消息都没长出来；
事件直方图只打出孤零零一个 `event×16`
```

界面上就是「消息发出去了，然后一直不动」。**而这个 bug 之前一直被对账漏过去** ——
`--harness-check-all` 走的是快照那条路（`ApplyRecord` 读 `entry["event"]`，是对的），
一次都不经过 `PumpAsync`。**对账绿不等于这条路径绿**：它只覆盖它走的那条路。

**二、`onEvent` 只在投影真的变了的时候回调。**
`turn/end`、`step/end` 这类事件不改视图，所以**不会**触发它。
「收到 turn/end 就停」写在 `onEvent` 里就永远不会触发 —— 实测直方图里
明明有 `turn/end×1`，循环却等到了 3 分钟超时。要等某个事件得用
`PumpAsync(..., stopWhen:)`，那个谓词每个事件都过。

**三、顺带一条断言纪律**：把模型改回正确值后仍然报「模型失败」，
因为断言扫的是**整个视图**，而上一轮**故意造的**那个失败还留在历史里。
**功能是好的，断言读错了范围**（与 U17 同类）。凡是「这一轮」的断言，
都得先记住发消息前的条数，只看新增的那一段。

## 提交规范

**Conventional Commits**：`feat:` / `fix:` / `docs:` / `chore:` / `refactor:` /
`test:` / `build:`。**正文用中文**，关键设计决策**必须记录原因** ——
让一年后读 `git log` 不需要开 PR。

**只有用户明确要求时才提交。** 获准提交后，一个 section / 逻辑单元一个 commit。
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

### U10 · 主题切换的应用时机与作用对象

这里踩过两次，都是顺序或作用对象不对：

- 应用到 `Window.Content`（那个 `Frame`）**无效** —— `NavigationView` 是它的
  **父级**，而 `RequestedTheme` 在视觉树上只向下传播
- 必须在 `InitializeComponent()` **之前**设，新页面才能带正确主题创建

**当前做法**：`ShellPage` 构造函数里、`InitializeComponent()` 之前调
`Settings.ApplyTheme(this)`；设置页改主题后调 `ShellPage.ReloadTheme()`。

**窗格背景**（曾经不跟随，已修）：`NavigationViewDefaultPaneBackground` 是平台
在加载期用 `StaticResource` 解析的 AcrylicBrush，事后改 `RequestedTheme` 不会让
它重新解析 —— 选浅色后内容区变亮、窗格仍黑。现在 `Themes/Flat.xaml` 里显式把它
指向跟随主题的纯色令牌。

这是**有意识地破了 U1**「不往平台外观上塞自定义配色」：那条规则的目的是保住
「跟系统走」，而一个不跟随主题的窗格恰恰破坏了这一点。用系统自己的令牌覆盖，
仍然跟系统走。

**实测**：内容区与导航窗格均正确切换，且**跨重启持久化**。

### U11 · 拿「右键点的是哪一行」不能用 `RightTapped`

UWP 不提供「被右键的那一项」。`ContextFlyout` 挂在 `ListView` 上时，菜单项的
Click 处理器只能读 `SelectedItems`，而**右键不改变选中状态** —— 先点 A 再右键 B，
「重命名」「删除」会作用在 A 上。这是很隐蔽的错：单次使用看不出来。
WPF 用 `Style` 的 `EventSetter` 解决，**UWP 不支持 `EventSetter`**。

**走过两条错路，都实测无效：**

1. 在 `GetContainerForItemOverride` 里给容器
   `AddHandler(RightTappedEvent, ..., handledEventsToo: true)` —— 右键后拿到的目标
   仍是 `null`（诊断输出 `sel=0 ctx=null tg=0`）。`RightTapped` 是**手势识别事件**，
   用合成鼠标消息（`mouse_event`）驱动时不一定会走完手势识别管道
2. 读 `SelectedItems` —— 语义错误，见上

**可行做法**（`Controls/SessionListView.cs`）：

- 在容器上记 `PointerPressed` 的坐标（原始指针事件，合成消息也走）
- 菜单 `Opening` 时用那个坐标遍历容器、`TransformToVisual` 换算相对矩形、
  取命中的那一行

两条附带经验：

- 容器矩形用 `TransformToVisual` 换算成**相对本控件**的坐标，不读屏幕坐标 ——
  免得多显示器与 DPI 缩放把换算搞错
- 调试这类问题时**在页面上放一个诊断 `TextBlock` 把判定过程写出来**，
  再用 `UiaRect.ps1` 读它。比猜快得多：这次一眼就看出是 `ctx=null`

### U12 · 纯图标按钮必须显式给无障碍名称

`Button` + `SymbolIcon` 不带任何文本，**`ToolTipService.ToolTip` 不构成
无障碍名称**。缺了它：

- 屏幕阅读器只念「按钮」，用户不知道是哪个
- UI Automation 按名字找不到它 —— 实测 `Uia.ps1 -Invoke "返回"` 报
  `element not found`，排查时很容易误以为是按钮没渲染

**写法**：`AutomationProperties.Name="返回会话列表"` 显式设上。图标按钮一律要加。

### U13 · 输入区没有落在窗口外 —— 是**量测工具**的坐标空间错了

这一条曾经作为「唯一未修缺陷」记了很久，**结论是错的**。写清楚是因为那个错误
本身比缺陷更值得记住。

**当初的实测**（`uwp/tools/shot/Geometry.ps1`，同一次进程内读窗口矩形与 UIA 矩形）：

```
实际窗口   rect=-7,-7  size=1550x830   dpi=120
UIA 窗口   -9,-9  1938x1038
输入框     421,964  1403x41      ← 964+41 = 1005 > 830，判为「被裁掉」
```

**真相**：那两个尺寸差**恰好 1.25 倍**，也就是显示器的缩放比。根因是
`GetWindowRect` 在**未声明 DPI 感知**的进程里被系统虚拟化，返回的是按 96 DPI
换算过的值；而 UIA 的 `BoundingRectangle` 报的是**物理像素**。
两个不同空间里的数字被直接比较，凭空造出一个「多出 200px」的布局。

实情：窗口是 **1938×1038 物理像素**（120 DPI，即 1550×830 DIP），
输入框底边 1005 在窗口底边 1029 **之内**。全屏截图里输入框与「发送」
都在屏幕上、也点得到。

**教训（比这条缺陷本身值钱）**：

- **两个来源的数字要先确认在同一个坐标空间，再相减。** 差一个整数倍就该怀疑
  是 DPI 缩放，而不是去改布局。
- 之前「试过四种办法都没改善」也就解释得通了 —— 改布局当然不会改善一个
  量测错误。**四种无效尝试本身就是「诊断错了」的信号**，当时没被读成信号。
- 新工具 `uwp/tools/shot/Layout.ps1` 第一件事是
  `SetProcessDpiAwarenessContext`，并把「是否拿到了 DPI 感知」打出来；
  同时用 `DwmGetWindowAttribute` 拿扩展边框（UWP 的 `GetWindowRect` 会把
  不可见的 resize 边框算进去）。**量坐标一律用它，不要再用 Geometry.ps1 的
  窗口矩形。**

### U13b · PowerShell 5.1 读 UTF-8 脚本要 BOM

`uwp/tools/shot/` 下的脚本由 `powershell.exe`（5.1）执行，而 5.1 **默认按 ANSI
解码无 BOM 的文件**。后果是脚本里的中文变成乱码，且**报错位置与真实原因无关**：

```
$(if ($inside) { "在窗口内 ✓" } else { … })
→ You must provide a value expression following the '*' operator
```

乱码把 `"` 吃掉了一部分，解析器于是抱怨运算符。**新增 .ps1 一律带 BOM**
（`[IO.File]::WriteAllText($p, $text, [Text.UTF8Encoding]::new($true))`）。
仓库里 `Crop.ps1`、`Ui.ps1`、`ZoomRect.ps1` 三个文件至今没有 BOM。

### U16 · 不要把构建输出用 `Select-Object -Last N` 截掉再 `--no-build` 跑

`dotnet build … | Select-Object -Last 5` 会在构建**失败**时把 error 行截掉，
而 `dotnet run --no-build` **不校验产物是否比源码新** —— 于是它照常跑上一次的 dll。

后果不是「跑不起来」，而是**「我刚修好的 bug 还在」**：看起来像修复无效，
实际是修好的那份根本没被编译进去。为这个白花了一轮。

**做法**：跑协议层验收用 `uwp\tools\sdk\run-cli.ps1` —— 它显式检查
`$LASTEXITCODE` 并 `throw`，且只在紧接一次成功构建之后才允许 `--no-build`。

### U17 · 自动化断言要看**对**元素，否则得到假阴性

写 UIA 验收脚本时踩了两次同类错，都是「断言读错了地方」：

| 症状 | 真正的原因 |
|---|---|
| 对 `ListItem` 取 `InvokePattern` 报 `Unsupported Pattern` | ListView 的行是 `SelectionItemPattern`；要**往上找**第一个能应答的元素（`Uia.ps1` 的 `Get-Actionable` 是对的） |
| 「最后一条里没有刚发出的文本」但界面明明有 | `ListItem` 自身的 `Name` 是**数据对象的类型名**（`ArcDeck.ChatMessage`），正文在它**子树**的 `Text` 元素里 |

**对照实验与断言之间要确认「读的是同一个东西」。** 这两个都表现为
「功能坏了」，而功能是好的 —— 与 U13 同类，只是错在元素层次而不是坐标空间。

### U18 · 完整信任伴随进程：方向、拉起顺序与生命周期

UWP 的 AppContainer 起不了 `dsh`、读不到包外路径（P1）。现在由包内
`Host\ArcDeck.Host.exe`（完整信任）承担执行与文件访问，界面经 AppService 与它说话。

1. **provider 必须是 UWP**（服务只能由包声明）；伴随进程是 client，用自己的包身份
   `OpenAsync` 连回来 —— 完整信任进程**不能**当 provider。
2. **先 `uwp\host\publish.ps1` 再 MSBuild**；顺序反了包能装上，但
   `FullTrustProcessLauncher` 找不到 exe。
3. **`ServiceClosed` 里退出进程**；不退的话下次装包被它锁文件，部署报
   `0x80073D02`（实测踩过）。

另两条：`FullTrustProcessLauncher` 要显式引用 `Windows.ApplicationModel.FullTrustAppContract`
的 winmd（UWP 引用集停在 15138，不引用报 `CS0103`，像拼错类名）；AppService
**不保证调用方是本包**，`OnBackgroundActivated` 里比对 `CallerPackageFamilyName`。
线协议用 `ValueSet` 扁平键 + `Windows.Data.Json`，不要用 `System.Text.Json`
（.NET Native 没有）。证据与验收见 [HOST_VALIDATION §七](docs/HOST_VALIDATION_2026-10-15.md)
与设置页「拉起并检查」。

### U14 · 资源键写错不会报错，只会静默回退

`{ThemeResource KEY}` / `{StaticResource KEY}` 指向**未定义**的键时，UWP **不报错** ——
目标属性保持继承值或默认值。所以一个不存在的画刷名表现为「颜色碰巧对」或
「文字看不见」，构建输出里一条提示都没有。

**真实案例**：项目里有三个键在任何地方都不存在：

```
BodyStrongTextBlockStyle     16 处
TextFillColorSecondaryBrush  20 处
TextFillColorTertiaryBrush   20 处
```

查证三处定义来源，全部 0 命中：SDK 的 `themeresources.xaml`、SDK 的
`generic.xaml`、WinUI 2 的 `Themes/Generic.xaml`。它们是 **WinUI 3** 的东西，
被误用到 UWP + WinUI 2 项目里。这 56 处引用没有一处被编译器或运行时提示过。

**工具**：`uwp/tools/shot/ResourceAudit.ps1` —— 把项目用到的每个资源键与平台 +
WinUI 2 的真实定义比对，同时校验 `SymbolIcon Symbol` 是否为真实枚举成员。
**新增任何资源引用或 SymbolIcon 之后跑一次。**

它有两类已知误报，都已在脚本里处理：项目自己 `Page.Resources` 里定义的键
（算作已定义），以及 `SystemColor*` 系列 —— 那些由 XAML 框架**运行时注入**，
SDK 里 152 处引用、0 处定义。

### U15 · 文字色不能用半透明色，背景不确定

**不要**用平台的半透明文字色（`SystemBaseMediumColor` = 深色 `#99FFFFFF` /
浅色 `#99000000`，以及 `SystemChromeDisabledLowColor` 这类按「背景 = 纯白或
纯黑」标定的值）。半透明色的最终观感取决于它压在什么上面。

**实测**：本应用的内容背景是 `#808080` 的中灰 —— 亚克力/材质与窗口内容混合的
结果，与 `ApplicationPageBackgroundThemeBrush` 的纯黑/纯白都不一致。

```
半透明方案一  #99FFFFFF 压在 #808080 上 → #959595   对比度 1.00  完全看不见
半透明方案二  #7A7A7A   压在 #808080 上 → 1.09      仍不可读
```

第一次修复就是踩了这个坑：给缺的键填了平台的半透明色，结果「字看不见」照旧。

**做法**：既然背景不确定，就不再假定背景 —— 直接给**不透明**、且按对比度算过的
色值（见 `Themes/Flat.xaml` 的 `TextFillColor*`）。

**工具**：`uwp/tools/shot/Contrast.ps1`（按 WCAG 相对亮度算比率），区域列表在
`contrast-regions.txt`。AA 正文需 ≥4.5，大字 ≥3.0。

两条使用注意：区域要**贴着实际文字**裁 —— 裁到背景上会量出误导性的 1.00；
区域列表放文件里而不是命令行参数，因为 `pwsh -Command` 会把分号分隔的参数
重新解析，报出与真实原因无关的 "cannot convert to Int32"。

**改任何文字色后都要跑它。「看着还行」与「对比度 1.09」在截图缩略图上没有区别。**

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
| **TUI 真终端可用性** | ✅ **已实测**（`crates/arc-deck-tui/tools/console-probe.ps1`，隐藏控制台 + 写按键 + 读屏幕）：交互内核就绪 / `@` / 帮助 / `/sessions` / `/runs` / **同一会话两轮（42 → 43）** / Esc 取消这一轮 / **跨进程恢复会话** / 退出后无残留内核；第七轮（密度照 OpenCode 收）后加测 stash / `/search` 真内核 / `/subagents`；第八轮补输入框底色 / 扫描条 / 一次 Enter / Agent 颜色；**227 个 TUI 测试**，见 [M4_PLAN 第七、八轮](docs/M4_PLAN.md) |
| **TUI 工具视图与模式** | ✅ 代码 + 夹具实测：按类别的工具摘要、着色 diff、折叠态可见失败、全屏查看器（`/diff` `/output`）；§7.4 六种模式转换、运行中先确认、Plan 在执行层只读（`policy_for` 单测：写范围清空 / shell 行摘掉）。子 agent 会话经 `/subagents` 打开（父地址由服务配，**从工具块直接进入未做**）；**鼠标点选不做**（用户裁定纯键盘，鼠标只留滚轮，复制用 `/copy`） |
| **confirm 审批** | ✅ 通道两侧都有证据：假 WS+HTTP 上 `$events` → `approval/request` → `$events/result` 逐字断言；真内核上 `$events` 首帧 `ready` 带 `clientId`；`approval: ask` 的 patch 真启动过、无激活警告。**没有**在 `confirm` 档下真发一条会触发提权的模型请求（要配额） |
| **交互会话层**（Rust Remote：`arc-deck-dsh::remote`） | ✅ **已实测**：web 内核启停、换票、mux、会话操作（live 不耗配额）；真实一轮流式 → 权威全文替换（quota）。见 [REMOTE_0.2.1](docs/REMOTE_0.2.1.md) |
| **GUI 后端接口** `arcdeck serve --stdio` | ✅ **已实测**（握手 / 坏行不断开 / 未知命令被容忍 / EOF 收内核放锁）；共享 socket 模式**未实现**。见 [GUI_PLUGGABLE](docs/GUI_PLUGGABLE.md) |
| `uwp/` 编译与打包 | ✅ 从零 Rebuild 无错误无警告，产出 MSIX（0.1.13.0） |
| `uwp/` 启动运行 | ✅ **已实测**（签名 → 覆盖安装 → 启动，无 crash.log） |
| `uwp/` 对话页块渲染（段落/代码/工具卡） | ✅ **已实测**，逐块截图核对 |
| `uwp/` 对话页 **Markdown 渲染** | ✅ **已实测**（标题 / 列表 / 斜体 / 行内代码 / 引用 / 链接 逐项核对） |
| `uwp/` **浅色与深色下的文字可读性** | ✅ **已实测**：对比度 4.4–5.3（改前多处 1.00，完全不可见） |
| `uwp/` 资源键有效性 | ✅ **已实测**：`ResourceAudit.ps1` 全绿（改前 3 个键从未定义、56 处静默回退） |
| `uwp/` 会话列表 + 新建会话 | ✅ **已实测**（点「新会话」正确进入新会话，标题/轮数正确） |
| `uwp/` 会话**右键命中目标行** | ✅ **已实测**（先点第 1 行再右键第 3 行，置顶作用于第 3 行） |
| `uwp/` 重命名 | ✅ **已实测**（改标题 → 保存 → 列表标题随之变更） |
| `uwp/` 删除 | ✅ **已实测**（确认对话框措辞正确；取消不删、确认后 5 → 4 个会话） |
| `uwp/` 置顶 | ✅ **已实测**（置顶后升到第一并显示图钉图标） |
| `uwp/` 搜索筛选 | ✅ **已实测**（输入即筛选，`匹配 0 / 4 个会话`，空状态文案正确） |
| `uwp/` 工具页（工具清单 + 详情） | ✅ **已实测**（10 个工具、4 个「需确认」、详情三行 + 说明齐备） |
| `uwp/` 设置持久化（主题） | ✅ **已实测**：内容区切换 + 跨重启保持 |
| `uwp/` 主题对导航窗格 | ✅ 已修（`Flat.xaml` 覆盖 `NavigationViewDefaultPaneBackground`；见 U10） |
| `uwp/` 对话页**输入区可见性** | ✅ **不是缺陷**：原来的「落在窗口外」是量测工具坐标空间错的产物（U13） |
| `uwp/` 对话页**发送通路** | ✅ **已实测**（`SendCheck.ps1`：进入会话 → 写输入框 → 点发送 → MessageList 5→6 条且正文正确 → 输入框清空）。⚠️ 修过一处：`Send()` 原先**没有**调 `Session.Touch()`，列表摘要与轮数不跟着更新 |
| `uwp/` 多选批量操作 | ✅ **已实测**（`CanSelectMultiple=True`；选中两行后批量置顶，两行都升到顶部且带图钉；多选时「重命名」正确置灰） |
| `uwp/` 外观 | ⚠️ 只做到圆角归零。图标字形与控件模板形状改不动，见 U3 |
| 无障碍 | `kanesumi/` ❌ 没有（自绘路线最大缺口）· `uwp/` ✅ 平台提供，图标按钮已补 `AutomationProperties.Name` |
| `arc-deck-dsh`（协议客户端） | ✅ **已实测**：找到内核 → 起 `dsh --profile sdk` → 握手 → `session/prompt` → 流式事件 → 投影出助手正文（`dsh-cli`，见下） |
| `arc-deck-dsh` 接进界面 | ❌ **未做**：协议层仍没有被任一界面引用。P1 的结构性障碍已由伴随进程绕开（见下一行），但「界面接真实数据」是另一件事 |
| `uwp/` **完整信任伴随进程 + AppService 通道** | ✅ **已实测**（0.1.18）：设置页「拉起并检查」→ `FullTrustProcessLauncher` → 伴随进程 `OpenAsync Success` → `ping` 报告 `packaged:true`、`env` 读到包外 `%APPDATA%`、`probe` 真起 `dsh --version`（exit 0）；关掉 UWP 后伴随进程收到「服务关闭」并退出。设计与坑见 [HOST_VALIDATION §七](docs/HOST_VALIDATION_2026-10-15.md) 与 [uwp/README](uwp/README.md) |
| `arc-deck-dsh` 会话重载（读回已存会话） | ✅ **已实测**：真实日志全部通过重放不变式（**现有 71 份里有 1 份失败** —— 那份日志确实坏了，两份独立读数都指向它，见 P14）；`dsh-cli --verify-reload` **同一条会话**实时 vs 重放逐块一致（参 P7/P9） |
| `arc-deck-dsh` 会话列表（列磁盘上的会话） | ✅ **已实测**：`--list` 列出会话并标出属于当前工作区的那些。只读会话头、**不解压全文**；**这条路**上拿不到标题（真标题要重载才知道，见 `DshSessionReader.Enumerate` 的注释）—— 内核那条路能拿到，见下面一行 |
| `arc-deck-dsh` 工具卡 / 思考块 | ✅ 已实现（工具卡带标题与结果、思考块默认折叠）。界面侧未接，所以「折叠」这个交互本身**没实测** |
| `arc-deck-dsh` **harness web / Remote 层**（会话列表、打开、翻页、对账） | ✅ **已实测**：**73 个会话里 71 个逐块对账一致、0 不一致**（`--harness-check-all`，往回翻 132 页）；`--harness-verify` 单条会话「两条路一致」。路上修掉三处：`follow` 打开挂死（P11）、WS 分块撕开汉字（P12）、子 agent 地址读错位置（P13） |
| `arc-deck-dsh` harness **会话列表带真标题** | ✅ **已实测**：标题就在 `projections.values.title`（`--harness-list-raw` 打出原文确认）。**扫磁盘那条路仍然拿不到标题** —— 两条路不要混 |
| `arc-deck-dsh` harness **发消息**（`--harness-prompt-live`） | ✅ **已实测**：真发一条 → `accepted=true` → live 事件 10 种 → 收到 `turn/end` → 这一轮新增 2 条消息、无失败块。**第一次跑就抓出两个 bug**（实时帧没拆包装、`stopWhen` 缺失，见 P16） |
| `arc-deck-dsh` harness **模型失败的出口**（P3） | ✅ **已实测**（对照实验）：故意把模型填成 provider 名字 → 失败以 `finish.reason.failure` 的原文出现在 `assistant/chunk:finish` 块里，检查抓住了它。**这条断言有牙齿**，不是论证 |
| `arc-deck-dsh` harness **当轮作用域的断言** | ⚠️ 踩过一次假阳性：断言扫了整个视图，把上一轮**故意造的**失败也算了进来，于是修复后仍报「模型失败」。已改成只看发消息后新增的那一段（参 P16 第三条） |
| `arc-deck-dsh` harness **往回翻页**（`session/page`） | ✅ **已实测**：`loadAllHistory` 翻到底再按时间顺序折叠；73 个会话全部「历史取全=True」，最多的一个 17 页（参 P13） |
| `arc-deck-dsh` harness **mux 分流**（一条连接两条流） | ✅ **已实测**（`--harness-mux-check`：两条流并发等，各自快照的 `header.id` 与请求逐字相同）。修前所有流共用一个队列 |
| `arc-deck-dsh` harness **子 agent 会话** | ✅ **已实测**：44 个子 agent 用**父地址**形状打开并对账通过。`mode` 从 `projections.values.subagent` 读（位置曾经读错，见 P13） |
| `uwp/` 数据 | ⚠️ 全部是示例数据（`Models/Store.cs`、`Store.Tools`），未接协议；协议层已能供数据但**尚未接线** |

### 还没实测的部分

上表标 ⚠️ 的项，含义是**代码写完了、能编译，但没有任何一次运行证明它行为正确**。
目前没有 ⚠️ 的未测项。

标 ❌ 的项是**没做**，不是没测 —— 别把两者混起来。

原先挂着的「对话页**发送消息**后摘要是否更新」也测过：当初判它「卡在 U13」
是相信了那条错误的量测 —— 输入框一直在窗口内（参 U13）。



## 构建

```bash
# headless 日常用法（PRODUCT_PLAN §6.0）。首次：装运行时、写配置、存密钥。
cargo build -p arc-deck-cli --release && cp target/release/arcdeck.exe ~/.cargo/bin/
arcdeck runtime install            # 钉住的 DSH 装进 %LOCALAPPDATA%\arcdeck\runtime\
arcdeck config init                # %APPDATA%\arcdeck\arcdeck.json；Ether 用 examples/ether/arcdeck.json
arcdeck auth set opencode-go       # 密钥从 stdin 读
arcdeck run task.md --worktree dsh/<id> --accept "cargo test -p x" --detach
arcdeck status --all               # 每件一行；单件时退出码 0/1/2/3/4/5/10/11
arcdeck report <id>

# **真实验证前先重编 release。** 改了代码却拿旧的 `target/release/arcdeck` 去跑真实任务，
# 看到的「没生效」其实测的是旧二进制 —— 与 U16「`--no-build` 跑旧 dll」同一类。
# `cargo test` 走 debug，真实场景走 release，两者不会互相更新。（2026-10-14 为此白跑两轮。）

# Arc Deck 产品 crate（core / dsh / cli）—— TUI 与 headless 的那一半
# `--features arc-deck-cli/test-runtime` 是必需的：假 SDK 对端与隐藏子命令
# 都在那个 feature 后面，发布构建里不存在它们。
cargo test -p arc-deck-core -p arc-deck-dsh -p arc-deck-cli \
    --features arc-deck-cli/test-runtime --offline

# 端到端离线验收（不发任何模型请求）。重复跑要先清状态目录，否则第二次
# 以 TASK_ALREADY_SEEN 退出 64 —— 见 docs/PRODUCT_PLAN.md §10.2。
cargo run -p arc-deck-cli --bin arcdeck --features test-runtime -- worker run \
    --config tests/fixtures/worker-offline.json \
    --task tests/fixtures/worker-task.json --format jsonl --result result.json

# 早期自绘 GUI 骨架（kanesumi 那一支，已暂停新增功能）
cargo test
cargo run --bin arc-deck-demo
cargo run -p layout-bench --release
```

```powershell
# 协议层（arc-deck-dsh）—— 与界面无关，不需要窗口
# 必须用这个包装脚本：它会在构建失败时炸，而不是拿旧 dll 继续跑（参 U16）
powershell -File uwp\tools\sdk\run-cli.ps1 --list                      # 列磁盘上的会话
powershell -File uwp\tools\sdk\run-cli.ps1 --load <会话 id>             # 重载一个已存会话
powershell -File uwp\tools\sdk\run-cli.ps1 --prompt "…" --verify-reload # 发一条并比对实时/重放

# harness web 那条路（Remote 层，完整功能面）。它会起一个 `dsh web` 子进程 ——
# **需要写 <DSH_HOME>\profiles\web**（内核每次启动都重写 cordis.yml），
# 所以文件沙箱是 workspace-write 时它会以 EPERM 失败在 prepareProfile 上。
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-list                # 内核给的会话列表
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-list-raw            # 把「我们解析的那几个位置」打出来
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-verify <会话 id>     # follow 投影 vs 日志重放
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-check-all           # 全部会话逐一对账
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-mux-check           # 一条连接上同时开两条流，看串不串帧

# 真发一条并消费 live 事件（A3）。**注意 `follow` 会 promote 会话**，所以这几条
# 不只是读；发消息那两条还会真的消耗模型配额，跑之前想清楚。
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-prompt-live "收到"                          # 自动挑一个空会话
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-prompt-live "继续" --harness-target <id>     # 指定会话
powershell -File uwp\tools\sdk\run-cli.ps1 --harness-prompt-live "x" --harness-target <id> `
    --harness-model deepseek-official/deepseek-official    # 对照实验：故意用错的模型，看 finish 失败块

# 重放不变式：对全部真实日志跑一遍（这是最接近回归测试的东西）
dotnet run --project arc-deck-dsh\tools\log-probe -- --check-all "$env:APPDATA\dsh-desktop\harness\sessions"
# 对照实验：展开打包行 + 按最朴素的方式消费增量，看断言会不会响。
# **注意：在当前的日志上它观察不到差异**（参 P9），`log-probe` 会自己把这件事报出来。
dotnet run --project arc-deck-dsh\tools\log-probe -- <日志路径> --check --no-replay
```

> 上面写的是 `powershell`（5.1）而不是 `pwsh`：这台机器上**没有装 PowerShell 7**，
> `pwsh` 会报 `CommandNotFoundException`。`run-cli.ps1` 两个版本都能跑。

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

# TUI 真终端探针（隐藏控制台 + 写按键 + 读屏幕；-Submit 会花一次小配额）
& $ps5 -File crates\arc-deck-tui\tools\console-probe.ps1 -Exe D:\arcdeck\target-main\release\arcdeck.exe
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
