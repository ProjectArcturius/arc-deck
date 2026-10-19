# M4 · TUI 完整落地的派工计划

日期：2026-10-14 起。用户裁定：TUI 做到 PRODUCT_PLAN §7 **完全完成**，用 `arcdeck` headless 派工人干
（`opencode-go/deepseek-v4.1-flash` · `effort high` · `--permission full --worktree dsh/<id>` · 每件都带 `--accept`）。
Claude 是派工者：写任务书、复跑验收、逐行审阅、在工人分支上提交后挑进 main。工人不提交（`allow_commit: false`）。

任务书在 `tasks/m4/`，共用条款在 `tasks/m4/_common.md`。**本文件只记计划与状态；证据在各提交正文里。**

## 当前状态：队列暂停（2026-10-14 用户裁定）

> 「目前在做的任务书完成后不再派发新的了」。顺序 0 与 w4-m3b-negative 收完即停；其余任务书**原样保留**，
> 由替班（sol）或下一次调度按 `tasks/m4/README.md` 继续。本轮的使用复盘与对 Ether 的意义：
> [`ARCDECK_WORKERS_REVIEW_2026-10-14.md`](ARCDECK_WORKERS_REVIEW_2026-10-14.md)。

## 「完全完成」的判据

PRODUCT_PLAN §7.3 的八个组件、§7.4 的键位与命令、§7.5 的视觉与运行验收。其中有三样不是 TUI crate 自己能完成的：

| 依赖 | 为什么 | 波次 |
|---|---|---|
| Rust Remote 适配（会话列表 / follow / 翻页 / 发消息 / 取消 / 模型目录） | 多轮会话、流式正文、思考折叠、会话列表、真实模型目录都只在 Remote 层有 | W2–W3 |
| M3 执行 guard（EffectivePolicy → DSH 工具执行限制） | `/plan`、readonly、确认审批在 TUI 上「可用」的前提是执行层真的拒绝 | W4 |
| 视觉对照 | §7.5 要同终端、同尺寸与 OpenCode v1.19.0 对照；**满意与否由用户判断** | W5 |

## 波次与状态

**派工手册**（队列依赖、派工命令、收工审阅流程、踩过的坑）：[`tasks/m4/README.md`](../tasks/m4/README.md)。
「日常可用」的七条判据也在那里 —— 用户 2026-10-14：做到能像 OpenCode 一样日常写代码、可以转去专注 GUI 的程度，
剩余调度可以交给替班（sol）。

| 顺序 | 任务 | 内容 | 状态 |
|---|---|---|---|
| 试点 | tui-model-catalog | 模型目录纯逻辑（§7.2） | ✅ 202b551 |
| 0 | w1-markdown | 助手正文 Markdown（pulldown-cmark） | ✅ 6cf64bd |
| 0 | w1-theme | 2.0.27 语义 token、v1→v2 迁移、内置主题、主题对话框 | ✅ e16e13d（合入时整合了取色接口） |
| 0 | w1-model-dialog | 模型 / provider / effort 对话框，F2 循环，偏好落盘 | ✅ f82555f |
| 0 | w2-remote-transport | Remote host / rpc / mux；0.2.1 协议核对文档 | ✅ 0774e85（收下工人的两个 wip 提交，修了挂死：假服务端继承非阻塞 + 客户端握手无超时；`docs/REMOTE_0.2.1.md`） |
| 0 | w4-m3-guard | EffectivePolicy → DSH 配置行，映射表 + `--dump-config` 验证 | ✅ f545990（合入后真实冒烟一次通过） |
| 1 | w3-commands | 命令系统：action 注册表、键位与上下文、slash 补全、面板、帮助 | ✅ 工作区（未提交）：`commands.rs` / `keymap.rs` / `autocomplete.rs`，160 个 TUI 测试 |
| 1 | w2-remote-session | 会话操作、事件投影、审批数据、模型目录 | ✅ 0774e85（`SessionClient` + `fold` + `core::conversation`；真实抓帧当夹具）。**审批数据未做** |
| 1 | w4-m3b-negative | 脚本化假模型端点驱动真实 DSH，证明越权被拒（零配额） | ✅ fb4d482（17 条负例 live 全过；复跑时修了一条用例） |
| 2 | w3-tui-input | `@` 文件、历史、粘贴摘要、Shift+Enter、外部编辑器、metadata | ✅ 工作区（未提交）：`files.rs` + 编辑器原子片段；177 个 TUI 测试。**stash 未做**；Shift+Enter 只在代码里启用了 kitty 协议，**真终端未实测**（见下） |
| 2 | w3-session-core | TUI 改走 Remote：多轮、流式、思考、取消；`/runs` 保留看工人 | ✅ 358425a（经 `core::service` 消息与 `remote::service` 说话 —— GUI 将来用同一份） |
| 3 | w3-session-list | 会话列表 / 恢复 / 改名 / 时间线；真实模型目录 | 🟡 358425a：列表（本工作区）/ 恢复（跨进程实测）/ `/rename` / 内核目录已做；**时间线、搜索会话内容未做** |
| 3 | w3-tool-views | diff、shell 输出、折叠、完整输出查看器 | 待 w3-session-core |
| 3 | w4-tui-modes | Agent / Plan / Build / 授权级别 / confirm 审批界面 | 待 w3-session-core · w4-m3b-negative |
| 4 | w5-polish | 信息栏真实数据、首次运行与故障提示、终端恢复、toast、复制、长会话性能 | 待顺序 3 全部 |
| — | 人工 | Arch 真终端、与 OpenCode v1.19.0 同终端对照（§7.5）；满意与否由用户判断 | 队列做完后 |

## TUI 真终端证据（2026-10-20，会话模式）

`console-probe.ps1` 改为会话模式后的实测（release 二进制）：

| 检查 | 结果 |
|---|---|
| 交互内核就绪（后台起，界面不等） | ✅ |
| `/sessions`（会话列表）· `/runs`（工人运行） | ✅ |
| **同一会话两轮**：问 7×6 → `42`；只说「再加 1」→ `43` | ✅ 答案不在题面里，只能来自上一轮 |
| 长输出中 Esc → 「已取消」，输入区仍可用 | ✅ |
| **新进程里** `/sessions` 打开上次会话 → 历史与「已取消」都回来 | ✅ |
| Ctrl+C 退出后无残留 `--profile web` 的 node 进程 | ✅ |

`-Submit` 两次、`-Cancel` 一次极小调用；`-Resume` 不耗配额。探针自身修过一处：`KEY_EVENT_RECORD` 没声明 CharSet，
中文按键按 ANSI 封送报 0x8007007A（之前只打过 ASCII）。

**交互面还没有的**（日常可用判据里对应的几条）：confirm 审批界面（判据 3）、diff 与完整工具视图（判据 6）、
会话时间线与内容搜索（判据 2 的一部分）、个性化 persona 注入交互面、子 agent 会话的打开、`session/control` 的实时用量。

## TUI 真终端证据（2026-10-15）

`crates/arc-deck-tui/tools/console-probe.ps1` 起一个**隐藏控制台**、把按键写进它的输入缓冲区、
再读回屏幕缓冲区 —— 把「在真终端里可用」做成几条可复现的断言。实测（release 二进制）：

| 检查 | 结果 |
|---|---|
| Home 渲染 | ✅ `Arc Deck` |
| `@` 文件补全 | ✅ `@ 文件引用` 列真实仓文件 |
| `F1` 帮助 | ✅ |
| 输入可见 | ✅ |
| `/sessions` 面板 | ✅ 打开「运行历史」 |
| **真提交一轮**（`-Submit`，发 `say OK`） | ✅ `运行已结束`，正文 `OK`，无错误提示 |
| **取消**（启动中 `Esc`） | ✅ 界面显示取消，`status.json` → `cancelled`；退出后无残留 `arcdeck` 进程 |
| `Ctrl+C` 退出（含收尾） | ✅ 进程退出 |

`-Submit` 会消耗一次小配额，默认关闭；不加它只跑界面检查。

**同轮发现（尚未修，属运行时而不是 TUI）**：DSH `0.2.1-alpha.2` 在**写出结果之后**以
`0xc0000409`（libuv 断言 `!(handle->flags & UV_HANDLE_CLOSING)`）退出，而 `status.json`
仍是 `completed`、`exit_code` 为空。结果与正文可用，但退出是崩溃；诊断事件里有
「进程已退出（exit code: 0xc0000409）」（info 级）。TUI 现在把长诊断折叠成首行 + 「另有 N 行」，
不再让整段 Node 栈占满正文。

## 派工中发现并已修的产品问题

- `4cf84b0` 旧配置字段（`sandbox` → `permission`）的报错不说新名字。
- `b227176` Windows 上 headless 弹常驻 / 闪烁的终端窗口；`$(arcdeck run --detach)` 挂到任务结束。

## 派工中发现、尚未修的

- **磁盘写满被归为 `MODEL_FAILURE`**（w2-remote-transport，2026-10-14）：运行时因 `ENOSPC` 以 1 退出，`result.json` 的 blocker
  是 `MODEL_FAILURE: ENOSPC…`。根因是环境（磁盘），不是模型；调度者按 `retryable` 决策时会被误导。应在失败分类里认出
  `ENOSPC` / `no space left` 并给独立的码（如 `HOST_DISK_FULL`，不可重试直到腾出空间）。监工本身随之死亡，`status` 由读者
  判 `dead` —— 这一半是设计如期生效（进程死了不能自称健康）。
