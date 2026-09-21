# Arc Deck · 两条并行工作流的分工

这份文档只回答一个问题：**两个 branch 同时开工时，各自动什么、不动什么、怎么交汇。**
产品与设计规格不在这里（那是 `AGENTS.md` 与 `docs/ARCHITECTURE.md`）。

## 为什么要有这份文档

两条路并行时最贵的错误不是「写错代码」，是**两边同时改同一个文件**——
那会让两边都在解冲突，而冲突的内容与各自要解决的问题无关。
所以下面按**文件所有权**切，而不是按「谁负责哪块功能」切功能描述。

| | Branch A（本机 · Windows） | Branch B（Linux 机） |
|---|---|---|
| 建议名 | `feat/harness-remote` | `verify/linux-crosscheck` |
| 起点 | `main` @ `94af565` | 同 |
| 状态 | **A1/A2 已落地并实测**；A3 未跑、A4/A5 未开工 | **已 suspend（未开工）** |
| 一句话 | 把「接真实 harness」这条路做完，做到界面能吃到数据 | 用第二个平台给 Windows 侧那条路做**独立事实核查** |

---

## Branch A · `feat/harness-remote` —— 做完整功能面

### 为什么要做这条路

SDK stdio（`dsh --profile sdk`）那条路已实测可用，但它**只能** create-session
然后 prompt：拿不到会话列表、拿不到历史、不能改名、不能取消。
而界面（会话列表、历史回看、重命名、置顶）需要的正是这些。

harness 自己那份 WebView 客户端走的是 **Remote 层**（`/api` HTTP +
`/api/remote.mux` WebSocket）—— 那才是完整功能面，也正是 DSH Desktop 自己在用的那套。
所以「接真实 harness」以这条路为主，SDK 那条保留为「只要内核、不要 profile」的轻装路。

### 上次卡在哪

`probe-harness7.log`（最后一次实测）停在这里：

```
✓ 已登录 http://127.0.0.1:54501（HTTP 303，cookie 长度 226）
✓ session/list：70 个会话（0 个运行中，44 个子 agent）
打开会话 arc-20260921024823-…（session/follow）…
✗ 打不开：[follow-timeout] follow 在 60s 内没有给出快照；WS 状态=Open
    11:06:28.076 [recv] 73806 字节 {"type":"item", …, "value":{"type":"snapshot", …
```

**线上已经把快照送到了**（73806 字节那一帧），而客户端报「没给出快照」。
所以这不是内核的问题，也不是协议的解析问题 —— 是我们自己**收下了却没往外走一步**。
根因见下面 A1。

### A 的范围

| | 做什么 | 验收 | 状态 |
|---|---|---|---|
| A1 | 修 `session/follow` 的**打开挂死**（卡点） | `--harness-verify <id>` 不再超时，能打出快照条数与 cursor | ✅ **已实测** |
| A2 | `follow` 投影 vs **日志重放**对账跑通 | 同一会话两条路逐块一致；这是 web 路唯一的正确性证据 | ✅ **已实测**（并扩成全部会话） |
| A2b | 顺手加的：`--harness-check-all`（A2 的「全部会话」版） | 71 个会话逐一对账 | ✅ **已实测**：一致 15 · 不一致 0 |
| A3 | 发一条消息并消费 live 事件（含 P3 的 `finish.reason.failure`） | `--harness-prompt "…"` 收到 `turn/end` 且有事件直方图 | ❌ **未跑** —— 见下 |
| A4 | mux 按 `streamId` 分流（见「已知隐患」） | 一条物理连接上开两条逻辑流，互不串帧 | 未开工 |
| A5 | **之后**：P1 的下一步 —— 全信任桌面外壳引 `arc-deck-dsh` | 另立 section，不在本 branch 的验收里 | 未开工 |

**A3 为什么没跑**：`--harness-prompt` 会往一个**真实会话**里发一条消息，
而目标会话是「第一个非空的非子 agent 会话」—— 也就是用户自己的会话。
擅自跑等于在别人的对话里插一句。代码在，但**一次都没执行过**，
所以它是 ❌ 而不是 ⚠️（参 `AGENTS.md` 状态表的同一行）。

**A 顺带量出来的三件事**（都写进了 `AGENTS.md` P11–P14）：

1. 卡点的根因不是协议、也不是内核 —— 是我们**收下快照后没跳出读循环**。
   契约（来自上游源码）是：快照就是 opening 的全部，之后全是 live
2. WS 分块边界会把一个汉字撕成两个替换字符 —— **中文内容在长消息里会被静默损坏**。
   抓法不是读代码，是两条路对账（A2 的价值当场兑现）
3. 快照是**一页**（`hasMore`），大会话只给最新一页；子 agent 要**父地址**。
   两条都还没做，所以 `--harness-check-all` 现在如实报「只比了一页 10 / 子 agent 跳过 44」

### A 不做什么

- 不碰 `kanesumi/`（Rust 那条线有自己的节奏）
- 不在这一轮做 P1 的外壳（A5 只是记着方向，别把它塞进 A1–A4 的提交里）
- 不改 `uwp/` 现有界面 —— UWP 界面本身是好的，它缺的是数据通路，不是控件

### 已知隐患（A4 的由来）

`HarnessMux.Dispatch` 读了 `streamId` 却**没有用它分流**：所有逻辑流的帧进了同一个
channel，而 `MuxFrame` 里**没有** streamId 字段。当前每个 `HarnessSessionClient`
只开一条流，所以没暴露。但只要有人在同一 mux 上开第二条
（`session/follow` + `session/control` 是界面上迟早要同时有的），
帧就会串到错误的读者手里 —— 而症状是「控制流收到了对话事件」这种**看起来像内核错**的表现。

---

## Branch B · `verify/linux-crosscheck` —— 用第二个平台验 Windows 侧的前提

### 为什么 B 在 Linux 上，却是**辅助 UWP** 的

P1 的结论是：UWP 的界面外壳要换成**完整信任的桌面进程**，`arc-deck-dsh` 作为它的引用。
但那条路有一个至今**没有被验证过的前提**：

> `arc-deck-dsh` 只在 Windows 上跑过。

它能不能在一个别的平台上不做改动地跑起来，决定了将来那个外壳里**哪些代码是「窗口」、
哪些代码是「平台假设」**。这件事的答案只影响 Windows 侧的设计，而验证它最便宜的地方
是 Linux：没有 MSIX 签名、没有包注册、没有截图量坐标，`dotnet run` 就是全部成本。

Linux 侧**不产出界面、不产出 UWP 代码**。它产出的是**事实**，而事实喂给 A 和 UWP。

### B 的范围

| | 做什么 | 产出（可证伪的东西） |
|---|---|---|
| B1 | 在 Linux 上把 `arc-deck-dsh` 建起来并跑 `log-probe --check-all`、`zstd-probe` | 第二平台上的重放不变式结果；与 Windows 侧逐项对照 |
| B2 | **Windows 耦合清单**：`%APPDATA%` 路径、`Kill(entireProcessTree)` 语义、`node.exe`、`X509Store`、路径分隔、`pwsh` vs `bash` profile | 一张表：每一处标「必须改 / 可以留着 / 只影响 Windows」 |
| B3 | 第二份会话日志语料上的不变式 | P9 缺的正是**「权威全文缺席」的日志** —— 换个语料才有机会撞到，那是唯一能让「跳过打包行」被证伪的机会 |
| B4 | （**待确认**）headless 宿主原型：把 AppContainer 干不了的那三件事的平台无关部分先跑通 | 只有确认后才开工，见下 |

### B 不做什么

- **不改 `arc-deck-dsh/src/ArcDeck.Dsh/Harness/**`** —— 那是 A 的所有权，B 只是消费者
- 不碰 `uwp/`（Linux 上编译不了 UWP，改了也无法验证）
- 不产出「建议重构」类结论：要出就出「这一处会坏 / 不会坏」的判断加证据

### 为什么现在 suspend

两条，都是依赖顺序，不是「没时间」：

1. **B 的核对对象还在动。** A 正在改 `Harness/**` 与 CLI，B 现在跑交叉核对是对着一个
   移动的目标 —— 跑出来的差异分不清是「平台耦合」还是「A 正在改的那一半」。
2. **B 现在不阻塞任何人。** A 的卡点在自己的 `follow` 循环里，B1–B3 一条都帮不上。
   让 B 先等着，比让它先跑一遍再重跑一遍便宜。

**恢复条件**：A1–A3 落地之后，A 的接口面稳定，B1–B3 才有意义。
**进展：A1/A2 已落地并实测，A3 未跑**（见上）—— 所以 B 仍未到可以开工的点，
但已经比上次近了一步。B4 需要先明确一件事：原型是要走
`kanesumi/` 的 Rust 外壳，还是走一个新的 C# 无头宿主 —— 这决定了它归哪条线。

---

## 文件所有权

| 路径 | A | B |
|---|---|---|
| `arc-deck-dsh/src/ArcDeck.Dsh/Harness/**` | ✍️ 独占 | 🚫 只读 |
| `arc-deck-dsh/src/ArcDeck.Dsh.Cli/**` | ✍️ 独占 | 🚫 只读 |
| `arc-deck-dsh/src/ArcDeck.Dsh/**`（Harness 之外） | ⚠️ 必要时改，改前在下面记一行 | ⚠️ 只加平台适配，不改折叠规则 |
| `arc-deck-dsh/tools/**` | 用，不改 | ✍️ 加跨平台校验脚本 |
| `uwp/**` | 暂不动（A5 才动） | 🚫 |
| `kanesumi/**` | 🚫 | 🚫（两条线都不碰） |
| `docs/WORKSTREAMS.md` | ✍️ | 🚫 |
| `docs/LINUX_FINDINGS.md` | 🚫 | ✍️ 独占 |
| `AGENTS.md` | 各写各的 section，**不要改对方那段** | 同 |

`AGENTS.md` 是唯一注定会冲突的文件 —— 两边都会加 P/U 段。约定：
**各写各的编号段，谁也不重排谁的**；冲突在合并时按「两段都留」解，不删对方的记录。

## 交汇与合并顺序

`main` ← A 先合（它改的是 `src/`，是别人要消费的东西）← B 后合（它改的是 `tools/` 与 `docs/`）。
B 若在 B2 里发现**必须改** `src/` 的 Windows 耦合点，**不自己改**：
写进 `docs/LINUX_FINDINGS.md` 让 A 改 —— 那样改动有一处出处，也有一次编译验证。

## 这份文档本身的状态

- 分工与所有权是**当前判断**，不是已冻结的契约；改了就在这里改，别在提交信息里悄悄改
- B4（headless 宿主原型）**尚未确认归属**，在确认前它不算 B 的验收项
- 已知的文档漂移：`docs/ARCHITECTURE.md` 把 `arc-deck-dsh` 写成 Kanesumi 分层里的
  Rust crate，而仓里实际的 `arc-deck-dsh/` 是 C# / .NET 9（参 `AGENTS.md` 的 P 段）。
  两者要么是同一件事的两个时期，要么是两个东西 —— **没有查证过**，所以两条都留着，
  不在这里替它下结论
