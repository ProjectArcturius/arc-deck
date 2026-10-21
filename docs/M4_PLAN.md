# M4 · TUI 完整落地的派工计划

日期：2026-10-14 起。用户裁定：TUI 做到 PRODUCT_PLAN §7 **完全完成**，用 `saiku` headless 派工人干
（`opencode-go/deepseek-v4.1-flash` · `effort high` · `--permission full --worktree dsh/<id>` · 每件都带 `--accept`）。
Claude 是派工者：写任务书、复跑验收、逐行审阅、在工人分支上提交后挑进 main。工人不提交（`allow_commit: false`）。

任务书在 `tasks/m4/`，共用条款在 `tasks/m4/_common.md`。**本文件只记计划与状态；证据在各提交正文里。**

## 当前状态：队列做完（2026-10-20 第六轮）· 密度与缺口（第七轮）· 焦点与颜色（第八轮）

> 派工队列自 2026-10-14 暂停后没有再开；剩余四项（w3-session-list 收尾、w3-tool-views、w4-tui-modes、w5-polish）
> 由调度者直接做完，证据见下文「第六轮」。**只剩需要人判断的那一条**：§7.5 与 OpenCode v1.19.0 的
> 同终端视觉对照。`tasks/m4/` 下未派出去的任务书保留作历史，不再有「待 …」的依赖关系。

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
| 2 | w3-tui-input | `@` 文件、历史、粘贴摘要、Shift+Enter、外部编辑器、metadata | ✅ 工作区（未提交）：`files.rs` + 编辑器原子片段；177 个 TUI 测试。stash 第七轮补上；Shift+Enter 只在代码里启用了 kitty 协议，**真终端未实测**（见下） |
| 2 | w3-session-core | TUI 改走 Remote：多轮、流式、思考、取消；`/runs` 保留看工人 | ✅ 358425a（经 `core::service` 消息与 `remote::service` 说话 —— GUI 将来用同一份） |
| 3 | w3-session-list | 会话列表 / 恢复 / 改名 / 时间线；真实模型目录 | ✅ 358425a + 第六轮：**时间线**（`/timeline`，选中滚到那条消息）与**内容检索**（`/search`，`session/search` + 内核 FTS 行）已补；**删除未做**（Remote 没有删除方法，`session/*` 里只有 create/rename/fork/cancel/page/follow）；子 agent 会话第七轮经 `/subagents` 打开（仍不在顶层列表里） |
| 3 | w3-tool-views | diff、shell 输出、折叠、完整输出查看器 | ✅ 第六轮（`toolview.rs` + `/output` `/diff`）：按类别的摘要、着色 unified diff、失败/被拒折叠态可见、全屏查看器（滚动 + 过滤）；**子 agent 会话不能从工具块点进去**（见下） |
| 3 | w4-tui-modes | Agent / Plan / Build / 授权级别 / confirm 审批界面 | ✅ 第六轮：§7.4 六种转换、运行中先确认、Plan 落到执行层只读、`confirm` 接通审批通道（`$events` 流 + 审批弹层） |
| 4 | w5-polish | 信息栏真实数据、首次运行与故障提示、终端恢复、toast、复制、长会话性能 | ✅ 第六轮：信息栏（标题/目录/git 分支与脏文件/模型/Agent·模式/授权/用量/上下文占用/改动文件/todo/待答审批）、Home 提示与页脚、故障提示带可复制命令、panic 恢复终端、toast、OSC 52 复制、正文渲染缓存 |
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

> 上面这一行是第五轮的记录，**第六轮已把其中前三条补上**，**第七轮补上 persona 注入与子 agent 会话打开**（`/subagents`）；
> 仍未做的：`session/control` 实时用量（用 provider 实报的 usage 与目录里的 contextWindow 算占用，没有订阅 control 流）。

## 第六轮（2026-10-20）：把顺序 3 剩余与 w5-polish 收完

**一句话**：TUI 的四项剩余全部落地 —— 工具视图（diff / 查看器）、模式与 confirm 审批、时间线与内容检索、
收尾（信息栏真实数据 / 故障提示 / 终端恢复 / toast / 复制 / 渲染缓存）。**判据 7 的「与 OpenCode 同终端视觉对照」
仍要人看**，不在这一轮的自动证据里。

### 证据

| 检查 | 结果 |
|---|---|
| 离线套件 | core 84 + 契约 7 · dsh 108（8 个 live 默认 ignore）· TUI **216** · cli 49 + 43 端到端（`--features test-runtime`）全过 |
| `cargo clippy -p arc-deck-tui --all-targets -- -D warnings` | ✅ |
| 审批通道（我们这侧） | ✅ 真实 WS + 真实 HTTP 假内核：`$events` 开流 → `ready` → `approval/request` → 回答写到 `$events/result`（`clientId`/`eventId`/`allowed-once` 逐字断言） |
| 审批通道（真内核） | ✅ `remote_live_forwarded_events_stream_is_ready`：0.2.1-alpha.2 上 `$events` 开得起来、首帧 `ready` 带 `clientId`（零配额） |
| 确认档的补丁 | ✅ 把 `approval: ask` + 预设 `confirm` 写进 patch 后真起一次 web 内核，**stderr 无 `did not activate` / `no preset`**；`--dump-config` 里 `session-query-sqlite` 的 `openAt: first-search` 与 `approval`/`permission` 行都在 |
| 真终端 | ✅ `console-probe.ps1`（release）：Home 品牌 / 内核就绪 / `@` 补全 / F1 / 输入 / `/sessions` / `/runs` / 退出 / 无残留内核 |
| 长会话 | ✅ 500 条消息的会话连续绘制 6 帧，正文只重排 **1** 次（`render_builds` 计数）；滚动不重排 |
| 界面行为 | ✅ 模式六种转换、运行中确认（n 不改 / y 才重启）、审批可见与 Esc 不是允许、队列与撤回、时间线跳转、`/diff` `/output` 查看器、toast 生命周期、OSC 52 base64 |

### 这一轮踩到并修掉的

- **`$events` 被自己的方法名校验挡住**：`valid_method` 原来只认「恰好两段」，而 Gateway 的内部端点只有一段。
  症状不是「端点不存在」，而是客户端在 `open` 之前就报「不是一个合法的 Remote 方法名」——
  与真实原因无关。现在只对 `$` 开头的单段端点放行（其余规则不变）。
- **黑白的界面其实是启动环境里的 `NO_COLOR=1`（2026-10-20 排查）**：crossterm 照
  no-color.org 的约定把颜色全关了，而我从 agent 环境里拉起 TUI，把它带进了窗口 ——
  产品本身是有配色的（默认主题 `opencode`：橙 accent / 紫 keyword / 绿 string …）。
  现在启动时若检测到 `NO_COLOR` 会在通知行直说，下次不会再把它当成主题坏了。
- **TUI 视觉照 OpenCode 抄（2026-10-20 用户裁定）**：代码块按主题 `syntax.*` 着色
  （关键字 / 字符串 / 注释 / 数字 / 类型 / 函数 / 运算符，块注释与 Python 三引号跨行），
  用户消息改成 OpenCode v1 那种「左边竖线 + 抬高背景」的块，去掉自己发明的「你」/
  「Saiku」角色标签，工具行的名字用 accent。视觉语言只在 GUI 上做，TUI 不搞原创。- **发送键改回 Enter（2026-10-20 用户反馈）**：M4a 为了躲开 Windows 粘贴拆键，把发送键改成了 F6，
  代价是「没人找得到发送键」。现在 Enter 发送、Shift+Enter / Ctrl+J 换行；Windows 的粘贴靠
  「同一次 poll 里的整批按键」识别（`App::input_batch`），批里的 Enter 当换行。F6 只留兼容别名。
- **正文缓存的键必须含「内容版本」**：运行记录的更新（`Update::Detail`/`Runs`）原先不加版本号，
  缓存会把新内容当成旧的。补上之后，「同一内容第二次绘制不重排」才是可断言的事实。

### 仍未做（不假装）

| 项 | 为什么 |
|---|---|
| 子 agent 会话**从工具块**进入 | 第七轮用 `/subagents` 打开（父地址由服务按列表配，协议不变）；工具块本身不可选中 —— 纯键盘界面里「选中某一行工具」是另一套焦点模型，没做 |
| 鼠标点选消息 / 按条复制 | **不做（用户 2026-10-20 裁定）**：用户是纯键盘习惯，鼠标点选「不仅复杂并且很讨厌」。鼠标只保留滚轮；复制用 `/copy`（OSC 52，复制最后一条回复）。以后不要把它当缺口补回来 |
| 最终视觉对照 | §7.5 要同终端、同尺寸与 OpenCode v1.19.0 对照，满意与否由用户判断 —— 机器不能替 |

## 第七轮（2026-10-20）：密度照 OpenCode 收、补缺口、封装验收

**一句话**：用户说「TUI 里字太多，density 不舒服，OpenCode 不是这样的」。对着 `/REFS/opencode`
（`routes/session/index.tsx`、`message-parts.tsx`、`component/prompt/*`）逐件改形状，再补上第六轮记下的缺口。

### 密度：改了什么（每一条都是 OpenCode 的对应件）

| 位置 | 之前 | 现在 | OpenCode 对应 |
|---|---|---|---|
| 页脚 | 两行：常驻通知 + 八个键位的说明书 | **一行**：左边「现在怎么了」（通知 / `⋯ Esc 中断` / 目录:分支），右边 `shift+tab agents  ctrl+p 命令` | prompt footer |
| 通知 | 永不消失；启动时就有一句「内核启动中；可以先输入；/help…」 | 没事不说；普通通知 8 秒收起，**故障与 `错误：` 不收** | toast / footer status |
| 会话页标题栏 | ` Saiku  /  标题` 一整行 | 去掉；标题在信息栏顶上与终端窗口标题里 | 无标题栏 |
| 工具行 | 两行一个，行尾挂「· 完成」「（/tools 展开 diff，/diff 看全部）」 | 一行一个、次要色、按类别的图标（`→ ✱ ← $ ◉`）；失败才变红；连续 ≥2 个只读查看折成「→ 浏览了 N 个文件、M 次查找」 | `InlineTool` / exploration 分组 |
| 思考 | `思考 · 1234 字（/tools 展开）` | `+ 思考：<标题>`（`**标题**` 开头时取标题） | `ReasoningPart` |
| 一轮结束 | 无 / 「模型失败：…」 | `▣ Echo · 模型 · 已取消`；失败另起一行 `错误：…` | `AssistantFooter` |
| 用户消息 | 竖线 + 底色，无内边距 | 上下各一行内边距 | `UserMessage` paddingTop/Bottom |
| 正文 | 顶格 | 缩进 3 列；段之间空一行，连续工具之间不空 | `paddingLeft={3}` + row `marginTop` |
| 输入框元数据 | `模型 · echo·build · workspace` | `Echo · 模型 provider · effort`；授权级别**只在偏离缺省时**出现 | `PromptMetadataRow` |
| 信息栏 | 工作区 / 会话 / 目录 / 分支 / 模型 / Agent / 授权 / 状态 / N 轮 …… 两行一对 | 粗体标题 → 上下文 → 改动的文件 → Todo → 待答审批；目录·分支·版本沉到底部；输入框已有的不再写 | `Sidebar` + sidebar footer |
| 弹层 | `┌标题──┐` 方框，底下一行「↑↓ 选择 · Enter 执行 · Esc 返回」 | 无边框面板、粗体标题 + 右侧 `esc`、背后页面压暗；说明行去掉（只在选中行不可用时说原因） | `Dialog` |
| 补全弹层 | 带标题的方框 | 输入框正上方一块列表 | autocomplete |
| Home | 品牌 / 副标题 / 输入框 / 一行五条提示 / 目录·版本·内核状态 | 品牌 / 输入框 / 一条提示；目录与版本在页脚 | home |

### 补上的缺口

| 项 | 做法 | 证据 |
|---|---|---|
| 子 agent 会话 | `/subagents`：列当前会话的子会话；子会话里第一行回父会话，且**只读**（发消息被挡、草稿保留）。服务按上一次列表给子会话配父地址 `{kind:'subagent',…,mode}`（`mode` 从 `projections.values.subagent` 读，P13），**协议不变** | `subagent_sessions_get_the_parent_address`、`subagent_sessions_open_from_the_parent_and_are_read_only`；真终端 `/subagents` 弹层起来（本机最近的会话没有子会话，显示「还没有派出子 agent」） |
| 交互面 persona | `serve::interactive_persona`：个性化关 → 空串（一段都不注入）；开 → 与 headless 一般模式同一份 `compose_system_prompt`（工程规则 → Agent → 模式 → 风格） | `the_interactive_persona_follows_the_personalization_switch` |
| stash | `<leader>s` / 命令面板：收起草稿；`/stash`：列出、Enter 取回；跟偏好一起落盘 | `a_draft_can_be_stashed_and_brought_back`；真终端三条全过 |
| `session/search` 真查询 | — | ✅ `saiku serve --stdio` 对真内核检索 `OK`，回了本机会话的真实命中；真终端 `/search OK` 弹出「N 条命中」（零配额） |

### 顺手抓到的真 bug

- **`/search <词>`、`/output <序号>` 敲出来一律被拒成「不接受参数」**：`execute` 里收参数的名单是手写的
  （`Effort | Permission | Rename`），后加的两条没进名单；单测直接调 `execute` 所以没抓到。
  现在看注册表的 `arg`。是 console-probe 对着真内核跑 `/search` 时发现的。
- **模型 / provider 对话框里敲的字从来没画出来**（光标在动、字看不见）。对着预览发现。
- **孤儿内核**：TUI 被硬杀（关终端窗口不跑 Drop）后 node 内核还活着；下一个 saiku 照规矩接管遗留锁，
  于是同一份 home 上**两个内核** —— P14 的形状。实测抓到一个 10:58 起、主人已死的内核。现在锁旁边另记
  内核 pid（`saiku.kernel`，确认收掉才删），下一次拿锁时**核对命令行**（`--profile web` + 本 home 的 patch）
  再收树；对不上只删记录（pid 会复用）。Windows 的 `kill_tree_by_pid` 也改成真确认根进程退出，不再一律「未确认」。
  那个实测的孤儿就是被新二进制收掉的。
- **段落后紧跟列表时少一行空行**（`Start(Item)` 把段落要的空行清掉了）。
- **假一元接口在机器忙时答非所问**（全量套件里偶发一次）：Windows 上 accept 出来的连接继承非阻塞，
  请求没到就读空、拿 `rpcId:"X"` 回话。切回阻塞并按 Content-Length 读满。修后全量连跑三次全绿。

### 证据

| 检查 | 结果 |
|---|---|
| 离线套件 | core 84 + 契约 7 · dsh 110（8 个 live 默认 ignore）· TUI **222** · cli 50 + 43 端到端（`--features test-runtime`）全过，连跑三次 |
| `cargo clippy -p arc-deck-tui --all-targets` | TUI crate 无告警。**core / cli / dsh 有既有告警**（clippy 1.95 新加的 `collapsible_match` 等，不是这一轮引入的；`-D warnings` 全仓不再成立） |
| 真终端（release，`console-probe.ps1 -Resume`，零配额） | 内核就绪 · 页脚一行 · 占位符写着 Enter 发送 · `@` 补全 · stash 三步 · F1 · 输入 · `/sessions` · `/runs` · **`/search` 真内核** · `/subagents` · 恢复会话 · 退出 · 无残留内核。`resume_history` 两条判据假定最近一次是 `-Cancel` 留下的会话，本机最近的是用户自己的会话，所以那两条是 false（打开与历史本身在快照里是好的） |
| 安装 | `~/.cargo/bin/saiku.exe` 换成本轮 release（与 `target-main/release` 逐字节相同）；上一版留在 `saiku.prev-2026-10-20-1015.exe` |

预览：`cargo test -p arc-deck-tui -- generate_reviewable an_agentic` 把各屏写成文本到 `target/tui-preview/`
（新增 `conversation-*`、`models-*`、`agentic-*`）。**颜色看不出来** —— 满意与否仍要在真终端里看。

## 第八轮（2026-10-20）：输入框焦点、扫描条、一次 Enter、Agent 颜色

用户看过第七轮后提的四条，逐条照 OpenCode 源码改：

| 反馈 | 改法 | OpenCode 对应 |
|---|---|---|
| 输入框没有灰色底，没有视觉焦点 | 底色改成 `promptBg = decrease(background.raised)`（默认深色主题 `#1e1e1e` 压在 `#0a0a0a` 上）；左边 Agent 颜色的 `┃`，下沿一排 `▀`。之前用的 `background.formfield` 在 OpenCode 主题里是 `transparent` | `component/prompt/index.tsx` 的 `promptBg` |
| 底部要有「在工作」的滚动色条 | 页脚左边 knight-rider 扫描条（宽 8、两头停 30/9 帧、6 级拖尾、40 ms 一帧）+ `esc 中断`，颜色是当前 Agent 的颜色；**只在干活时**按帧重画，空闲时一帧不多画 | `ui/spinner.ts` 的 `createFrames` / `createColors` |
| `/per` 回车要按两次 | 补全里只有**必需**参数的命令（`<…>`：`/rename`、`/search`）才填进去等参数；可选参数（`[…]`：`/permission`、`/effort`、`/output`）一次 Enter 直接执行（不带参数时打开选择器）。Tab 只补全并补一个空格 | autocomplete 的 select |
| 不同 Agent 不同颜色，Echo 青色 | Echo 青、Build 橙、Plan 紫，取色阶 200 那一步；每个 Agent 一条偏好链（35 个内置主题的色阶并不齐，`aura` 没有 cyan），保证**每个主题、两种模式下三者都不撞色**（有测试）。用在输入框竖线、Agent 名、用户消息竖线、`▣` 页脚、扫描条 | `local.agent.color` |

### 可用性检查里顺手修的

- **Windows 上一次按键被当成粘贴**：`input_batch` 按「一批里几条事件」判粘贴，而 Windows 每个键是 Press + Release
  两条，按得快就同一批到 —— 于是 Enter 变换行、补全弹层被绕过。改成数**按下**的键。真终端探针实测抓到的，单测走
  `App::input` 绕过了这条路。
- **Shift+Tab 只在 echo / build 之间翻**，不经过 Agent 对话框那条路（没有运行中确认、到不了 Plan），提示还写着工人运行的
  「下一次独立请求」。现在 Echo → Build → Plan → Echo，走 `select_agent`。
- **个性化开着时换 Agent 不重启内核** —— persona 是按 Agent 装进内核的，不重启就还是旧提示词。`Boot` 带上
  `personalization`，开着时换 Agent 也重启。
- 一轮页脚的模型名用目录里的展示名（与输入框一致），不再是内核原始 id。

### 证据

| 检查 | 结果 |
|---|---|
| 离线套件 | TUI **227**、dsh 110、cli 50 + 43 端到端、core 84 + 7 全过；`clippy -p arc-deck-tui` 无告警 |
| 真终端（release，`console-probe.ps1 -Resume`，零配额） | 20 项全过，含 `one_enter_runs_completion`（真 Enter）、`shift_tab_switches_agent`、`agent_back_to_echo`、`search_real_kernel`、`subagents_dialog`、`resume_history` |
| 真终端（`-Submit -Cancel`，三次极小调用） | 同一会话两轮 42 → 43 ✅、无错误 ✅、取消后输入区可用 ✅；跑着时页脚快照是 `⬝⬝⬝⬝⬝■■■ … 中断`（扫描条在动）。`scanner_while_running` / `cancel_shown` 两项那一次判 false，是判据的问题（见下），判据已改 |

**已知限制（不修，如实记下）**：中文代码页的**旧控制台 conhost** 把东亚歧义宽度字符（`■`、`·`、`…`）画成两格，而 TUI
按一格排，同一行后面的字会错位（探针读回来 `esc 中断` → `■中断`、`· 已取消` → `· 取消`）。**Windows Terminal 按一格画，
没有这个问题**；OpenCode 用的是同一批字符。

## TUI 真终端证据（2026-10-15）

`crates/arc-deck-tui/tools/console-probe.ps1` 起一个**隐藏控制台**、把按键写进它的输入缓冲区、
再读回屏幕缓冲区 —— 把「在真终端里可用」做成几条可复现的断言。实测（release 二进制）：

| 检查 | 结果 |
|---|---|
| Home 渲染 | ✅ `Saiku` |
| `@` 文件补全 | ✅ `@ 文件引用` 列真实仓文件 |
| `F1` 帮助 | ✅ |
| 输入可见 | ✅ |
| `/sessions` 面板 | ✅ 打开「运行历史」 |
| **真提交一轮**（`-Submit`，发 `say OK`） | ✅ `运行已结束`，正文 `OK`，无错误提示 |
| **取消**（启动中 `Esc`） | ✅ 界面显示取消，`status.json` → `cancelled`；退出后无残留 `saiku` 进程 |
| `Ctrl+C` 退出（含收尾） | ✅ 进程退出 |

`-Submit` 会消耗一次小配额，默认关闭；不加它只跑界面检查。

**同轮发现（尚未修，属运行时而不是 TUI）**：DSH `0.2.1-alpha.2` 在**写出结果之后**以
`0xc0000409`（libuv 断言 `!(handle->flags & UV_HANDLE_CLOSING)`）退出，而 `status.json`
仍是 `completed`、`exit_code` 为空。结果与正文可用，但退出是崩溃；诊断事件里有
「进程已退出（exit code: 0xc0000409）」（info 级）。TUI 现在把长诊断折叠成首行 + 「另有 N 行」，
不再让整段 Node 栈占满正文。

## 派工中发现并已修的产品问题

- `4cf84b0` 旧配置字段（`sandbox` → `permission`）的报错不说新名字。
- `b227176` Windows 上 headless 弹常驻 / 闪烁的终端窗口；`$(saiku run --detach)` 挂到任务结束。

## 派工中发现、尚未修的

> 2026-10-20 1.0 收尾：下述 ENOSPC 已改为 `HOST_DISK_FULL`；
> Windows SDK shutdown 的 `0xc0000409` 采用钉版本的进程树收尾规避，并增加异常退出码留档。
> Esc 后续轮、重连请求去重与光标绘制修复及证据见 [1.0 记录](releases/v1.0.0.md)。下文保留历史发现。

- **磁盘写满被归为 `MODEL_FAILURE`**（w2-remote-transport，2026-10-14）：运行时因 `ENOSPC` 以 1 退出，`result.json` 的 blocker
  是 `MODEL_FAILURE: ENOSPC…`。根因是环境（磁盘），不是模型；调度者按 `retryable` 决策时会被误导。应在失败分类里认出
  `ENOSPC` / `no space left` 并给独立的码（如 `HOST_DISK_FULL`，不可重试直到腾出空间）。监工本身随之死亡，`status` 由读者
  判 `dead` —— 这一半是设计如期生效（进程死了不能自称健康）。
