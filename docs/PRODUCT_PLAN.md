# Arc Deck：产品需求与编码方向

版本：0.2 · 日期：2026-10-14 · 状态：方向已定型（§0）；M1 已实测；**M2a/M2b 的主体与 M2c 的 Windows 部分已落地，并在 Windows 上用真实模型跑通**（§10.2b）；**Linux 一侧的目标改为 Arch（本机），Debian 不再作为目标** —— 本机已实测：构建、离线套件、夹具端到端，以及**真实 DSH 运行**（派工 / 探活 / 取消 / 杀监工 / 孤儿回收 / worktree 真提交）。§10.2b 之外的内容都不是功能完成声明。

本文件是后续开发的产品方向入口。旧文档中「两条并列 GUI」与「新桌面宿主直接引用 C# 协议库」的路线由本文件取代；旧代码、历史实测和适用该代码的约束保留。未创建的目录、命令、schema 和测试在本文件中均为待实现规格。

## 0. 2026-10-14 定型结论

**本轮推进补记**：用户要求审查核心/headless 完备性后开始 TUI。审计及 M4a 第一段见
[CORE_HEADLESS_AUDIT_2026-10-14.md](CORE_HEADLESS_AUDIT_2026-10-14.md)。无参数入口已替换为
独立运行 TUI；这不代表 M2c/M2d/M3 或完整 M4 已通过。Remote 会话和执行 guard 仍待接通。

本节是 0.2 版的变化摘要，与后文冲突时以本节为准；后文已按本节同步。

| 编号 | 决定 | 依据 |
|---|---|---|
| D1 | **当前焦点只有 Rust 后端，且其中只有 headless 可用这一件事。** 目的是尽快接进 Ether 的工人流程；TUI 排在 headless 之后，两个桌面 GUI 不在焦点内 | 用户 2026-10-14 |
| D2 | **一个可执行文件 `arcdeck`**：在工作目录直接运行 `arcdeck`（无参数）进入 TUI；带子命令进入 headless（§6.0）。TUI 落地前，无参数运行给出明确提示并退出，不假装有界面。**headless 本身分两条**：不给任务书、不带工人参数时是**一般模式**（与 `opencode run "…"` 同类，stdout 是给人看的回答）；带了工人那套参数才是**工人模式**。Arc Deck 不是一个工地，只是一个带工人优化方案的 agent | 用户 2026-10-14 |
| D3 | **headless 的主用法是 Ether 的「工人任务书」**：调度者派一件任务、之后只靠 shell 上的一行状态判断「做完 / 在跑 / 卡死 / 出错」，不读长日志（§6.5–6.6） | 用户 2026-10-14；Ether `docs/DISPATCHER.md`、`tools/dispatch/dsh-runner.sh` |
| D4 | **底层只用 DSH 一个内核**，非 DS 模型走 DSH 的 pi-ai 路由。调查结论：DSH 的优势一半 DS 专属（协议与缓存）、一半 provider 中立（loop 工程），后者对非 DS 适用；按用户规则「若有则全部 DSH」。OpenCode / Claude Code 只作 TUI、Agent、工具定义的设计参考 | [DSH_FINDINGS.md](DSH_FINDINGS.md) |
| D5 | **DSH 的 DS 优势只在官方路由上成立**：worker 默认 `deepseek-official`；第三方网关上 DSH 同样出过 DSML 泄漏 | DSH_FINDINGS §1.4 |
| D6 | **桌面版分两块且都冻结**：Windows = C# + WinUI 2，Linux = Rust + Kanesumi。都等后端的本地服务接口（M5）稳定后再接，不并行开发 | 用户 2026-10-14 |
| D7 | **headless 的控制面是任务状态目录里的文件**（状态、事件、结果、取消），不是常驻服务。ssh 过去 `arcdeck status` 就能读，Windows 与 Arch 一样 | 本文 §6.5 |
| D8 | 「非 DS 用 DSH 不劣于 OpenCode」与「完整工具集 vs RL 两工具组合」都**未测**，列为 M2d 对照门；没有数据前不建第二个内核 | DSH_FINDINGS §5 |

## 1. 文档状态与需求基线

### 1.1 用户已明确的要求

| 编号 | 要求 | 判定方式 |
|---|---|---|
| R1 | 产品分为 Rust 后端（TUI + headless）与桌面 GUI 两部分 | 后端可独立使用，不依赖窗口或任何 GUI |
| R2 | 后端用 Rust，目标跨平台 | Windows、Linux（Arch）、macOS 分别运行验收；只在 Windows 编译不算跨平台完成。首批是 **Windows + Arch（本机）** —— 2026-10-14 修订：原定 Debian，但那台机器上的自研项目暂时跑不起来，改以本机 Arch 为准（参 §10.2b） |
| R3 | 桌面 GUI：Windows 用 C# + WinUI 2，Linux 用 Rust + Kanesumi；当前都不在焦点 | 保留 WinUI 2 平台控件路线，不擅自改成 WinUI 3 或 WebView；Kanesumi 守 K1–K9 |
| R4 | headless 给 workers 用，外部主控可通过脚本派发任务书并收回结果 | 无 TUI 的完整任务提交、执行、终止和机器可读回报 |
| R4a | 在工作目录输入 `arcdeck` 回车进入 TUI；加参数进入 headless | 同一个可执行文件；无参数与有参数两条路径各有测试 |
| R4b | headless 的探活要便宜：调度者用 shell 就能知道做完、在跑、卡死还是出错，不必读一大堆输出 | `arcdeck status <id>` 一行（约 120 字节）+ 退出码即可判定；判定由读者按时间戳计算，进程死了不能自称健康（§6.5） |
| R4c | 尽快用于 Ether 开发 | 能直接吃 Ether 的 Markdown 任务书与工人守则，取代 `dsh.sh`/`dsh-runner.sh` 的 worktree、看门狗与续跑（§6.6） |
| R5 | 引入 OpenCode 的 Build、Plan；Echo 成为默认 Agent | 新会话、未指定 Agent 的 worker 默认使用 Echo；Build/Plan 可显式选择 |
| R6 | 工具与 Agent 具有 Claude Code 那样明确、一致的定义 | 有角色、工具 schema、执行边界、权限反馈、验证和交付约定，不仅有一段语言风格提示词 |
| R7 | TUI 视觉效果迁移 OpenCode | 有对应组件、主题和终端运行对照；不是只借用几个颜色 |
| R8 | 骨架与底层基于 DSH，保持 DeepSeek 调用体验与成本优势 | 不另造 provider、agent loop 或模型计费系统；使用量与费用仅按真实数据呈现 |
| R8a | 先查清 DSH 的优势是否对非 DS 模型成立：成立则底层全部 DSH，否则以 OpenCode 为主、Claude Code 为辅，DS 专用 DSH | 已查（[DSH_FINDINGS.md](DSH_FINDINGS.md)）：部分成立 → 全部 DSH（D4）；对照门 M2d |
| R9 | `REFS/` 只作本地参考，不进入 Git | 根目录 `/REFS/` 忽略；干净克隆与发布不依赖其存在 |

### 1.2 本文选定的技术方案

- 共用产品逻辑与 DSH 适配逐步收敛到 Rust；GUI 通过 Arc Deck 的本地 IPC 消费它，不再另写一套 DSH 客户端。
- DSH 是唯一执行内核；DS 模型走 `deepseek-official`（Messages 协议），非 DS 模型走 DSH 的 pi-ai 路由。Arc Deck 不自建 provider 或 agent loop。
- worker 首版一任务一 runtime，使用 DSH SDK stdio；交互会话使用 Remote API。两者共用事件投影与 Agent 配置，不伪装成能力完全相同的传输。
- headless 由一个**监工进程**（supervisor）托管一个 DSH runtime：负责状态文件、心跳、卡死判定、有界续跑与进程树收尾；调度者只读状态文件，不连进程。
- Rust TUI 首选 `ratatui + crossterm`，复写 OpenCode 的视觉与交互；不将 TSX 代码称作可以直接链接进 Rust 的组件。
- Echo 具有完整默认角色与工作纪律；风格仍作为可组合的一层，不能用风格开关替代权限策略。
- Windows 完整信任宿主先做 WinUI 2 / XAML Islands 可行性验证，再决定页面迁移方式。

这些是当前编码方向，不是声称参考代码已经实现了 Arc Deck 的接口。涉及审美、宿主迁移和数据处理的未决项见 §11。

## 2. 产品定位与范围

Arc Deck 是基于 DSH 的编码 Agent 产品：人可以在 TUI 或 Windows GUI 交互，Claude 等外部主控可以把它作为执行 worker。外部主控拥有任务分配与结果接受权；Arc Deck 不冒充 Claude，也不承诺 DeepSeek 的模型能力与 Claude 相同。

「定义好的感觉」落实为五个可观察结果：默认行为不靠每次临时提示；工具参数和错误稳定；权限与模式清楚；执行进度可追踪；完成说明有证据并承认未验证项。

### 2.1 核心使用流程

1. 主控：提交任务书 → 获取任务 ID → 消费进度 → 收到结果与证据 → 决定接受、补充任务或人工检查。
2. TUI 用户：打开项目 → 默认 Echo → 浏览或实施 → 查看工具与权限状态 → 验证 → 查看结果；Plan 不自行转入 Build。
3. GUI 用户：使用现有 Windows 控件浏览和操作同一产品数据；GUI 不拥有另一套权限或完成判定。

### 2.2 范围与优先级

- P0（当前焦点）：**headless 在 Ether 上可用** —— 真实 DSH 单任务、分离运行、便宜的状态探活、卡死判定与有界续跑、紧凑回报、Ether 任务书与 worktree、Windows + Arch 两台机器。**（离线骨架已落地，真实执行在 M2a）**
- P1：Echo/Build/Plan 的权限实装（Plan 只读在执行层生效）、M2d 对照实验、OpenCode 风格 TUI 主流程。
- P2：本地服务与 IPC、两个桌面 GUI 接线、有限并发、专用研究/审查 Agent、Skills 扩展、Hooks、macOS、分发完善。
- 首版不做：云端调度、集群、自动工作树**合并**（创建 worktree 可以，合并归调度者）、Claude Code API/插件的兼容承诺、完整 OpenCode 插件生态、自动推送、旧会话批量迁移、第二个执行内核。

## 3. 参考项目与复用边界

### 3.1 固定的参考快照

| 参考 | 已静态核对的版本/提交 | 用途与边界 |
|---|---|---|
| [OpenCode](https://github.com/anomalyco/opencode/tree/b9ca4d21b4f241154081143a6f5bef648f5ccfd0) | `2.0.27` · `b9ca4d21b4f241154081143a6f5bef648f5ccfd0` | MIT；迁移 TUI 视觉/交互、适配 Build/Plan 行为。复制代码、主题或提示词的实质部分须保留版权与许可声明，并审计实际引入依赖 |
| OpenCode **v1 TUI 基线** | `v1.19.0` · `733b2c627e5aec39cb33cc22373bdce33053ef2c` | TUI 的**迁移基线**（同一个仓的 tag，不是另一个仓）。MIT 同上；见 §7.1 对「v1」三种含义的区分 |
| [DSH](https://github.com/deepseek-ai/deepseek-harness/tree/d743267388641bc76f17c45ce8b4c231aed1d32c) | `0.2.1-alpha.2` · `d743267388641bc76f17c45ce8b4c231aed1d32c` | 根许可证 MIT；执行内核与协议适配候选。运行时第三方组件另行审计；通过验收后才成为正式支持版本 |
| [Claude Code 公开仓](https://github.com/anthropics/claude-code/tree/3deb873687379f0c199a69fdaccc65f0865f7a29) | `3deb873687379f0c199a69fdaccc65f0865f7a29` | 参考角色、工具范围、开发流程和扩展契约。根许可证为 All rights reserved / Commercial Terms，不按 MIT 处理，不默认复制实现或提示词 |
| 用户 Echo | 本机 `.config/opencode/agents/echo.md`，正文 changelog `v1.1 (2026-10-03)` | 用户提供的语言风格与工程纪律；后续整理为仓内定义，运行时不得依赖这台机器的绝对路径 |

Claude Code 这份检出包含公开插件、示例以及部分内置 mod 源码，不是已确认完整的 CLI 核心源码；`mods/README.md` 明确列出四个内置 mod。不得从工具名称或插件声明杜撰 Claude 默认 Agent、核心工具 schema 或内部执行行为。

### 3.2 源码依据

以下是本地静态核对入口，不是运行通过记录；`REFS/` 被忽略，因此同时保留上游仓名与提交号供重新取证。

- OpenCode：`packages/tui/package.json` 为 TSX + Solid + OpenTUI；`packages/core/src/plugin/agent.ts` 定义 Build；`packages/core/src/plugin/plan.ts` 定义 Plan 编辑限制与模式提醒。
- OpenCode TUI：`packages/tui/src/theme/index.ts` 使用 v2 默认主题；`routes/home.tsx`、`component/session-frame.tsx`、`ui/layout.ts`、`config/keybind.ts` 是布局与快捷键依据。
- Claude Code：`LICENSE.md`、`mods/README.md`；`plugins/feature-dev/agents/code-explorer.md` 有角色、工具集与输出要求；`plugins/feature-dev/commands/feature-dev.md` 有探索、澄清、设计、实施、审查的阶段约定。
- Claude Code：`mods/types/claude-code.d.ts` 的 `AgentSpec` 声明工具 allowlist/denylist，但标为会变化的扩展接口，不作为 Arc Deck 的兼容 schema。
- DSH：`packages/sdk/protocol/README.md`、`packages/sdk/server/src/server.ts`、`packages/core/session/src/types.ts`、`packages/api/session-controller/src/types.ts` 为当前协议依据。
- DSH：`packages/plan/plan-mode/src/index.ts` 明确 Plan 与权限独立；`packages/core/tools/src/index.ts` 提供 scoped restriction 与执行 guard；限制实现必须在实际工具执行点验证。

参考源码只作设计依据。Arc Deck 的公开名称、参数、错误、权限和版本规则由自己的契约确定；发布包不得读取 `REFS/`，也不得夹带未核清授权的参考文件。

## 4. 目标架构与现有资产

```text
                         arcdeck（一个可执行文件）
             ┌───────────────┼──────────────────────┐
         无参数 → TUI    子命令 → headless       serve → 本地服务（M5）
             │               │                       │
             │        supervisor（监工）──写──▶ 任务状态目录 ◀──读── arcdeck status/wait/report
             │               │                  status.json / events.jsonl / result.json / cancel
             └───────┬───────┘
                Arc Deck core（任务、配置、Agent、授权、归一化事件与投影）
                     │
                DSH adapter（唯一内核适配）── SDK stdio（worker）/ Remote（交互）
                     │
                DSH runtime ── deepseek-official（Messages）│ pi-ai 路由（非 DS）

WinUI 2（Windows）、Kanesumi（Linux）── 版本化本地 IPC ──▶ serve（M5 之后，当前冻结）
```

**为什么控制面是文件而不是服务**：派工的一方与干活的一方常常不在同一台机器上（Windows 上派、Linux 上跑，或反过来），
派工脚本还会在命令结束时被一起杀掉（DISPATCHER §三，「只加 `&` 不加 `nohup`」实测任务消失）。
文件在两台机器上语义一致、跨 ssh 只需一条命令、监工进程死了文件还在 —— 而「进程已死但没写终态」
正是最需要被看见的那种状态（§6.5）。常驻服务留到交互端（TUI 多会话、GUI）真需要时再做。

### 4.1 计划目录与依赖方向

| 计划位置 | 职责 | 不负责 |
|---|---|---|
| `crates/arc-deck-core/` | 产品配置、任务与结果类型、状态文件 schema、Agent 解析、归一化事件/投影、策略要求 | 终端绘制、XAML、进程管理、模型 HTTP 调用 |
| `crates/arc-deck-dsh/` | 运行时定位/启动/关闭、版本验证、**per-task DSH_HOME 与 profile 生成**、SDK/Remote 协议、能力映射与 DSH 事件转换 | 自建模型循环、另一套文件/命令执行器 |
| `crates/arc-deck-run/`（M2b 新建） | 监工：任务状态目录、心跳与进度、卡死判定、有界续跑、进程树收尾（Windows Job Object / Unix 进程组）、worktree 准备、结果采集（git 提交与 diff 由此采，不信模型自报） | 参数解析、终端输出格式 |
| `crates/arc-deck-cli/` | `arcdeck` 二进制：参数解析与分派（无参数 → TUI，子命令 → headless）、一行状态的格式、退出码 | 单独维护 Agent、投影或监工逻辑 |
| `crates/arc-deck-tui/`（M4 新建） | OpenCode 风格终端渲染、编辑、命令与交互 | 直接理解 DSH 原始事件与私有字段 |

依赖方向：`core ← dsh ← run ← cli`，`tui` 依赖 `core` 与 `run`（TUI 里发起的任务与 headless 同一套状态）。
现在 `arc-deck-cli/src/worker.rs`（约 1070 行）里的生命周期逻辑在 M2b 拆进 `arc-deck-run` —— 拆的理由是
TUI 与将来的服务都要复用它，不是行数；M2a 不拆，先让真实 DSH 跑通。
| `agents/`、`prompts/` | 版本化角色、Build/Plan 行为、Echo 风格和交付规则 | 凭据、用户私有全局配置 |
| `schemas/`、`tests/fixtures/` | 公开契约与无凭据测试夹具 | 真实用户会话与 API key |
| `runtime/` | DSH profile/preset 与必要的薄插件 | fork 内核、复制上游执行循环 |
| `uwp/` | 现有 GUI 与宿主迁移验证 | 生产路径另起一套 DSH 协议栈 |

`core`、`dsh`、`cli` 与 `agents/`、`prompts/`、`schemas/`、`tests/fixtures/` 已在 M1 创建；`run`、`tui`、`runtime/` 尚未创建。core 不依赖 adapter；adapter、CLI 与 TUI 使用 core 类型。GUI 后续通过版本化 IPC 获取产品模型，不能把 SDK JSON-RPC 与 Remote 信封直接当 Arc Deck IPC。DSH 插件仅用于已有接口不能满足的 persona/策略接入，并保持独立测试。

### 4.2 现有代码的去向

- `kanesumi/`：**Linux 桌面版的路线**（D6），当前冻结 —— 不新增功能，保留源码、测试与测量记录；它的设计正典不支配新 TUI 或 WinUI。
- `uwp/`：**Windows 桌面版的路线**，当前冻结 —— 保留现有页面、平台行为与 UIA/像素验收；当前仍为示例数据，不称作已接入新 core。
- 根 `arc-deck-dsh/`：保留 C# 旧适配器和探针供行为核对；新 Rust 适配器验收完成后再决定归档方式，不先删旧代码。
- Cargo workspace 现有成员先保留；新增 crate 时才改构建结构，不用目录移动制造无关 diff。
- 历史约束按实现范围保留；例如 Kanesumi 的 Win32 FFI/IME 规则不是新 headless 必须依赖窗口层的理由。

## 5. Echo、Build、Plan 与工具契约

### 5.1 Agent 不是仅一段风格提示词

每个定义至少包含 `schema_version`、`id`、`description`、角色（主 Agent/子 Agent）、默认模式、prompt 组成、工具集合引用、权限 profile、模型选择策略和结果要求。定义加载、引用解析与权限生效分别测试；未知工具、重复 ID、缺失引用或不支持配置必须明确报错，不能静默退回 Build。

| Agent | 固定产品行为 | 模式/权限 |
|---|---|---|
| `echo` | 默认工程协作者；先理解与取证、执行获准任务、验证并如实交付，使用用户 Echo 风格 | 默认 `build`；允许显式选择 `plan`，模式限制不受风格影响 |
| `build` | 适配 OpenCode Build 的实施角色；工具按实际配置执行 | `build`，仍受项目/任务授权，不等于无限制权限 |
| `plan` | 适配 OpenCode Plan 的研究与规划角色；默认使用 Echo 风格 | `plan`，不实施、不自动切模式、不委派可写任务 |

组合矩阵：`echo + build`、`echo + plan`、`build + build`、`plan + plan` 有效；`build + plan` 和 `plan + build` 冲突并拒绝。未指定 Agent 选择 `echo`，未指定模式取定义默认值。Agent/模式/模型是不同维度。`echo + plan` 的实现和风格可以组合，但安全边界始终按 Plan 执行。交互命令如何切换组合见 §7.3，不能生成非法组合。

Build/Plan 借鉴的是角色与行为，不将 OpenCode 的 provider、工具循环和事件词汇移入 DSH。专用 Explore/Review Agent 属于 P2；首版不为每一步自动起子 Agent。

### 5.2 Echo 的默认工作纪律

1. 先读相关代码与适用工作区指令，区分可查明事实和用户拥有的决定；不把能查到的问题交回用户。
2. 非平凡任务给出简短计划与进度；独立研究可并行，实施时明确文件范围，避免重复委派和抢写。
3. 使用专用文件工具而不是用 shell 绕过权限；只做任务范围内变更，不顺手重构无关代码。
4. 技术取舍给理由；审美、数据删除、权限扩大、提交推送等按任务授权或交互确认处理。
5. 实施后按任务验收验证；已运行、未运行、推测和需要用户感官判断的项分开报告。
6. 输出平静、语言跟随用户、结论先行、有路径与测法，不使用戏剧化宣告或执行拟态；详细风格源自用户 Echo，而不是复制 Claude 的私有 system prompt。
7. headless 遇到必须由主控决定的事项返回 `blocked` 与问题；不无限等待，也不假造审批者。

这些是行为契约，不要求所有任务执行固定七阶段；改一条注释不必启动完整探索/设计流程。语言风格与模型能力不能用确定性单测等同证明，需要固定样例与人工评价。

### 5.6 授权级别：一个说法，四个入口（2026-10-14 定型）

**产品层只有一个说法：授权级别。** DSH 的沙箱档（`read-only` / `workspace-write` /
`danger-full-access`）是实现细节，不出现在命令行与界面上 —— 与 `effort` 不用 opencode 的
`variant` 是同一个道理。

| 级别 | 含义 | → DSH 沙箱 | → 策略档 |
|---|---|---|---|
| `readonly` | 只读工作区：不能写、不能跑命令 | `read-only` | `workspace-readonly` |
| `workspace` | 工作区内可读写、可执行 | `workspace-write` | `workspace-development` |
| `full` | 沙箱之外的全权（worktree 里提交必需） | `danger-full-access` | `workspace-development` |
| `confirm` | 与 `workspace` 同权限，但每次改动/执行前确认 | `workspace-write` + 应答者 | — |

四个入口各划各的，**不叠加**：

| 面 | 谁划定 | 默认 | 运行中能不能改 |
|---|---|---|---|
| **CLI · 一般模式** | 用户显式带 args（`--permission`） | `workspace` | **不能** —— 一次划定，过程不再二次修改 |
| **CLI · 工人** | 任务书（`constraints`）**收紧**启动配置 | 启动配置 | 不能；任务书只能收紧，不能放宽 |
| **TUI** | `/permission <级别>` | `workspace`（工作区内直接改、直接跑） | 能，但要显式下命令 |
| **GUI** | 同 TUI | **`confirm`**（执行前确认） | 能，但要显式操作 |

三条纪律：

- **`confirm` 在 headless 明确拒绝**，不静默降级成 `workspace`。它需要一个应答者，
  而无人值守时没有（§6.4）。报错好过「以为在确认、其实直接执行了」。
- **收紧只有一个方向**：「启动配置上限 → Agent 档位 → 任务书 `constraints`」，
  没有能以更后面的 allow 覆盖前面 deny 的顺序（§6.1）。
- **授权不是提示词。** 级别落到 `DSH_PERMISSION_MODE` 与生成的 profile 上，由执行层生效；
  system prompt 里写一句「请不要写工作区之外的文件」不算授权。

### 5.3 提示词与配置装配

保留 DSH 基础指导、真实工具说明及上下文管理，再组合 Arc Deck 工程规则、Agent 行为、模式、Echo 风格、工作区指令与任务。策略高于提示词；项目文件与任务文字不得扩大启动时授权。

#### 5.3.1 装什么进 system prompt：三档（2026-10-14 定型）

用户裁定：「**人格特性生效于非 workers 场景，并且是用户开启个性化设置的情况下启用；
否则不记录、不调用、不修改，直接按模型供应商本体特性行事**」。

| 档位 | 什么时候 | 装什么 |
|---|---|---|
| `worker` | 工人模式 | 工程纪律 + 角色 + 模式行为。**不带风格** |
| `personalized` | 一般模式 + `personalization.enabled` | 上面这些 **+ 风格** |
| `bare` | 一般模式 + 个性化关（**缺省**） | **一段都不加** |

- 档位由 `cmd_run` 判定，写进物化的启动配置（`WorkerConfig.prompt_scope`），
  由 worker 在组装时读 —— 装配点只有一个。
- `bare` 是**空集**，不是「加得少」：这是「按供应商本体行事」的可判定形式，
  调用方据此写出空的 `personaSuffix`。
- 为什么工人不带风格：工人交出去的是结果与证据，风格不影响它们，而每一段提示词
  都要花掉上下文。工人那层是**功能契约**，不是人格。
- 判据与边界（哪些属于个性化、哪些属于宪法）见 `prompts/constitution.md`。
- **宪法尚未接进装配**：`prompts/constitution.md` 目前只是文档，不产生任何行为。
  接通它要等它的正文定稿。

- Echo 文本进入真正的 system prompt/persona 配置，不能仅塞进普通 user message 然后宣称导入完成。
- 使用 DSH profile/preset 与必要的 scoped 插件；SDK wire 没有 Agent/preset/tools 参数，worker 必须在启动前完成部署配置。
- DSH 自带 Plan 是协作状态，不是只读保证；不得不经适配直接要求 headless 调用其人工评审工具。worker 的 Plan 结果由主控接受，只有新的明确 Build 任务才实施。
- Agent 切换与压缩后仍维持模式/风格；测试检查实际模型请求与策略，不只检查 Markdown 文件存在。
- 工作区 `AGENTS.md` 与 Skills 优先由 DSH 的已有机制装载，避免第二次重复注入；worker 默认不加载未经指定的用户全局 Skills。来源与有效配置可诊断，不输出凭据。

### 5.4 首版工具契约

以下是 Arc Deck 的首版逻辑工具规格，不是对 Claude Code 或 DSH 现有工具名称的声明。M1/M2 必须生成版本化 JSON Schema、给出实际 DSH 名称映射，并在模型可见目录中只暴露一种对应能力，避免别名与原工具同时绕过策略。能保留上游 schema 的能力优先保留；必要的薄适配不得重造执行器。

| 逻辑工具 | 必须定义的输入 | 输出/失败要求 | 首版授权 |
|---|---|---|---|
| 文件读取 | `path: string`；可选 `offset: integer >= 1`、`limit: integer >= 1`，均为行数 | 原文、行范围、是否截断；二进制/不存在/越界给明确错误，不返回伪造空正文 | workspace 读取范围；敏感文件另受策略限制 |
| 文件匹配 | `pattern: string`；可选 `root: string` | 确定顺序的路径与截断信息；不得匹配后自动读取 | 同上 |
| 内容搜索 | `pattern: string`；可选 `root: string`、`include: string` | 路径、行号、匹配文本及截断；明确正则语义与错误 | 同上 |
| 文件修改 | `patch: string`，格式版本固定 | 实际变更路径、失败原因；发布前明确多文件失败是否原子，不能称部分写入为全成功 | Build 的可写范围；Plan 执行层拒绝 |
| 命令执行 | `command: string`、`cwd: string`、`timeout_ms: positive integer`；shell 类型取运行时能力 | stdout/stderr、exit code、timeout/cancel 状态、完整输出位置；非零退出不伪装成功 | Build 明确授权；Plan 首版不开放任意 shell |
| 进度管理 | 带稳定 ID 的任务条目与状态 | 状态更新；不作为任务完成或测试通过的证据 | 不触及工作区文件的产品簿记 |
| 子任务委派 | 任务说明、Agent、文件范围、验证要求；可选已有子任务 ID | 子任务 ID、状态与证据引用；失败/取消不可只给一段成功文案 | P2；继承不可放宽的父策略 |
| 询问用户/主控 | 问题与必要上下文，选项如存在需明确 | 交互端获答；headless 无应答者时生成阻塞原因 | 不得通过「未回答」默许实施 |

公共输出使用一致的工具调用 ID、状态、结构化错误 `code/message`、内容与 artifact 引用。结果有完整大小限制和多字节计数测试；截断必须可见，完整内容单独保存。模型可见说明讲能力、参数、限制与反馈，不暴露 GUI/IPC 实现细节。

文件范围声明不是 OS 沙箱。尤其 shell、MCP、scope-local 工具和委派链必须经过实际执行限制；禁止仅靠工具列表隐藏、提示词、命令前缀匹配或 UI 禁用声称隔离。首版 Plan 默认拒绝修改、任意 shell、未经审核 MCP 和可写委派；安全读取白名单与必要的只读工作区沙箱另行验证。

### 5.5 Skills 与 Hooks

保留 DSH 的按需 Skills 装载，区分 Agent 角色与知识/流程包；Skill 脚本不是自动获得授权的工具。完整 Claude/OpenCode 插件兼容、用户自定义 Hooks 与插件市场不进入首版。P2 可增加执行前/后、任务结束的扩展点，但不能放宽上层 deny；超时、失败和阻断必须有明确行为。

## 6. Headless worker 需求

### 6.0 命令面（`arcdeck`）

可执行文件改名为 `arcdeck`（现为 `arc-deck`，M2a 改；crate 名不动）。参照 OpenCode 的
`opencode` / `opencode run` 分法：**无参数是交互，有子命令是 headless**。

```text
arcdeck                                   当前目录进 TUI（M4a：独立运行/历史/取消；非交互终端退出 64）
arcdeck run <任务书> [选项]                前台执行一件任务；stdout 是 JSONL 协议，退出码 = 终态（§6.3）
arcdeck run "<要对模型说的话>" [选项]       一般模式：不给任务书、不带工人参数；stdout 是给人看的回答
arcdeck run <任务书> --detach [选项]       分离执行：等到工人接住（写出状态）才返回；接不住就报出原因
arcdeck status [<id>…|--all] [--json]     每件任务一行；单件时退出码编码状态（§6.5）
arcdeck wait <id> [--timeout <时长>]      阻塞到终态或超时，打印一行；退出码同 status
arcdeck report <id> [--json]              终态的紧凑回报（§6.5.4）
arcdeck logs <id> [--tail N] [--events]   需要排查时才看的原始输出
arcdeck cancel <id>                       写取消请求并等待有界收尾
arcdeck usage [--json]                    各任务的 token 用量与合计（只有 provider 报告的，无费用）
arcdeck ls [--state-root <路径>]          列出状态目录里的任务
arcdeck worker run --config … --task …    M1 的底层入口，保留为 `run` 的别名，参数不变
```

`run` 的常用选项：`--config <路径>`、`--workspace <路径>`（缺省 = 当前目录）、`--id <任务ID>`
（缺省取任务书文件名）、`--by <谁>`（§6.2）、`--permission <级别>`（§5.6）、`--accept <命令>`
（可重复，追加 `command` 类验收）、`--preamble <文件>`、`--worktree <分支>`（§6.6）、
`--model <provider/model>`、`--effort <级别>`（模型强度；协议里叫 `reasoningEffort`）、
`--format <jsonl|text>`、`--timeout <时长>`。

**一般模式与工人模式（2026-10-14 定型）。** 判据是**工人参数**，不是「参数是不是文件」：

| | 触发 | stdout | 工人守则 | 验收 |
|---|---|---|---|---|
| **一般模式** | 位置参数是一句话，且没有 `--accept` / `--worktree` / `--git-base` / `--by` / `--preamble` | 给人看的回答（`--format text`） | 不带 | 无 |
| **工人模式** | 位置参数是任务书（已存在的文件，或 `-`），或出现上面任一工人参数 | 控制面 JSONL | 带上 | 按任务书 |

一般模式下**装不装 Arc Deck 的人格**由 `personalization.enabled` 决定，**缺省关**：
关的时候一段都不注入，按供应商本体行事（§5.3.1）。

**Arc Deck 不是一个工地** —— 它只是一个带工人优化方案的 agent（用户 2026-10-14）。
所以不给任务书、不带工人参数时，它的行为与 `opencode run "…"` 同类：问一句、答一句。
**两种模式都落状态目录、都写 `result.json`** —— 外部监控程序一视同仁，不必分两套读取。
一般模式的缺省任务 ID 是 `run-<毫秒>`（每次唯一）：同一句话问两遍不该被
「重复 task_id 不重跑」拒掉，那条规则是给工人任务用的。

位置参数看着像路径（无空白、以 `.md`/`.json` 结尾或含分隔符）却不存在时**明确拒绝**，
不把它当成一句话发给模型 —— 那多半是任务书路径打错了，静默改语义会得到一个莫名其妙的回答。

**启动配置的查找顺序**：`--config` → `$ARCDECK_CONFIG` → 用户配置目录下的 `arcdeck/worker.json`。**永远不从 workspace 里找配置** —— 任务书和工作区文件不得扩大启动时授权（§5.3）。找不到就退出 64，
不用内置默认值顶上。

**`--detach` 是一次派工握手，不是 fork 完就走。** 只 `spawn` 就返回 0，会把预检失败
（缺密钥、运行时没装、Agent 不认）整个藏进子进程：派工的人拿到「started」，而
`arcdeck status --all` 报「没有任务」，原因只躺在 `console.err` 里 —— 那正是 §6.5 第四个
问题（漏了就静默消失）的翻版。所以 `--detach` 等到**工人写出 `status.json`** 才算派工成功；
子进程在任何状态写出之前退出时，由派工方把这次失败**代写进 `status.json`**（终态 `failed`
＋ `last_error`）再原样报出来。判据是确定的（状态文件出现 / 子进程先退出），不靠时间猜：
只是慢、还活着的进程照常报「已启动」。

### 6.1 首版调用方式

```powershell
# M1 已实现的形式（离线夹具）；M2a 起同一形式接真实 DSH。
arcdeck worker run --config worker.json --task task.json --format jsonl --result result.json --cancel-file cancel.json
# --task - 从 stdin 读取一份 JSON 任务书；不以 shell 解析 task 内容。
```

同步 worker 是 P0 接口，`--detach` 与 `status/wait/report` 是同为 P0 的探活面（§6.5）。常驻服务属于后续阶段，不先要求 MCP；外部 Claude 可以直接运行命令并解析结果。

`--config` 为必需的主控启动配置，与任务书独立；不得自动查找 workspace 的配置、用户 Desktop home 或 PATH 上的 runtime。启动配置 v1 至少包括：

- `schema_version: 1`；`runtime` 的类型、显式入口、支持版本与部署 profile；独立的 `state_root`。
- `authorization` 的 workspace、读取/写入范围、shell 策略、联网、提交/推送开关，以及可用 Agent。没有指定的权限默认拒绝；未知策略 ID、缺失配置、workspace 不匹配与任务越权报结构化配置错误，退出 `64`，不启动内核。
- 环境、凭据与 Skills 的显式来源；日志只显示配置来源与有效授权，不显示秘密。

解析顺序为「启动配置授权上限 → Agent/模式限制 → 任务 constraints 收紧」，没有能以更后面的 allow 覆盖 deny 的顺序。首版内置 profile 至少有 `offline-test`、`workspace-readonly`、`workspace-development`；profile 规定能力模板，但具体路径和网络上限仍必须由启动配置提供。`workspace-development` 不自动授予提交、推送或整个磁盘访问。

M1 只支持 `runtime.kind=fixture-sdk` 与 `offline-test`，且仅在 `test-runtime` 构建 feature 下启用假 SDK 子进程；fixture 不读取真实环境凭据、不联网、不执行任务书里的命令。未启用该 feature 或出现其他 runtime 类型时拒绝，不回退真实 DSH。M2a 才增加真实 `dsh-sdk` 配置。

### 6.2 任务书 v1

```json
{
  "schema_version": 1,
  "task_id": "fix-session-filter",
  "workspace": "C:/work/project",
  "agent": "echo",
  "mode": "build",
  "assigned_by": "sol",
  "instruction": "修复会话筛选，并补回归测试",
  "constraints": {
    "read_paths": ["."],
    "write_paths": ["src/", "tests/"],
    "shell_policy": "workspace-development",
    "allow_commit": false,
    "allow_push": false
  },
  "acceptance": [
    { "id": "filter-regression", "kind": "command", "command": "cargo test session_filter" }
  ],
  "timeout_seconds": 900
}
```

- `workspace` 是显式绝对路径；范围相对 workspace 解析，检查路径逃逸、链接与平台大小写语义。示例不是所有项目通用的测试命令。
- `agent/mode` 可省略并按 §5.1 解析；`constraints` 可缺省为启动配置上限再取模式限制，但缺省不等于 full access。显式要求越过上限时拒绝，不静默扩大或裁剪要求。
- `assigned_by` 是**派工关系的边**（谁把任务书递过来的），不是授权。它是自由文本（`claude` / `sol` / 某个任务 id），因为 Arc Deck 里几乎全是工人，树的上两层（Claude Code、Codex）**不是 Arc Deck 任务**、没有自己的状态目录 —— 记任务 id 会指向查不到的东西。命令行给法 `arcdeck run <任务书> --by <谁>`；用途是让外部监控程序画任务书流向，不影响任何判定。
- `acceptance` 可省略为 `[]`；v1 每项均为必需项，ID 唯一，只支持下面两类。空列表只表示没有额外交付门槛，不表示代码已经测试。
- 同一状态目录重复 `task_id` 不自动重跑；重试用新 ID 并关联原任务。首版不承诺跨崩溃 exactly-once。
- 超时为整个任务的墙钟预算，不是单次 LLM 的输出上限。总 token/费用硬预算只有在完整 accounting 与停止机制验证后才开放，不做假预算开关。

验收联合类型与执行规则：

| 类型 | v1 输入 | 执行与结果 |
|---|---|---|
| `command` | `id/kind/command` 必需；`cwd` 可省略为 workspace；`timeout_seconds` 可省略为剩余任务预算内至多 120 秒 | Build 通过同一 DSH 工具链执行；shell 取启动配置已验证的能力。Rust 从当前任务的真实 command/cwd/退出记录采集证据，不另起绕过权限的 verifier；模型文字不是证据 |
| `manual` | `id/kind/description` 必需 | 等主控/人工检查，当前同步调用记录 `pending` 并返回 `blocked`，不伪造自动批准；主控可在外部接受结果，worker 不回写已终止状态 |

Plan 中的 `command` 验收、或任何模式下超出启动授权的验收，在预检时以 `ACCEPTANCE_NOT_ALLOWED` 拒绝（退出 `64`），不得借验收身份绕过限制；Plan 可以无验收或使用 `manual`。Build 指令必须包含获准的验收要求：实际命令未执行/缺少可靠退出记录记 `not-run` 并返回 `blocked`；真实非零退出记 `failed`；退出 0 且输入与证据匹配才记 `passed`。M1 用假记录验证映射，绝不执行 fixture 内的验收命令。

### 6.3 生命周期、事件与回报

阶段：`accepted → starting → running → verifying → terminal`。允许在任一阶段失败/受阻；阶段是产品状态，不直接等同 DSH 的某一帧。

JSONL 信封固定含 `schema_version/task_id/seq/type/data`，本任务 `seq` 从 1 递增。首版事件至少含状态、工具开始/结果、诊断和最终结果；SDK 没有当前 attempt 的逐 token 通道，不承诺 worker token 流。stdout 只输出协议，stderr 输出脱敏诊断；下游断开按 §6.4 取消，不能留孤儿任务。

最终结果至少包含：

| 字段 | 含义 |
|---|---|
| `schema_version/task_id/status` | 版本、任务身份、最终状态 |
| `summary` | 模型撰写的摘要，标识为陈述而非执行证据 |
| `session_id/runtime_version` | 内核会话与已核实的运行时标识 |
| `changes` | 主控可检查的 diff/artifact 与相对基线的文件变化；不只引用模型自报 |
| `verification` | 验收项、命令、cwd、exit code、证据位置、passed/failed/pending/not-run |
| `unverified/blockers` | 未验证事项、权限/依赖/澄清缺口 |
| `artifacts` | 原始事件、完整工具输出及诊断文件的受控引用 |
| `usage` | provider 实际返回的用量；缺失留空，不填虚构成本 |

最终状态与 CLI 退出码：`completed=0`、`failed=1`、`blocked=2`、`cancelled=3`、`timed_out=4`、`unknown=5`；任务书/配置无效为退出码 `64`。具体错误仍由结构化字段说明。恢复过的临时工具失败不直接导致整任务失败，但未恢复的内核错误与未满足的必需验收不能标完成。

`completed` 必须有当前任务的已确认终止、子任务收尾（如存在）且所有验收为 `passed`（空列表除外）；内核完成但验收 `failed` 时返回 `failed`，`pending/not-run` 返回 `blocked`。取消/超时优先保留对应终态，不由未运行验收覆盖。worker 无法写出结果或被外部强杀时，主控不得仅凭最后一段正文判成功，缺失 terminal 按 `unknown` 处理。任务拒绝也尽可能输出结构化诊断，不打印一段自然语言冒充协议。

`result.json` 与 JSONL 最终结果内容一致，采用完整写入后发布；写结果失败时不得退出 0。事件/结果路径属于控制面，不是授权模型修改的工作区文件。

### 6.4 隔离、取消与并发

脚本取消的跨平台入口是可选 `--cancel-file`：启动时该路径必须不存在，且处于主控拥有、模型不可写的控制目录；主控写入 `{"schema_version":1,"task_id":"当前任务ID","action":"cancel"}` 请求取消。worker 观察并校验任务 ID，记录原因后执行有界收尾；不删除主控文件。非法内容不触发取消，记录可见诊断。M1 从这个外部入口验证取消，不只直接调用内部函数。

交互运行另处理 `Ctrl+C`，Unix 处理可捕获的 `SIGTERM`，二者进入同一收尾路径。`--task -` 读完一份任务书后的 stdin EOF 是正常输入结束，不是取消；stdout 消费端断开则请求取消并尽可能写 `--result` 文件。`SIGKILL`/强制结束进程无法保证最终回报，主控记 `unknown` 并检查/清理任务进程树，不将强杀返回当成已确认取消。Windows 的无人值守脚本使用 cancel-file，而不是假定它能给所有 detached 进程发送 Ctrl+C。

**监工已经死了的时候，`arcdeck cancel` 自己收进程树。** 写取消文件的前提是有人读它；
监工被 `kill -9` 之后没人读了，而它拉起的运行时（以及运行时拉起的工具）会一直活着 ——
**实测**（Arch，2026-10-14）：监工被杀后 DSH 运行时仍是活的孤儿，没有任何东西会回收它，
而它会一直占着工作区与配额。所以 `cancel` 在下面两种情形下会按 `status.json` 里的
`runtime_pid` 直接收掉那棵进程树（Unix 整组 SIGKILL 并**有界确认**组空了才算数）：

1. 一开始就发现监工确认不在（`health = dead`）；
2. 写了取消文件、等了 30 秒没收尾，再查一次发现监工已经死了。

这是最后一道防线，不是常规路径：常规路径仍然是「写取消文件 → 监工自己收尾」。

- 一任务一 DSH runtime；独立任务数据目录，明确 `DSH_HOME`、`DSH_AGENTS_HOME`、cwd、环境、凭据来源与技能来源，不复用 Desktop 真实 home。
- SDK 无单会话取消；按有界 shutdown → 进程树终止进行收尾，确认退出后才能称取消完成。Windows 与 Unix 分别验证，不能只杀父进程。
- 并发实施前使用独立工作树/目录与文件所有权；不自动合并，不覆盖用户原有变更，不清理未由本工具创建的数据。
- 无人值守 approval ask 必须明确拒绝/受阻；不为了通过测试默认启用 `danger-full-access` 或自动批准全部请求。

### 6.5 探活：便宜地知道「做完 / 在跑 / 卡死 / 出错」

Ether 现在的 runner 脚本（`tools/dispatch/dsh-runner.sh`、`runner.sh`）暴露了四个问题，本节逐条对着解决：

1. 卡死判据是「输出 30 分钟不增长」—— 一次 20 分钟的 `cargo build` 与一次挂住的模型请求分不开，判出来要 30 分钟。
2. 结果靠 `tail -n 40` 日志与工人自写的报告 —— 调度者每次检查都在读模型生成的长文本，这就是花钱的地方。
3. 续跑靠 grep 网络错误与 DSML 字样。
4. 后台化靠 `nohup`，漏了就静默消失。

#### 6.5.1 任务状态目录

`<state_root>/<task_id>/`：

| 文件 | 写者 | 内容 |
|---|---|---|
| `task.json` | `run` 启动时 | 归一化后的任务书（Markdown 任务书记路径与哈希） |
| `status.json` | 监工，**原子替换** | 下列字段；至少每 `heartbeat_s`（缺省 15 s）写一次，有进度时节流写 |
| `events.jsonl` | 监工 | 与前台 stdout 相同的 JSONL 协议（§6.3） |
| `runtime.log` | 监工 | DSH 的 stderr，脱敏 |
| `result.json` | 监工，完整写入后发布 | 终态结果（§6.3） |
| `cancel` | 调度者 | 取消请求（§6.4）；`arcdeck cancel` 代写 |

`status.json` 的字段：

- `schema_version`、`task_id`、`state`（`accepted/starting/running/verifying` 或终态）、`host`
- `pid` 与 `pid_started_at`（监工）：**判进程是否还在时两者都要比** —— 只看 pid 存在会被 pid 复用骗过去。
  `pid_started_at` 缺席 = 这个平台查不到（或这条记录是派工方代写的），读者退回心跳判据。
- `runtime_pid`：运行时进程的 pid。监工被强杀之后，收它拉起的进程树就靠这一个字段（§6.4）
- `assigned_by`、`title`：派工的边与节点标签，给外部监控程序用；缺席不写（§6.2）
- `started_at`；`heartbeat_at`（监工还活着）；`progress_at`（最后一次来自 DSH 的**实质**事件：模型输出块、工具开始或结束）
- `activity`：`{kind: model|tool|verify|idle, name, summary（≤80 字）, since}` —— 此刻在做什么
- `counters`：`steps`、`tool_calls`、`tool_errors`、`commits`（worktree 上 `main..HEAD` 的提交数）、`model_retries`（中途的模型侧失败次数，见 §6.5.7）
- `notice`：`{code, message, retryable, at}` —— 此刻正在出问题的东西（可能自己好）。**存在不等于失败**，见 §6.5.7
- `attempt`、`max_attempts`、`last_error: {code, message（≤200 字）, retryable}` —— `retryable` 见 §6.5.6
- `usage`：provider 实际报告的输入 / 缓存命中 / 输出 token；没有就不写
- `limits`：`{heartbeat_s, model_idle_s, tool_grace_s}` —— 判定用的阈值跟着文件写出，读者不必去猜配置

预检失败（工人还没开始就死了）由**派工方**代写一份终态 `status.json`：`state=failed`、
`terminal=true`、`pid=0`（不冒充一个活着的写者）、`attempt=0`（一次都没真正跑起来），
`last_error` 是预检报的那个结构化错误。没有这一条，一次配置错的派工在状态面上完全不存在。

#### 6.5.2 健康判定：由读者计算，不由写者自报

`arcdeck status` 读 `status.json`，结合**当前时间**与**进程是否还在**算出 `health`：

| health | 条件 |
|---|---|
| `done` | `state` 是终态 |
| `ok` | 心跳新鲜，且 `progress_at` 在阈值内 |
| `busy` | 正在跑工具，已超过 5 分钟但没超过该工具自己的超时 + `tool_grace_s` —— 长构建是正常的，不算卡 |
| `stalled` | 心跳新鲜，但：在等模型且 `progress_at` 超过 `model_idle_s`（缺省 300 s，对齐 DSH 的 `streamIdleTimeoutMs`）；或工具超过其超时 + 宽限；或空闲超过 `model_idle_s` |
| `dead` | 非终态，且**监工进程确认不在**（快路径，立刻判），或心跳超过 `3 × heartbeat_s`、或监工进程不在 → 按终态 `unknown` 处理 |

**为什么由读者算**：写者死了就写不出「我死了」。监工只写事实（时间戳、pid、在做什么），判定在读的那一刻按事实算。

**进程还在不在：比 `pid` 也比 `pid_started_at`。** 只看 pid 存在会被 pid 复用骗过去。
判据（`status::supervisor_liveness`）：

- **只在同一台机器上才可信** —— 先比 `host`，对不上就返回「不知道」，退回心跳判据。
  pid 只在它自己的机器上有意义；经 ssh 读远端状态时，跑的是远端那个 `arcdeck status`，所以照样成立。
- **Linux**：读 `/proc`（纯 `std::fs`，无 FFI）。已实测：`kill -9` 监工之后，
  `arcdeck status` **1 秒内**报 `dead`，而不是等 45 秒。
- **其余平台**：一律返回「不知道」并退回心跳判据。这是**刻意的降级**，不是遗漏 ——
  本仓目前没有可编译验证 Windows 目标的手段（开发机没装 Windows target，
  写了连 `cargo check` 都过不去），所以不写不能验证的实现。

#### 6.5.3 一行输出与退出码

```text
fix-filter  running  ok       tool:bash 4m12s "cargo test -p sysstate" · steps 42 · tools 118 · commits 2 · 18m · att 1/3
fix-filter  running  stalled  model 11m 无输出 · att 2/3（监工将重启）
fix-filter  failed   done     验收 filter-regression 退出 101 · commits 2 · 31m · 详情：arcdeck report fix-filter
```

单件 `status` / `wait` 的退出码：`0` completed · `1` failed · `2` blocked · `3` cancelled · `4` timed_out ·
`5` unknown（含 dead）· `10` running（ok / busy）· `11` running（stalled）· `64` 用法错或找不到任务。
终态部分与 `run` 的退出码一致。`--json` 输出 `status.json` 加上算出的 `health`，仍是一行。

调度者的典型用法是 `ssh ether arcdeck status --all`：每件任务一行，不读日志。

#### 6.5.4 监工自救（取代 runner 脚本的看门狗）

- **网络 / 429 / 5xx**：先交给 DSH 的 `llm-retry`（在 step 边界重试）；Arc Deck 不对同一请求再叠一层重试。
- **stalled**：杀 runtime 进程树 → 开新 attempt：新的 DSH 会话，提示里加「续跑说明」（先看
  `git status` 与 `git log main..HEAD`，接着做），与 `dsh-runner.sh` 同法。上限 `max_attempts`（缺省 3），
  用尽 → `failed`，`last_error.code = STALLED`。
- **工具调用被当成正文吐出然后收工**（DSML 等）：识别为失败类 `TOOL_CALL_AS_TEXT`，同样有界续跑，次数记进 `status`。
  官方路由上预期为 0，这个计数就是 DSH_FINDINGS E1 的数据来源。
- **续跑不是 exactly-once**：上一次 attempt 的未提交改动留在 worktree 里给下一次接手；回报里写明发生过几次续跑。

#### 6.5.5 紧凑回报（`arcdeck report`）

- 内容分两类并**分开标注**：
  - **Arc Deck 采集的事实**：终态与原因码、验收命令与退出码、`main..HEAD` 的提交列表、`git diff --stat`、未提交文件、用时、attempt 次数、usage。
  - **模型的陈述**：摘要、未完成 / 不确定、范围外发现 —— 取自最终消息里的固定小节（Ether 工人守则第 10 条的四节）；缺小节标「模型未给出」，不报错。
- **开头三行回答派工的五件事**：谁派的（`assigned_by`）、干什么（任务书标题）、
  什么样（终态 + 用时 + attempt 次数）、为什么（第一个原因码 + 原文）、要不要再派一次
  （`retryable`，见 §6.5.6）。目的是**不必**再打开 `result.json` 或 `events.jsonl` ——
  一条命令决定「接受 / 重派 / 自己看」。
- 缺省输出控制在约 2 KB；长内容只给路径（完整最终消息、diff、工具输出）。
- 回报让「要不要细看」变便宜，**不替代审查**：调度者仍按 Ether DISPATCHER §四 自己看 diff 与截图。

#### 6.5.6 失败原因：可判定的码，不用回去翻日志

调度者要的回答是「要不要再派一次」，不是一段英文。所以每条终态失败都带一个稳定的
**原因码**与一个 `retryable`：

| 码 | 意思 | retryable |
|---|---|---|
| `MODEL_ROUTE` | 模型名/路由不被接受（如实测原文 `supported API model names…`） | false |
| `MODEL_QUOTA_EXHAUSTED` | 额度/余额用尽 | false |
| `MODEL_AUTH` | 401/403、密钥无效 | false |
| `MODEL_RATE_LIMITED` | 429/529、限流 | **true** |
| `MODEL_SERVER_ERROR` | provider 侧 5xx | **true** |
| `MODEL_NETWORK` | 连接/代理/DNS 类 | **true** |
| `MODEL_CONTEXT_LENGTH` | 上下文超限 | false |
| `MODEL_FAILURE` | 认不出的兜底 | false |
| `ACCEPTANCE_FAILED` / `ACCEPTANCE_UNFINISHED` | 验收没过 / 没执行 | false |
| `TOOL_CALL_AS_TEXT`、`STALLED`、`RUNTIME_EXITED`… | 见 §6.5.4 | 前两者按 §6.5.4 处理 |

- 模型侧的原文只从一条通道出来（`finish.reason.failure`，参 AGENTS.md P3），归成上表；
  **原文永远保留在 `message` 里** —— 标签可能认错，原文不会。
- 验收失败也必须有码。没有它，一条验收不过的任务在状态面上只有一个 `failed`，
  调度者还得回去读 `result.json` 才知道是哪一条命令、退出码多少。
- `retryable` 是给调度者的**建议**，不是自动动作：Arc Deck 不会因为它去重试
  （§6.5.4 的重试只针对卡死与 DSML 泄漏）。判定表在 `arc-deck-core::failure`。
- **认不出就保守**：`MODEL_FAILURE` 且 `retryable = false`。分类错成「不可重派」只是多
  看一眼；错成「可重派」会白烧一次配额 —— 两边代价不对称，所以往保守那边倒。

**这张表是尽力而为，不是规范。** 上游没有给错误分类，provider 的措辞也不保证稳定；
它的输入目前只有 `finish.reason.failure` 这一条通道。其它失败形状（例如网络断在运行时
自身、或网关在握手阶段就拒绝）**没有真实样本，尚未分类** —— 认得出的一律是子串匹配，
认不出的一律落到兜底。要收紧这张表，得先攒真实原文。

上游给了**结构化字段**时优先用它：`turn/end` 的 `reason.error.code` / `status`。
真实会话里额度用尽就是 `{message:"Insufficient Balance", code:"QUOTA", status:402}` ——
比猜那串英文硬得多。所以顺序是「结构化 code → 结构化 status → 子串匹配 → 兜底」。

#### 6.5.7 中途：报错但**自起了**不算失败

断网五分钟、被限流、provider 抖一下 —— DSH 的 `llm-retry` 会在步边界重试并继续。
这种任务**不该**报失败。真正值得打扰调度者的是「从动到不动」这个跃迁，不是每一次报错。

（用户 2026-10-14 的真实场景：手机热点 + 那台机器的热管理，会无征兆断网五分钟；
旧 runner 下 worker 直接挂掉，而派工那一侧全程只看到 `running`。）

两条分开处理：

| | 落在哪 | 给谁看 |
|---|---|---|
| **最终失败**（轮次以 `error` 收尾） | `result.json.blockers` + `status.json.last_error` | 终态原因与「要不要再派」 |
| **中途失败**（重试后继续） | `status.json.counters.model_retries` + `notice`；`result.json.unverified` 留一条痕 | 实时反应 |

`notice` 是 `{code, message, retryable, at}`：说的是**此刻正在出问题**，任务可能还在跑、
还可能自己好。监控程序靠它实时反应（例如正在被限流时先别再派活儿），而不是等终态。
**`notice` 存在不等于任务失败** —— 这是它与 `last_error` 的全部区别。

### 6.6 Ether 接入

目标：用 `arcdeck` 取代 Ether 的 `dsh.sh` + `dsh-runner.sh`（以及 `oc.sh` + `runner.sh`），调度手册的派工一节缩成一条命令。

| runner 脚本现在做的事 | Arc Deck 里的对应 |
|---|---|
| worktree + 分支 `dsh/<id>`，已存在就复用（续跑） | `--worktree <分支>`：在 workspace 仓上 `git worktree add`（分支已存在则复用），之后 workspace 就是这个 worktree；位置模板（如 `<仓父目录>/.oc/wt/{task_id}`）由启动配置给 |
| 子模块初始化 | worktree 新建后 `git submodule update --init`；失败写进 `last_error` 并继续（与脚本一致） |
| `CARGO_TARGET_DIR` 每任务独占 | 启动配置的 `env` 模板，支持 `{task_id}` 替换；Arc Deck 不写死 cargo |
| 代理（Windows 借 10809，Linux 走反向隧道） | 启动配置的 `env`；反向隧道仍由调度者那端的网络建 —— 那是调度者的网络，不是 worker 的职责 |
| `PREAMBLE.md` 与任务书拼成一条消息 | `--preamble` 进 system prompt 的 persona 后缀（每件任务都一样，稳定可缓存，DSH_FINDINGS §4.2）；任务书正文进用户消息 |
| `dsh --profile headless`，`danger-full-access` | per-task `DSH_HOME` + Arc Deck 生成的 profile；级别是 `full`（§5.6），写范围是 worktree。Ether 工人要提交，所以由启动配置给 `allow_commit: true`；推送始终拒绝 |
| 30 分钟看门狗、grep 网络错误、grep DSML | §6.5.4 |
| `nohup … &`，日志在 `.oc/logs/` | `--detach`；`state_root` 可配到 `.oc/state/` |
| 收尾打印 `git log main..HEAD` / `git status --porcelain` | 回报里「Arc Deck 采集的事实」 |

**`--worktree` 必须配 `permission = full`，否则派工时会警告。**
worktree 里的 `.git` 是个**指向主仓**的指针文件，而主仓在 workspace 之外 —— 沙箱不是
`danger-full-access` 时，worktree 里的 git 命令一律失败。**实测**（Arch，2026-10-14，真实模型）：

```
git status --short / git log --oneline -1  →  退出码 128
整轮 34 秒、8342 输入 token，以「验收未通过 accept-1（退出 128）」收场，commits 0
```

同一件活改成 `danger-full-access` 之后：真提交 `132627b docs: append hello to README`、
验收 `git log --oneline -1` 通过、`report` 里的 git 事实取自 git 而不是模型自报。
所以这件事在**派工那一秒**就能说清，不该让工人跑完才知道 —— `arcdeck run --worktree`
在沙箱不是 `danger-full-access` 时会往 stderr 打一条明确警告。

**任务书**：Ether 的 Markdown 任务书（TASK_TEMPLATE 的五段）原样作为 `instruction`；机读字段从命令行给（`--id`、`--accept`、`--worktree`），或在 JSON 任务书里用新增的可选字段 `instruction_file` 指向 Markdown。
**不解析 Markdown 的「验证」段来猜验收命令** —— 猜错就是假通过；验收命令由调度者显式给。

Ether 侧的改动（新的派工脚本、DISPATCHER §三 的改写）在 Ether 仓里按 Ether 的流程做；
本仓只保证接口，并给一份可抄的示例脚本 `examples/ether/`。

## 7. TUI 视觉与交互需求

**排期**：完整 M4 在 headless 可用（M2）与权限实装（M3）之后。本节规格保留不变。
2026-10-14 用户要求开始 TUI，先落地 M4a 的独立运行/历史/取消入口；Plan/readonly/confirm
与 Remote 会话仍待接通，不把第一段当作完整 M4。Windows 暂用 F6 发送、Enter 换行，
原因与验证见 [本轮审计 §4–5](CORE_HEADLESS_AUDIT_2026-10-14.md)。

### 7.1 迁移目标与「v1」基线

**基线定为 OpenCode `v1.19.0` 的 TUI**（tag `v1.19.0`，提交 `733b2c627e5aec39cb33cc22373bdce33053ef2c`），
理由是它比 2.0.27 明显更小、更容易照搬：`packages/tui/src` 在 v1.19.0 是 **187 个文件**，
在 2.0.27 是 **315 个**。

「opencode v1」这个说法在同一份检出里对应三样不同的东西，这里写明取哪一个，免得实施时选错：

| 候选 | 实际是什么 | 是否采用 |
|---|---|---|
| `v1.19.0:packages/tui/src` | TypeScript + Solid + OpenTUI 的 TUI（187 文件） | **采用**：视觉与交互的迁移基线 |
| `v1.0.0:packages/tui` | 更早的 **Go** TUI（131 文件，其中 101 个 `.go`，含 `cmd/opencode/main.go`、`go.mod`） | 不采用：它是另一套技术栈，与 v1.19.0 的设计也不同 |
| `2.0.27:packages/tui/src/config/v1` | **配置 schema v1** 的兼容层（2 个文件，`TuiConfigV1`） | 只用来核对配置字段名，不是 TUI |

**一条必须先说清的代价：v1.19.0 的 TUI 与 2.0.27 是同一套栈**，都是 TSX + Solid + OpenTUI。
所以「照抄 v1」在 Rust 路线下**仍然是移植（port），不是复制** —— 语言边界不会因为换了基线而消失。
MIT 允许复制与改写，但复制过来的是 TSX，要一行行改写成 Rust。
真正能省下来的是**设计与交互**（布局、状态、键位、模型目录的组织方式），不是代码量。

首版不为了「省事」而在 Rust TUI 旁边塞一个 Bun 进程跑上游 TUI：那会把产品变成两个运行时，
也把启动、打包与更新全部复杂化。若移植量评估下来不可接受，应当把结论报出来由用户决定降级方案，
而不是无声地换栈。

**基线与主题 schema 是两件事，这里刻意分开取，因为两边的取舍不同：**

- **结构与交互取 v1.19.0**：Home / Session 主流程、输入区、侧栏、工具展示与折叠、命令面板。
- **主题 token 取 2.0.27 的语义集合**：`text` / `background` / `action` / `formfield` / `feedback` /
  `border` 这几组带状态的角色（hover / selected / disabled 等）。
  理由是 v1.19.0 的主题只是 `packages/tui/src/theme/assets/*.json` 下的一堆扁平调色板
  （aura、dracula、gruvbox…，没有 `v2/` 子目录），**没有状态角色**；而 v2 的语义 token 正是
  OpenCode 自己后来迁移到的形状，直接采用它比自造一套更省事，也不容易在状态配色上出错。

这是一次**有意的混搭**，不是笔误。若用户要的是纯 v1（连主题也照 v1 的扁平调色板），
那么 §7.3 的主题一行要跟着改，改之前先确认。

品牌文字换成 Arc Deck，默认 Agent 换成 Echo。不是 Kanesumi TUI，也不使用 WinUI 配色替代该参考。

**2026-10-14 补记：不要什么（用户看过 opencode `2.0.28` 之后裁定）。**

- **不要 session tabs。** `packages/tui/src/component/session-tabs.tsx` 与
  `session-tabs-rail.tsx` 在会话上方画了一排带序号、未读徽标（`activity` / `error`）、
  悬停态的页签，还用 `top: -1` 的「半行」把它画成贴着内容的浏览器标签 ——
  用户的原话是「这个愚蠢的浏览器式的分页头，我根本不需要」。
- **要沿用的**：**窗口适应、操作逻辑、构图**。这三样不因为上面那条一起丢。

判据是分工：opencode 的价值在**终端里的构图与交互**，不在把浏览器的心智模型搬进终端。
实施时如果发现某个设计是「因为在浏览器里合理」才存在的，它就不该跟过来。

Rust 必须复写 Solid/OpenTUI 组件行为。若 Rust renderer 无法达到所选效果，应报告具体差异并由用户决定是否接受，不无声降级视觉目标。

### 7.2 模型管理

用户指定这一块按 OpenCode 的做法来。v1.19.0 的实现是：`component/dialog-model.tsx`、
`component/dialog-provider.tsx`、`component/dialog-variant.tsx`、`component/dialog-agent.tsx`、
`util/model.ts`、`context/local.tsx`。

**数据形状**（Arc Deck 照此定义自己的类型）：

- 一个 provider 有一组 model：`Provider { id, name, models: Record<modelID, Model> }`。
- 一条路由写作 `providerID/modelID`；解析时按**第一个** `/` 切分，剩下的全部并回 modelID
  （v1 的 `util/model.ts` 就是这么写的，modelID 里带 `/` 也能用）。
- 展示名取 `model.name`，回退到 modelID；provider 名作为副标题。

**交互形状**（首版要有的）：

| 能力 | 行为 |
|---|---|
| 模型列表 | 可模糊搜索；搜索为空时**先列 Favorites 与 Recent**，其余按 provider 分组 |
| 收藏 / 最近 | 持久化在本地偏好里；Recent 里已经在 Favorites 出现的不重复列 |
| 变体 | 选中模型后按需进入**强度（effort）**选择（v1 的 `DialogVariant`）；没有强度档的模型直接跳过。**产品名是 `effort`，不用 opencode 的 `variant`**（用户 2026-10-14） |
| provider 维度 | 有独立的 provider 列表入口，可从模型对话框跳进去 |
| 代价提示 | 有明确的代价信息就显示（v1 在免费条目上打 `Free`）；没有数据就不显示，不编 |
| 不可用项 | 不可用的模型显式禁用并说明原因，不是从列表里悄悄消失 |
| Agent 维度 | Agent 与模型是两个独立维度，各自的对话框分开 |

**与 DSH 的接法**（Arc Deck 特有的约束）：

- provider/model 目录的**权威来源是 DSH**，不是 Arc Deck 自己维护一张表。M1 里路由写在启动配置的
  `model.provider` / `model.model`；M2 接真实内核后按内核实际报告的可用路由呈现。
- **worker 不能在任务中途换模型** —— 路由在 `initialize` 时就被解析并钉住。
  所以 TUI 里的「切换模型」是**开新的会话/运行时**，不是热切换。这一点必须在界面上说清，
  否则用户会以为换完了、实际这一轮还在用旧路由（这与 P3「provider 与 model 不是一回事、
  校验发生在轮次内部」是同一类陷阱）。
- 收藏与最近是 Arc Deck 的本地偏好，不写回 DSH；不要把本地偏好伪装成内核状态。

### 7.3 首版组件与规则

| 组件 | 待实现要求 | 验收 |
|---|---|---|
| 主题 | 语义 token 采用 2.0.27 的集合（见 §7.1 的取舍说明）：text/background/action/formfield/feedback/border，含状态角色；深浅色 | 深/浅主题各一组真实终端对照，状态不借用无关颜色 |
| Home | 居中 Arc Deck 品牌区、共用 Prompt、输入区最大 75 列、条件页脚 | 空会话可直接输入，不先强迫进入会话列表 |
| Session | 顶部会话导航、滚动正文、底部 composer；上滚停止追随，提供回到最新 | 流式更新不把用户拉回底部，不丢滚动锚点 |
| 输入框 | 左侧强调线、层级背景、多行输入、历史、agent/model metadata；宽度不足依次省略 | 粘贴/换行/CJK 不错位；有明确输入焦点 |
| 信息栏 | 默认宽 42 列；会话有效宽度（终端宽度减纵向 tabs，如存在）大于 120 列时自动并排，否则按需覆盖 | 不挤掉输入区；只显示真实会话/目录/分支/用量信息 |
| 命令与选择 | 可搜索命令面板、Agent/模型选择、会话列表；快捷键与 slash 共用 action 定义 | 无鼠标可完成主流程；不可用能力有原因 |
| 消息 | 用户正文、助手 Markdown/代码、思考折叠；权威正文替换临时流式段落 | 不重复正文，不将内部事件画成聊天 |
| 工具 | inline 摘要与 block 输出；运行/成功/失败/拒绝状态；长输出折叠、错误详情可达 | 折叠不隐藏权限阻塞/失败；完整输出有入口 |

首版保留切换会话期间的折叠状态；跨重启持久化只对明确定义的偏好实现，不声称所有局部状态均持久化。Context/费用/MCP 区域没有真实数据时省略或标不可用，不填演示数字。

### 7.4 首版默认键位

参考 OpenCode，Arc Deck 首版选择：`Ctrl+P` 命令面板；`Ctrl+X` 为 leader；`leader,a` Agent 列表；`Shift+Tab` Agent 切换；`leader,n` 新会话；`leader,l` 会话列表；`leader,b` 信息栏。`Enter` 提交，`Ctrl+J` 换行，支持可识别的 `Shift+Enter`；`Esc` 先关闭弹层/取消当前编辑状态，无弹层且任务运行时请求取消。具体上下文动作在 action 注册表中唯一维护。

模型与 Agent 相关键位（采自 v1.19.0 / 2.0.27 的同名定义）：`leader,m` 模型列表；`F2` / `Shift+F2` 在最近使用的模型间前后切换；`Ctrl+T` 循环变体；`Ctrl+A` 从模型对话框进入 provider 列表；`Ctrl+F` 收藏当前项。这些键在模型对话框内有独立上下文，与同名的会话级绑定不冲突 —— 冲突要按上下文解析，不能全局覆盖。

`/agents`、`/models`、`/new`、`/sessions`、`/plan`、`/build`、`/cancel` 是首版产品命令方向。**`/permission`**（授权级别，§5.6）、**`/usage`**（用量与合计）、**`/effort`**（模型强度）同样是首版必备。`/plan` 对 Echo 只切为 `echo + plan`、保留 Echo 风格；对 Build 切为 `plan + plan`；对 Plan 不变。`/build` 对 Echo 只切为 `echo + build`；对 Plan 切为 `build + build`；对 Build 不变。显式 Agent 选择取新 Agent 默认模式，未知 Agent 不静默回退。这些是用户动作，不是模型自行提权；每次仍受启动授权约束。

**命名纪律**：模型强度的产品名是 **`effort`**，不是 opencode 的 `variant`（用户 2026-10-14）。
DSH 协议里的 `reasoningEffort` 是上游字段名，映射在适配器里做，不往界面上漏。
同理，界面上说的是**授权级别**，不说 DSH 的沙箱档名。

运行中的模式切换须先结束/取消当前活动并确认，再影响下一轮。终端无法区分某个组合键时给可用替代键，不声称完全兼容所有终端。

### 7.5 视觉与运行验收

- 在同一终端、字体与配色能力下做 OpenCode/Arc Deck 对照，至少 `80×24`、`120×30`、`160×40` 终端格；另测 `40×12` 的可用降级。
- 夹具覆盖 Home、长消息、流式、工具展开/折叠、失败、权限、命令面板、深/浅色，不只检查主屏。
- 自动检查 buffer 文本/样式、裁剪、命中与滚动；真实终端截图与键盘操作检查 Unicode、中文输入、鼠标、粘贴、resize 和退出后终端恢复。
- 自动化证明布局与行为；迁移后的视觉是否满意仍由用户判断。当前没有运行过参考 TUI 或新 TUI，静态源码不是视觉通过证据。

## 8. 桌面 GUI 需求（冻结，不在当前焦点）

桌面版分两块（D6），**都等后端的本地服务接口（M5）稳定后再接**，现在不并行开发：

| | Windows | Linux |
|---|---|---|
| 技术 | C# + WinUI 2（`uwp/`） | Rust + Kanesumi（`kanesumi/`） |
| 约束 | U1–U17 | K1–K9 |
| 接后端的方式 | 版本化本地 IPC | 同一个 IPC —— 即便 Kanesumi 也是 Rust、能直接链接 core，也走 IPC，保证会话、权限与完成判定只有后端一份 |
| 先决验证 | 完整信任宿主（下文） | Wayland 外壳与 IME（Kanesumi 在 Ether 侧的进展决定） |

下文是 Windows 一侧的既有要求，保留。

继续 C# + WinUI 2，使用平台控件、主题、键盘/无障碍与既有列表/Markdown/设置经验；不将 OpenCode 终端视觉搬进 WinUI，也不继续推进 Kanesumi 自绘 GUI。

现有 `uwp/src/ArcDeck.csproj` 为 `AppContainerExe`，不能直接承担完整信任的 DSH/Rust 宿主职责。先验证完整信任桌面宿主 + WinUI 2 / XAML Islands：关键控件、主题、中文输入、UIA、资源、打包和启动均须运行检查，不承诺 XAML 原样迁移。

通过后，经版本化本地 IPC 消费 Rust core 的任务/会话/工具/结果；页面不直接解释 DSH 私有事件。验收必须包含去掉示例数据后的新建、发送、停止、历史、失败与设置持久化，并与 TUI 的同任务状态一致。

若宿主验证失败，备选为保留 UWP UI + 完整信任伴随进程/AppService；代价是额外生命周期与 IPC，需要另行决定。不得为绕过困难偷偷换 WinUI 3、WebView、关闭沙箱或要求用户日常提权。

## 9. DSH 适配、权限与数据

### 9.1 两种传输的边界

| 路径 | 首期用途 | 已知限制与验收重点 |
|---|---|---|
| SDK stdio | 单任务 worker | JSON-RPC 按换行分帧；prompt 回执只表示入队；用 `session/wait` 及当轮终止/错误检查收尾；没有单会话取消、历史恢复或当前 attempt 的实时 token 通知 |
| Remote | TUI/GUI 交互会话 | HTTP Connection 信封 + WS mux，与 SDK 不同；需要认证；历史分页、单会话取消及 `assistant-stream` 独立测试 |

产品能力按传输/运行时报告，缺失能力不静默模拟。Remote 使用 web composition 不意味着 GUI 使用 WebView；初期不另造精简 backend，只有已测的成本或能力缺口才推动新桥接。

### 9.2 升级与投影

根 C# `DshVersion.cs` 记录 `0.1.2-rc.1`，当前参考 DSH 为 `0.2.1-alpha.2`，**本机 npm 安装的 `dsh` 是 `0.1.7-rc.2`**（2026-10-14 `dsh --version`）。三者不一致：DSH_FINDINGS 的机制分析基于 0.2.1-alpha.2 源码，M2a 的第一步是选定一个版本、在 Windows 与 Arch 装同一版本并钉住，再核对那些机制在该版本里是否存在。新版本在 initialize 校验路由，durable assistant 事件与实时通道也已变化；不能只改版本常量、照搬旧 `assistant/chunk` 或打包行规则。

Arc Deck 为每件任务生成独立的 `DSH_HOME` 与 profile patch，**显式**写出以下各项而不吃 DSH 的默认值：模型路由与推理强度、persona（Echo + `--preamble`）、工具呈现方式（`native` / `ptc`，M2d 决定缺省）、权限与沙箱模式，以及 `session-log-deepseek`（会话日志随请求上传 DeepSeek，DSH 默认开，DSH_FINDINGS §2）。最后一项由用户裁定（§11）；裁定之前 Arc Deck 生成的 profile 关闭它。

- 验证运行时版本/产物来源并钉住支持组合；握手 `serverInfo.version=0.0.1` 不是可用的兼容判据。不支持版本明确拒绝或进入显式实验模式。
- 原始事件保留可诊断信息，产品正文过滤系统/内部簿记；用户来源按事件契约，不按内容长度猜。
- 分清 turn、step、attempt、权威消息与临时 stream；将工具配对、恢复与正文替换规则放在共用投影，界面不各自拼接。
- SDK/Remote/日志对账比较完整文本与结构，不只比较长度。负例覆盖等长不同正文、错误 ID、漏拆包装、失败 attempt 和中文跨分帧。
- `follow` 可能激活冷会话，不称其纯只读；测试只用隔离数据。分页/重连和实时 cursor 分开处理，不沿用旧版本未经验证的算法。
- 不自动打开、转换、覆盖或删除 Desktop 的真实会话。旧数据迁移另立带回滚与用户确认的任务。

### 9.2a DSH 怎么嵌入（2026-10-14 实测后定）

**不用 submodule，不 vendor 源码，不从源码构建。** DSH 在 npm 上按包拆开发布（`@deepseek-ai/dsh-*`），
`dsh` CLI 本身只是一个 12 KB 的启动器，按 profile 组合各个 bundle。所以 Arc Deck 的做法是：

- **私有、钉版本的运行时目录**：`arcdeck runtime install` 在用户数据目录下建
  `arcdeck/runtime/dsh-<版本>/`，写一份把 `@deepseek-ai/dsh` 钉成**精确版本**的 `package.json` 并 `npm install`。
  不碰全局 npm 的 `dsh`（本机那份是 0.1.7-rc.2，和 Desktop 共用，不能被我们升级），也不进 Arc Deck 的二进制或仓库。
- **启动**：`node <运行时>/node_modules/@deepseek-ai/dsh/lib/bin.js --profile sdk --patch <每任务生成的 patch>`，
  `DSH_HOME` 指向任务状态目录里的独立 home，cwd 是 workspace（DSH 的写入沙箱以 cwd 为根）。
- **体积**（0.2.1-alpha.2，Windows 实测）：整个 `node_modules` 约 549 MB；其中 webui 相关约 30 MB，**大头是
  Office 技能的 LibreOffice 侧车（184 MB）**与原生插件（node-pty、sharp、onnx）。体积不在 Arc Deck 的分发物里，
  一台机器装一次。要瘦身时先试 `--omit=optional` 去掉平台侧车，前提是验证 sdk profile 仍能启动 —— 未做。
- **补丁是 JSON 写的 YAML**：YAML 是 JSON 的超集，生成 patch 不需要 YAML 依赖。实测 `--dump-config` 能读、boot 能用。
- 钉定版本：**0.2.1-alpha.2**（npm `alpha` tag；`latest` 是 0.2.0-rc.2）。选它是因为 REFS 源码就是这一版，排查时能对着读。

实测（2026-10-14，Windows）：boot + `initialize` 约 2.5 s；经 OpenCode Go 的 `deepseek-v4.1-flash` 跑通
「调用 pwsh → 结果 → completed」；`personaSuffix` 进了 system prompt（模型自己提到了标记文本）；
`session-log-deepseek` 关闭生效；缓存命中（第一步 6445 输入里 5760 命中）。
真实事件的字段都在 `data` 里（`{type, seq, time, data:{…}}`），工具结果是 `data.message.toolCallId`，
退出码以 `[exit code: N]` 标记写在结果正文里 —— M1 的投影按顶层字段读，必须改。

### 9.2b 供应商管理（参照 OpenCode）

形状照抄 OpenCode，让用户的 `opencode.json` 供应商块可以原样搬过来：

- **配置**：`provider.<id> = { name, npm | api, options: { baseURL, apiKey, headers }, models: { <id>: { name } } }`；
  模型写作 `<provider>/<model>`。`npm` 只用来推断协议（`@ai-sdk/openai-compatible` → `openai-completions`，
  `@ai-sdk/anthropic` → `anthropic-messages`，`@ai-sdk/openai` → `openai-responses`，`@ai-sdk/google` → `google-generative-ai`），
  不加载任何 npm 包；也可以直接写 `api`。
- **内置**：`deepseek`（官方，走 DSH 原生 Messages 适配 `deepseek-official`）与 `opencode-go`（Anthropic Messages，
  自动带每任务的 `x-opencode-session` 头 —— 实测缺这个头网关返回 400 `MissingSessionID`）。
- **凭据**：与配置分开，存在用户数据目录的 `arcdeck/auth.json`（与 OpenCode `auth.json` 同形：`{ "<id>": { "type": "api", "key": "…" } }`）。
  `arcdeck auth set <id>` 从 stdin 读、`arcdeck auth ls` 只列名字、`arcdeck auth import-opencode` 搬 API 类凭据。
  `options.apiKey` 也接受 `{env:VAR}` / `{file:path}`。
- **传给 DSH 的方式**：密钥只以环境变量 `ARCDECK_KEY_<ID>` 传给运行时子进程，patch 里只写变量名（`apiKeyEnv`）。
  密钥不落任务状态目录、不进日志。

当前 workers 的缺省路由：`opencode-go/deepseek-v4.1-flash`（用户 2026-10-14 指定）。本机两份 DeepSeek 官方 key
实测都是 402 余额不足，所以官方路由（DSH_FINDINGS D5 的「DS 优势只在官方路由」）暂时用不上；
OpenCode Go 走的也是 Anthropic Messages 协议，缓存命中实测有效，但它是网关，E1（DSML 是否为 0）要在它上面单独记数。

### 9.3 安全、依赖与性能

- Arc Deck 的授权上限、任务范围与 Agent 策略取交集；子 Agent、Skill、插件和 project instructions 不得扩大权限。执行 guard 与 OS 沙箱的保证范围分别记录。
- 先验证路径、runtime、shell 与必要 sidecar。若 DSH 需要的 shell 缺失（例如 Windows 缺 `pwsh`），明确报告依赖错误，不悄悄换解释器造成行为差异。
- 只绑定本地服务并使用可审计认证/IPC 权限；诊断脱敏，不把 provider 凭据传给无关工具，不默认采集/上传任务内容。
- 先记录单任务启动、内存与长消息渲染基线；未知性能不编造数字承诺。事件队列、输出和内存保留必须有界；空闲不持续重绘，流式更新做合并但不丢工具/终止事件。
- 低成本的动机来自用户对 DSH/DeepSeek 的选择；实际 token/cache/费用按 provider 证据测量，不声称 Rust 或 Echo 本身使模型调用变便宜。

## 10. 编码顺序与验收

### 10.1 里程碑

| 阶段 | 编码交付 | 退出条件 |
|---|---|---|
| M0：需求基线 | 本文、README/AGENTS/旧工作流入口、参考与许可边界 | ✅ 已完成：产品要求/选定方案/未决项分开，旧双 GUI 路线不再支配新开发 |
| M1：离线 worker 骨架 | core/adapter/CLI 最小 crate；任务/结果/JSONL schema；Echo 定义解析；假 SDK 子进程 | ✅ **已实测**：127 个离线测试通过（成功/失败/受阻/EOF/坏帧/分帧中文/错 ID/结果写失败/超时/取消），全程无模型请求 |
| **M2a：真实 DSH 单任务**（焦点） | 钉 DSH 版本（Windows 与 Arch 同版）；`runtime.kind = dsh-sdk`；per-task `DSH_HOME` 与 profile 生成（§9.2）；新版本事件投影；Echo + `--preamble` 进 system prompt；usage 采集；二进制改名 `arcdeck`，无参数给出 TUI 未实现提示 | 一件真实的 Ether 风格任务在 Windows 上经 `deepseek-official` 跑到 `completed`，验收命令的退出码由 Arc Deck 采集；内核失败（错模型名，参 P3）被识别为 `failed`；`session-log-deepseek` 的实际开关在请求里核对过。**会消耗模型配额，跑前经用户同意** |
| **M2b：分离运行与探活**（焦点） | 拆出 `arc-deck-run`；状态目录与 `status.json`；`--detach`；`status/wait/report/logs/cancel/ls`；读者侧健康判定；stalled 自救与有界续跑；进程树收尾（Windows Job Object、Unix 进程组） | 离线故障注入全部过：假内核挂住（→ stalled → 续跑）、杀监工（→ dead/unknown）、杀 runtime、取消、pid 复用、状态文件写到一半。阈值判定有时钟可注入的单测；**一行输出的字节数有测试上限**。**2026-10-14 补**：`pid` + `pid_started_at` 已实装（Linux 走 `/proc`，其余平台刻意降级为「不知道」），`kill -9` 监工后 `status` 1 秒内报 `dead` —— 真实运行时实测过 |
| **M2c：接进 Ether**（焦点） | Markdown 任务书、`--accept`、`--worktree`、env 模板、`allow_commit`；**Arch 构建与运行**（2026-10-14 修订，原为 Debian）；`examples/ether/` 派工示例 | 一批真实 Ether 任务在 Windows 与 Arch 各派至少一件，调度者**只用 `status` 与 `report`** 完成跟踪与是否细看的判断；与旧 runner 的结果并排对照一次。**Ether 侧脚本改动在 Ether 仓做** |
| M2d：对照门 | DSH_FINDINGS §5 的 E1–E3 | 有样本量、有数据地回答：官方路由 DSML 是否为 0；`sdk` vs `sdk-minimal` 谁做缺省；非 DS 是否需要第二内核（预期否） |
| M3：Agent 与权限实装 | Echo/Build/Plan 组合矩阵在执行层生效；工具映射与 schema；Plan 写负例被拒 | §5 的契约有真实内核下的负例；提示词不代替权限 |
| M4：TUI 主流程 | `arc-deck-tui`：Ratatui 渲染、OpenCode token/布局、输入/action、Remote 与共用投影；`arcdeck` 无参数进入 | §7 夹具、真实终端与功能验收；历史/stream/取消/失败一致；TUI 里发起的任务与 headless 同一套状态目录 |
| M5：本地服务与稳定 | `arcdeck serve` 与版本化 IPC；受控并发（单机并发上限由后端守）；macOS；分发与许可 | 平台矩阵实际验收；无孤儿进程/越权/重复写入；不把断点恢复等同 exactly-once |
| M6：桌面 GUI（冻结中） | Windows：完整信任宿主验证 + IPC 接线；Linux：Kanesumi 接同一 IPC | §8 运行通过且任务状态与 TUI 一致；宿主验证失败则停在决策，不冒充接线完成 |

**首个可用版本 = M2a + M2b + M2c**：「默认 Echo + 单任务 worker + 分离运行 + 一行探活 + 有证据的紧凑回报」，在 Ether 的两台机器上可用。
M2a 与 M2b 可以部分并行（M2b 的探活与故障注入全部能用假内核离线做）；M2c 依赖两者。M2d 在 M2c 之后用真实批次做，M3 起不阻塞 headless 的日常使用。
GUI 的宿主探针不在当前焦点内，不安排。

### 10.2 第一份编码任务书（M1，已实现）

范围固定为 M1：建立最小 Rust crate 与公开 schema；实现任务解析/默认 Echo/配置拒绝；SDK 行传输与关联；事件投影与结果输出；假 SDK 对端模拟收尾与错误；取消/超时使用本地可控子进程验证。不要同时做完整工具生态、Remote、TUI、GUI、旧日志迁移或重新生成参考仓。

已落地：`crates/arc-deck-core/`、`crates/arc-deck-dsh/`、`crates/arc-deck-cli/`、`agents/`、`prompts/`、`schemas/`、`tests/fixtures/`，以及 Cargo workspace 成员。公开契约的单一事实来源是 Rust 类型；`schemas/*.example.json` 是**示例**，由 `crates/arc-deck-core/tests/schema_contract.rs` 解析并重新序列化来钉住字段名（参 `schemas/README.md`）。

验收命令（实测通过；`--features arc-deck-cli/test-runtime` 是必需的，夹具与隐藏子命令都在那个 feature 后面）：

```powershell
# 全部离线测试：core 56 + dsh 21 + cli 20 + cli 端到端 23 + 契约 7 = 127 个
cargo test -p arc-deck-core -p arc-deck-dsh -p arc-deck-cli `
    --features arc-deck-cli/test-runtime --offline

# 端到端离线验收：退出码 0，结果 status=completed、验收 outcome=passed
cargo run -p arc-deck-cli --features test-runtime -- worker run `
    --config tests/fixtures/worker-offline.json `
    --task tests/fixtures/worker-task.json `
    --format jsonl --result result.json
```

两条注意，都是实测踩出来的：

- **重复跑要先清状态目录。** 同一 `task_id` 在同一个 `state_root` 下**不会重跑**（第二次以 `TASK_ALREADY_SEEN` 退出 64）。要重跑就换 `task_id`，或删掉 `target/arc-deck-worker-state`。
- **相对路径的基准是文件所在目录，不是当前工作目录。** 配置与任务书里的 `../..` 相对各自文件解析；这条路径曾经因为基准没锚成绝对路径而失败过（参 `paths::absolutize` 的注释）。

启动配置固定为 §6.1 的 `fixture-sdk`，并且只在 `test-runtime` feature 下可用；发布构建里出现其他 runtime 类型一律拒绝，不回退真实 DSH。离线负例必须真的触发拒绝/失败；假模型/假协议证明程序行为，不证明 Echo 在真实模型上的风格。

M1 交付的离线覆盖（每一项都有对应测试）：成功 / 内核失败（含 `finish.reason.failure` 原文）/ 受阻 / 人工验收 pending / 缺命令证据 / 运行时提前退出 / stdout 坏帧 / 错位响应 id / 中文跨分帧 / 墙钟超时 / 取消文件 / 状态文件残留 / 结果写失败 / 重复 task_id / 越权 Agent。

### 10.2a 下一份编码任务书：M2a 的开工顺序

1. **钉版本**：选定 DSH 版本，Windows 与 Arch 装同一版；记下安装来源与 `dsh --version`。核对 DSH_FINDINGS §2 的机制在该版本里存在（Messages 协议、`systemPromptUpdate`、`session-log-deepseek` 的配置位置）。不存在的写进 FINDINGS，不假设。
2. **离线先行**：per-task `DSH_HOME` 与 profile patch 的生成是纯函数，先用快照测试钉住产物（不起内核）。
3. **最小真实链路**：`dsh-sdk` runtime → `initialize`（路由由 initialize 校验，参 §9.1）→ `session/prompt` → 消费 `session.event` → `session/wait` → 投影 → 结果。先跑一个只读的小任务（如「列出仓库顶层目录并总结」），再跑一个带 `--accept` 的写任务。
4. **改名与无参数行为**：`[[bin]] name = "arcdeck"`；无参数打印 TUI 未实现提示与 headless 用法、退出 64；`worker run` 保留为别名。
5. **跑真实任务前停下来问用户**：会消耗模型配额（§10.3）。

不做：`--detach` 与状态目录（M2b）、worktree（M2c）、Plan 的执行层限制（M3）、TUI。

### 10.2b 2026-10-14 落地的 headless（第一个可用版本）

**能用了**：`arcdeck run <任务书.md> --worktree <分支> --accept <命令> --detach`，之后 `arcdeck status/wait/report`。

| 项 | 状态 | 证据 |
|---|---|---|
| 私有钉版本的 DSH 运行时（§9.2a） | ✅ Windows 实测 | `arcdeck runtime install` 装 0.2.1-alpha.2 到 `%LOCALAPPDATA%\arcdeck\runtime\`，约 1 分钟 |
| 供应商管理（§9.2b） | ✅ opencode-go 实测；deepseek 官方只到「请求发出、402 余额不足」 | `auth.json` 同 OpenCode 形状；密钥只经环境变量进运行时 |
| 每任务 `DSH_HOME` + patch（session-log 关、persona、路由） | ✅ 实测 | `--dump-config` 核对；模型自己提到了 persona 里的标记文本 |
| 真实事件投影（`data` 信封、`toolCallId`、`[exit code: N]` 只写非零） | ✅ 实测后修正，有回归测试 | 不修时，成功的验收命令全被判「无法判定」 |
| 前台 run：真实任务 → completed，验收按执行记录 passed | ✅ 实测（smoke1，32 s，缓存命中 86400 / 88304） | `arcdeck report smoke1` |
| `--detach` + `status` / `wait` / `report` | ✅ 实测（smoke2、smoke3） | 一行状态约 90 字；退出码按 §6.5.3 |
| `--worktree`（建/复用分支、子模块、git 事实采集） | ✅ 实测（Windows + **Arch**） | Arch 真实模型：真提交 `132627b`、验收 `git log --oneline -1` 通过、`report` 的 git 事实取自 git 不是模型自报 |
| `workspace-write` 下 worktree 里的 git 全失败 | ✅ 实测（预期内） | worktree 的 `.git` 指向主仓、主仓在沙箱外 → 每条 git 命令退出 128。**现在派工时就会警告**（§6.6） |
| 卡死 → 杀进程树 → 续跑 → 用尽判 `STALLED` | ✅ 离线故障注入测试 | `stalled_run_is_restarted_then_fails_with_stalled_when_attempts_run_out` |
| 读者侧健康（dead / stalled / busy） | ✅ 单测 + **真实运行实测** | `kill -9` 监工后 `status` 1 秒内报 `dead`（退出码 5），不再等 45 秒 |
| 监工被杀后的**孤儿运行时** | ✅ 修复 + 实测 | 实测：`kill -9` 监工后 DSH 运行时仍活着、无人回收。现在 `arcdeck cancel` 在监工确认不在时按 `runtime_pid` 收整棵进程树（Unix 整组 SIGKILL + 有界确认） |
| 正常取消（监工还活着） | ✅ 真实运行实测 | Arch 真人任务：`cancel` → `cancelled done CANCELLED: 主控请求取消`，退出码 3，无 node 残留 |
| Debian → **Arch（本机）** | ⚠️ 部分验证 | 2026-10-14 修订：目标改为本机 Arch。已实测：发布构建通过、离线套件 166/166、夹具端到端 `completed` + 验收 `passed`、契约测试在 Arch 上全绿（改前因写死盘符路径挂 3 个）。**未做**：真实 DSH 运行（本机未装运行时、无密钥）。`cfg(unix)` 的进程组收尾由离线套件在 Linux 上真跑到，不再是「只做人工核对」 |
| Ether 侧派工脚本 | ⚠️ 只有示例 `examples/ether/ad.sh` | 改 Ether 的 `tools/dispatch/` 与 DISPATCHER 在 Ether 仓做 |

已知限制（有意留到后面，不是遗漏）：

- 监工逻辑仍在 `arc-deck-cli/src/worker.rs` 里，`arc-deck-run` crate 没拆（§4.1）。等 TUI 要复用时再拆。
- `status` 判「进程已死」只靠心跳过期（45 s），没有比对 pid 与启动时间。
- 没有 `--accept` 的任务，`completed` 只表示内核正常结束 —— 实测出现过「提交被拒但 completed」，回报的事实栏能看出来（commits 0、未提交 1）。**派 Ether 任务时总是带 `--accept`。**
- 续跑开的是新 DSH 会话（SDK 没有历史恢复），靠工作区里的成果接着做，不是 exactly-once。

### 10.3 每次交付纪律

一个逻辑单元对应清晰 diff；验证按源码核对、编译/离线测试、真实 runtime、真实界面分别报告。修改接口同步 schema、模型可见说明、夹具与文档；未实现/不支持部分显式记录。未获明确要求不提交、推送、安装应用、迁移数据或执行会消耗模型配额的验收。

## 11. 未决事项与验证边界

| 项目 | 当前状态 | 后续动作 |
|---|---|---|
| 用户对本文范围的审阅 | 0.2 方向已定（§0）；细节待审阅 | 技术细节按本基线推进 |
| **`session-log-deepseek` 开还是关** | 待用户裁定；裁定前 Arc Deck 生成的 profile **关闭** | 它把会话日志增量随请求发给 DeepSeek 官方 API（DSH 默认开）。worker 跑的是 Ether 源码，这是数据外发决定，归用户 |
| DSH 钉哪个版本 | 参考源码 0.2.1-alpha.2，本机安装 0.1.7-rc.2 | M2a 第一步；Windows 与 Arch 同版 |
| worker 缺省工具组合（`sdk` 完整工具集 vs `sdk-minimal` RL 两工具） | 未测 | M2d E2 |
| 非 DS 模型是否需要第二内核 | 按 D4 不建；未测 | M2d E3；DSH 明显更差才重新讨论 |
| headless 的缺省阈值（心跳 15 s、模型空闲 300 s、续跑 3 次） | 初值，来自 DSH 默认值与 Ether runner 经验 | M2c 用真实批次校准，改了写明依据 |
| 失败分类表（§6.5.6） | 按子串匹配写，输入只有 `finish.reason.failure` 一条通道；**限流/额度/网络的真实原文没攒到**，只有路由错有实测原文 | M2c/M2d 攒真实失败原文后收紧；在此之前认不出的一律落兜底且不可重派 |
| TUI 基线取「v1」的哪一支 | 已按 `v1.19.0:packages/tui/src` 记录（§7.1），待确认 | 该检出里「v1」有三种含义（TS TUI / Go TUI / 配置 v1）。若用户指的是 Go TUI，需要重新评估；确认后把基线提交号钉进 §3.1 |
| v1 TUI 的移植量 | 187 个 TSX 文件，**未评估工时** | M4 开工前先做一次组件清单与工作量评估，把结论报出来；移植量不可接受时由用户决定降级方案 |
| OpenCode 视觉迁移的满意度 | 目标已明确，效果未验证 | M4 提供同终端对照，由用户判断视觉；不先要求设计另一套主题 |
| Echo 的模型行为与语言效果 | 定义来源已读，实际请求与输出未验 | M2a 起做固定任务样例和人工评审；不把「提示词有规则」当行为保证 |
| DSH 新版本兼容与权限 | 源码依据已核对，运行未验 | M1 离线 → M2 隔离运行；正式钉版本前完成矩阵 |
| WinUI 2 完整信任宿主 | 未验证；冻结（D6） | M6 时做小型 XAML Islands 探针；不预先替用户选 WinUI 3 |
| 所有平台的运行/取消 | 未验证 | 各平台实际跑进程树/输入/终端测试；当前 Windows 环境不代表其他平台 |
| Claude Code 参考的复制授权 | 根许可证不是开放源码授权 | 本期只参考功能事实并独立实现；若将来要复制，先核清具体文件与条款 |
| 旧实现/数据删除或迁移 | 未授权，不进行 | 保留现状，另立经用户确认的任务 |

0.2 版（2026-10-14）的新增证据仅为 DSH 源码静态阅读（DSH_FINDINGS）、Ether 派工脚本与手册的阅读，以及本机 `dsh --version`；没有运行 DSH、调用模型或做任何对照实验。下一步：按 §10.2a 开始 M2a。
