# Arc Deck：产品需求与编码方向

版本：0.1 · 日期：2026-10-12 · 状态：需求与实施基线，等待用户审阅；**M1（离线 worker 骨架）已实现并实测**，M2 起尚未开工。除 M1 之外的内容都不是功能完成声明。

本文件是后续开发的产品方向入口。旧文档中「两条并列 GUI」与「新桌面宿主直接引用 C# 协议库」的路线由本文件取代；旧代码、历史实测和适用该代码的约束保留。未创建的目录、命令、schema 和测试在本文件中均为待实现规格。

## 1. 文档状态与需求基线

### 1.1 用户已明确的要求

| 编号 | 要求 | 判定方式 |
|---|---|---|
| R1 | 产品分为 TUI/headless 与 GUI 两部分 | 两部分可独立使用，前者不依赖窗口或 Windows GUI |
| R2 | TUI/headless 用 Rust，目标跨平台 | Windows、Linux、macOS 分别运行验收；只在 Windows 编译不算跨平台完成 |
| R3 | Windows GUI 以现有 WinUI 方案为准 | 保留 C# + WinUI 2 平台控件路线；不擅自改成 WinUI 3 或 WebView |
| R4 | headless 给 workers 用，外部主控可通过脚本派发任务书并收回结果 | 无 TUI 的完整任务提交、执行、终止和机器可读回报 |
| R5 | 引入 OpenCode 的 Build、Plan；Echo 成为默认 Agent | 新会话、未指定 Agent 的 worker 默认使用 Echo；Build/Plan 可显式选择 |
| R6 | 工具与 Agent 具有 Claude Code 那样明确、一致的定义 | 有角色、工具 schema、执行边界、权限反馈、验证和交付约定，不仅有一段语言风格提示词 |
| R7 | TUI 视觉效果迁移 OpenCode | 有对应组件、主题和终端运行对照；不是只借用几个颜色 |
| R8 | 骨架与底层基于 DSH，保持 DeepSeek 调用体验与成本优势 | 不另造 provider、agent loop 或模型计费系统；使用量与费用仅按真实数据呈现 |
| R9 | `REFS/` 只作本地参考，不进入 Git | 根目录 `/REFS/` 忽略；干净克隆与发布不依赖其存在 |

### 1.2 本文选定的技术方案

- 共用产品逻辑与 DSH 适配逐步收敛到 Rust；GUI 通过 Arc Deck 的本地 IPC 消费它，不再另写一套 DSH 客户端。
- worker 首版一任务一 runtime，使用 DSH SDK stdio；交互会话使用 Remote API。两者共用事件投影与 Agent 配置，不伪装成能力完全相同的传输。
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

- P0：Echo 基线、定义加载、明确的工具与权限契约、单任务 worker、结构化结果、失败与取消、离线测试。**（Echo 定义与 worker 骨架已落地，真实执行在 M2）**
- P1：OpenCode 风格 TUI 主流程、交互会话历史与实时事件、Windows GUI 宿主验证及接线。
- P2：有限并发、专用研究/审查 Agent、Skills 扩展、Hooks、恢复与分发完善。
- 首版不做：云端调度、集群、自动工作树合并、Claude Code API/插件的兼容承诺、完整 OpenCode 插件生态、自动提交推送、旧会话批量迁移。

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
Rust TUI ─────────────┐
Rust CLI / worker ───┼── Arc Deck core ── DSH adapter ── DSH runtime
Windows WinUI ─ IPC ─┘      │                  │
                     任务/配置/结果       SDK / Remote
```

### 4.1 计划目录与依赖方向

| 计划位置 | 职责 | 不负责 |
|---|---|---|
| `crates/arc-deck-core/` | 产品配置、任务与结果类型、Agent 解析、归一化事件/投影、策略要求 | 终端绘制、XAML、模型 HTTP 调用 |
| `crates/arc-deck-dsh/` | 运行时定位/启动/关闭、版本验证、SDK/Remote 协议、能力映射与 DSH 事件转换 | 自建模型循环、另一套文件/命令执行器 |
| `crates/arc-deck-cli/` | worker 命令、进程退出码、JSONL 输出；后续本地服务入口 | 单独维护 Agent 或投影实现 |
| `crates/arc-deck-tui/` | OpenCode 风格终端渲染、编辑、命令与交互 | 直接理解 DSH 原始事件与私有字段 |
| `agents/`、`prompts/` | 版本化角色、Build/Plan 行为、Echo 风格和交付规则 | 凭据、用户私有全局配置 |
| `schemas/`、`tests/fixtures/` | 公开契约与无凭据测试夹具 | 真实用户会话与 API key |
| `runtime/` | DSH profile/preset 与必要的薄插件 | fork 内核、复制上游执行循环 |
| `uwp/` | 现有 GUI 与宿主迁移验证 | 生产路径另起一套 DSH 协议栈 |

表中新增目录尚未创建，`uwp/` 已存在。core 不依赖 adapter；adapter、CLI 与 TUI 使用 core 类型。GUI 后续通过版本化 IPC 获取产品模型，不能把 SDK JSON-RPC 与 Remote 信封直接当 Arc Deck IPC。DSH 插件仅用于已有接口不能满足的 persona/策略接入，并保持独立测试。

### 4.2 现有代码的去向

- `kanesumi/`：暂停新增自绘 GUI 功能，保留源码、测试与测量记录；它的设计正典不支配新 TUI 或 WinUI。
- `uwp/`：保留现有页面、平台行为与 UIA/像素验收；当前仍为示例数据，不称作已接入新 core。
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

### 5.3 提示词与配置装配

保留 DSH 基础指导、真实工具说明及上下文管理，再组合 Arc Deck 工程规则、Agent 行为、模式、Echo 风格、工作区指令与任务。策略高于提示词；项目文件与任务文字不得扩大启动时授权。

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

### 6.1 首版调用方式

```powershell
# 待实现：这是公开接口方向，不是当前可执行命令。
arc-deck worker run --config worker.json --task task.json --format jsonl --result result.json --cancel-file cancel.json
# --task - 从 stdin 读取一份 JSON 任务书；不以 shell 解析 task 内容。
```

同步 worker 是 P0 接口。常驻服务、提交后返回再查询的异步接口属于后续阶段，不先要求 MCP；外部 Claude 可以直接运行命令并解析结果。

`--config` 为必需的主控启动配置，与任务书独立；不得自动查找 workspace 的配置、用户 Desktop home 或 PATH 上的 runtime。启动配置 v1 至少包括：

- `schema_version: 1`；`runtime` 的类型、显式入口、支持版本与部署 profile；独立的 `state_root`。
- `authorization` 的 workspace、读取/写入范围、shell 策略、联网、提交/推送开关，以及可用 Agent。没有指定的权限默认拒绝；未知策略 ID、缺失配置、workspace 不匹配与任务越权报结构化配置错误，退出 `64`，不启动内核。
- 环境、凭据与 Skills 的显式来源；日志只显示配置来源与有效授权，不显示秘密。

解析顺序为「启动配置授权上限 → Agent/模式限制 → 任务 constraints 收紧」，没有能以更后面的 allow 覆盖 deny 的顺序。首版内置 profile 至少有 `offline-test`、`workspace-readonly`、`workspace-development`；profile 规定能力模板，但具体路径和网络上限仍必须由启动配置提供。`workspace-development` 不自动授予提交、推送或整个磁盘访问。

M1 只支持 `runtime.kind=fixture-sdk` 与 `offline-test`，且仅在 `test-runtime` 构建 feature 下启用假 SDK 子进程；fixture 不读取真实环境凭据、不联网、不执行任务书里的命令。未启用该 feature 或出现其他 runtime 类型时拒绝，不回退真实 DSH。M2 才增加真实 `dsh-sdk` 配置。

### 6.2 任务书 v1

```json
{
  "schema_version": 1,
  "task_id": "fix-session-filter",
  "workspace": "C:/work/project",
  "agent": "echo",
  "mode": "build",
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

- 一任务一 DSH runtime；独立任务数据目录，明确 `DSH_HOME`、`DSH_AGENTS_HOME`、cwd、环境、凭据来源与技能来源，不复用 Desktop 真实 home。
- SDK 无单会话取消；按有界 shutdown → 进程树终止进行收尾，确认退出后才能称取消完成。Windows 与 Unix 分别验证，不能只杀父进程。
- 并发实施前使用独立工作树/目录与文件所有权；不自动合并，不覆盖用户原有变更，不清理未由本工具创建的数据。
- 无人值守 approval ask 必须明确拒绝/受阻；不为了通过测试默认启用 `danger-full-access` 或自动批准全部请求。

## 7. TUI 视觉与交互需求

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
| 变体 | 选中模型后按需进入变体选择（v1 的 `DialogVariant`），没有变体的模型直接跳过 |
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

`/agents`、`/models`、`/new`、`/sessions`、`/plan`、`/build`、`/cancel` 是首版产品命令方向。`/plan` 对 Echo 只切为 `echo + plan`、保留 Echo 风格；对 Build 切为 `plan + plan`；对 Plan 不变。`/build` 对 Echo 只切为 `echo + build`；对 Plan 切为 `build + build`；对 Build 不变。显式 Agent 选择取新 Agent 默认模式，未知 Agent 不静默回退。这些是用户动作，不是模型自行提权；每次仍受启动授权约束。

运行中的模式切换须先结束/取消当前活动并确认，再影响下一轮。终端无法区分某个组合键时给可用替代键，不声称完全兼容所有终端。

### 7.5 视觉与运行验收

- 在同一终端、字体与配色能力下做 OpenCode/Arc Deck 对照，至少 `80×24`、`120×30`、`160×40` 终端格；另测 `40×12` 的可用降级。
- 夹具覆盖 Home、长消息、流式、工具展开/折叠、失败、权限、命令面板、深/浅色，不只检查主屏。
- 自动检查 buffer 文本/样式、裁剪、命中与滚动；真实终端截图与键盘操作检查 Unicode、中文输入、鼠标、粘贴、resize 和退出后终端恢复。
- 自动化证明布局与行为；迁移后的视觉是否满意仍由用户判断。当前没有运行过参考 TUI 或新 TUI，静态源码不是视觉通过证据。

## 8. Windows GUI 需求

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

根 C# `DshVersion.cs` 记录 `0.1.2-rc.1`，当前参考 DSH 为 `0.2.1-alpha.2`。新版本在 initialize 校验路由，durable assistant 事件与实时通道也已变化；不能只改版本常量、照搬旧 `assistant/chunk` 或打包行规则。

- 验证运行时版本/产物来源并钉住支持组合；握手 `serverInfo.version=0.0.1` 不是可用的兼容判据。不支持版本明确拒绝或进入显式实验模式。
- 原始事件保留可诊断信息，产品正文过滤系统/内部簿记；用户来源按事件契约，不按内容长度猜。
- 分清 turn、step、attempt、权威消息与临时 stream；将工具配对、恢复与正文替换规则放在共用投影，界面不各自拼接。
- SDK/Remote/日志对账比较完整文本与结构，不只比较长度。负例覆盖等长不同正文、错误 ID、漏拆包装、失败 attempt 和中文跨分帧。
- `follow` 可能激活冷会话，不称其纯只读；测试只用隔离数据。分页/重连和实时 cursor 分开处理，不沿用旧版本未经验证的算法。
- 不自动打开、转换、覆盖或删除 Desktop 的真实会话。旧数据迁移另立带回滚与用户确认的任务。

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
| M2：真实执行与 Agent | 隔离 DSH 启动；profile/preset；Echo/Build/Plan；工具映射与权限执行；验收证据采集 | 真实工程任务可回报；Plan 写负例被拒；当前任务失败被识别；取消进程树已退；Echo 进入实际 system prompt；费用调用经授权 |
| M3：TUI 主流程 | Ratatui 渲染、OpenCode token/布局、输入/action、Remote 与共用投影 | §7 夹具、真实终端与功能验收；历史/stream/取消/失败一致；不复制第二套内核 |
| M4：Windows GUI | 宿主验证、IPC、现有页面数据接线 | §8 运行通过且任务状态与 TUI 一致；失败则停在宿主决策，不冒充接线完成 |
| M5：稳定与扩展 | 跨平台构建/运行、分发许可、受控并发、专用角色/Skills/Hooks、恢复 | 平台矩阵实际验收；无孤儿进程/越权/重复写入；不把断点恢复等同 exactly-once |

M4 的宿主探针可在 M2/M3 期间独立验证，但不得阻塞 worker；首个可用版本以 M2 的「默认 Echo + 单任务脚本 worker + 有证据回报」为目标，不等待完整 GUI。

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

### 10.3 每次交付纪律

一个逻辑单元对应清晰 diff；验证按源码核对、编译/离线测试、真实 runtime、真实界面分别报告。修改接口同步 schema、模型可见说明、夹具与文档；未实现/不支持部分显式记录。未获明确要求不提交、推送、安装应用、迁移数据或执行会消耗模型配额的验收。

## 11. 未决事项与验证边界

| 项目 | 当前状态 | 后续动作 |
|---|---|---|
| 用户对本文范围的审阅 | 待审阅 | 编码前确认对 Echo/工具/TUI 方向是否还需调整；技术细节按本基线推进 |
| TUI 基线取「v1」的哪一支 | 已按 `v1.19.0:packages/tui/src` 记录（§7.1），待确认 | 该检出里「v1」有三种含义（TS TUI / Go TUI / 配置 v1）。若用户指的是 Go TUI，需要重新评估；确认后把基线提交号钉进 §3.1 |
| v1 TUI 的移植量 | 187 个 TSX 文件，**未评估工时** | M3 开工前先做一次组件清单与工作量评估，把结论报出来；移植量不可接受时由用户决定降级方案 |
| OpenCode 视觉迁移的满意度 | 目标已明确，效果未验证 | M3 提供同终端对照，由用户判断视觉；不先要求设计另一套主题 |
| Echo 的模型行为与语言效果 | 定义来源已读，实际请求与输出未验 | M2 做固定任务样例和人工评审；不把「提示词有规则」当行为保证 |
| DSH 新版本兼容与权限 | 源码依据已核对，运行未验 | M1 离线 → M2 隔离运行；正式钉版本前完成矩阵 |
| WinUI 2 完整信任宿主 | 未验证 | 小型 XAML Islands 探针；不预先替用户选 WinUI 3 |
| 所有平台的运行/取消 | 未验证 | 各平台实际跑进程树/输入/终端测试；当前 Windows 环境不代表其他平台 |
| Claude Code 参考的复制授权 | 根许可证不是开放源码授权 | 本期只参考功能事实并独立实现；若将来要复制，先核清具体文件与条款 |
| 旧实现/数据删除或迁移 | 未授权，不进行 | 保留现状，另立经用户确认的任务 |

本轮证据仅为参考源码/许可证、Git 快照与忽略规则核对，没有运行 DSH/Claude/OpenCode、调用模型、验证跨平台或验证新 GUI。下一步：审阅本基线后，以 §10.2 的 M1 单任务 worker 离线骨架开始编码。
