# M3 · Arc Deck 授权策略落到 DSH 执行层（映射表与缺口）

日期：2026-10-14。被钉住的运行时：**DSH 0.2.1-alpha.2**（`userconfig::PINNED_DSH_VERSION`），
profile `sdk`（= `@deepseek-ai/dsh-base` + `@deepseek-ai/dsh-sdk-app`）。
所有 DSH 出处都指 `REFS/deepseek-harness/` 下的源码，路径相对该目录；本机没有跑过的
结论一律标注「**未实测**」。

要解决的问题（`docs/CORE_HEADLESS_AUDIT_2026-10-14.md` §2）：`ExecutionPlan::resolve_with_ceiling`
算出的 `EffectivePolicy` 原先只进结果留档，真实 DSH 用的仍是启动配置里的沙箱档 ——
把 Agent 换成 Plan、或任务书收紧为不许 shell，**真实 DSH 照样能写文件、跑 shell**。
本段把这份策略变成 DSH 的配置行，并把**表达不了的部分**明确成拒绝启动或留档。

## 1 · 结论

- **`readonly` / Plan 的「不能写、不能跑命令」现在是执行层的**：沙箱档落到 `read-only`，
  且 bash / pwsh / jobs / workflow / PTC 六行工具被摘掉。
- **`shell = false` 是执行层的**：那六行工具不存在，不靠提示词、不靠命令前缀匹配。
- **`workspace` / `full` 的写入边界是 OS 沙箱给的**：`workspace-write` = 会话 cwd（workspace 根）
  + 平台临时区；`danger-full-access` 不设防。
- **读范围、路径收窄、提交 / 推送做不到**，且**没有任何一项被静默放宽**：
  收窄的读写路径与「不许联网但 shell 还开着」→ **拒绝启动**（退出码 64）；
  提交 / 推送 → **留档并逐条报进 `result.unverified` 与诊断事件**（理由见 §3）。
- 子 Agent / Skill / 插件 / project instructions / MCP 的那几条路径逐条核对过，**没有发现
  模型可达的提权路径**；但有三条「不是模型直接可达、却在组合里活着」的边界写在 §4。

## 2 · 映射表

保证范围只有四种说法，全文一致：

| 记号 | 含义 |
|---|---|
| **OS 沙箱** | 由 DSH 的沙箱后端在进程外强制（Linux bwrap/Landlock、macOS Seatbelt、Windows 受限令牌）；**不是**「提示模型别这么做」 |
| **工具管线** | 工具行没挂上 → 工具不存在 → 调用它的那一步直接失败（`UNKNOWN_TOOL`），执行层没有可绕的余地 |
| **可见性** | 只影响模型看得见什么 / 调用被策略检查拦下，**不是**内核边界 |
| **做不到** | DSH 没有对应的表达；处理方式见 §3 |

### 2.1 授权级别 → DSH 沙箱档（唯一出处：`crates/arc-deck-core/src/authz.rs`）

| Arc Deck 级别 | 行 id | 包 | 配置键 | 取值 | 保证范围 |
|---|---|---|---|---|---|
| `readonly` | `sandbox-policy` | `@deepseek-ai/dsh-sandbox-policy` | `mode` | `read-only` | **OS 沙箱** |
| `workspace` | `sandbox-policy` | `@deepseek-ai/dsh-sandbox-policy` | `mode` | `workspace-write` | **OS 沙箱** |
| `full` | `sandbox-policy` | `@deepseek-ai/dsh-sandbox-policy` | `mode` | `danger-full-access` | 不设防（用户显式给的） |
| `confirm` | — | — | — | — | 不进执行层：headless 在 `run` 里直接拒绝（需要应答者） |

档名由 `Permission::sandbox()` 一个函数给出，反向由 `Permission::from_sandbox()` 解析 ——
两个方向写在同一个 `impl` 里，「映射只有一份」才是真的。

**最终的档是「启动级别 ∩ 策略上限」，不是启动配置那一行**。上限只由一个事实决定：
策略还允许写吗。不许写（`write_paths` 为空 —— Plan 模式与 `readonly` 档都会走到这里）→
一定是 `read-only`；允许写 → 保持启动级别（`full` 不被降档，worktree 里的 `git status`
本来就靠它）。见 `authz.rs` 的 `EffectivePolicy::execution_guard`。

出处：

- 沙箱档的键与默认值：`packages/bundle/base/cordis.patch.yml:238-242`
  （`- id: sandbox-policy` / `name: '@deepseek-ai/dsh-sandbox-policy'` /
  `mode: !!js process.env.DSH_PERMISSION_MODE ?? 'workspace-write'` /
  `workspaceRoot: !!js process.cwd()`）。
- `mode` 是会话级可覆盖的部署默认值，**会话覆盖优先**：
  `packages/sandbox/sandbox-policy/src/index.ts:164-171`
  （`mode: request.mode ?? (session === undefined ? undefined : this.overrideOf(session)) ?? this.defaultMode`）。
- 供应商自己的配置模式：`mode` 的 schema 默认是**最严**的 `read-only`
  （`packages/sandbox/sandbox-policy/src/index.ts:113`），`workspaceRoot` 必须绝对（`:37`）。
- 写边界就是会话 cwd（**不可变**）：同文件 `:168`（`workspaceRoot: resolveWorkspaceRoot(session?.header.cwd ?? this.workspaceRoot)`）；
  `working_directory` 工具改的是「有效工作目录」，header 里的项目根不动 ——
  `packages/session/working-directory/src/index.ts:35`（`the immutable Session header retains the original project`）。
  也就是说**改工作目录不能把写边界挪出 workspace**。
- 写根集合：`packages/sandbox/sandbox/src/roots.ts:52-54`
  （`workspace-write` = workspaceRoot + `/tmp` + `os.tmpdir()`；**`read-only` 返回空数组**）。
- 三档在 system prompt 里的原话（写进去的是**描述**，不是保证）：
  `packages/sandbox/sandbox-policy/src/index.ts:42-56`。

### 2.2 `EffectivePolicy` 逐字段

| 字段 | 行 id（包） | 配置键 / 取值 | 保证范围 | 出处 / 缺口 |
|---|---|---|---|---|
| `mode` | `plan-mode`（`@deepseek-ai/dsh-plan-mode`） | `disabled: true` | **工具管线** | 同 `packages/bundle/base/cordis.patch.yml:327-341`；§2.4 |
| `profile` | — | — | 只做策略上限，不进 DSH 配置 | `authz.rs::Profile` |
| `read_paths` | — | 无对应键 | **做不到** | §3；`packages/fs/fs-sandbox/src/index.ts:5-8`（`Reads pass through untouched: every mode permits reading`） |
| `write_paths` | `sandbox-policy`（`@deepseek-ai/dsh-sandbox-policy`） | `mode` ∈ {`read-only`, `workspace-write`, `danger-full-access`}；只能表达「全 workspace 可写」/「完全不可写」 | **OS 沙箱** | `roots.ts:52-54`；§3 |
| `shell` | `tool-bash` / `tool-pwsh` / `tool-jobs` / `tool-workflow` / `ptc-runtime` / `workflow-ptc` | `disabled: true` | **工具管线** | §2.3 |
| `network` | `tool-web`（`@deepseek-ai/dsh-tool-web`） | `disabled: true` | **工具管线** | `packages/bundle/base/cordis.patch.yml:474-478`；§3 |
| `commit` | — | 无对应键 | **做不到** | §3 |
| `push` | — | 无对应键 | **做不到** | §3 |
| `agent` | — | 只影响 system prompt 与 Agent 档位上限 | 可见性（提示词） | `PRODUCT_PLAN §5.3` |

### 2.3 工具行开关

`shell = false` 时摘掉这六行（`render_patch` 里 `disabled: true`）：

| 行 id | 包 | 它给模型的能力 | 出处 |
|---|---|---|---|
| `tool-bash` | `dsh-tool-bash` | `bash`（`bash -c`） | `packages/bundle/base/cordis.patch.yml:276-278`；工具名 `packages/shell/tool-bash/src/index.ts:368` |
| `tool-pwsh` | `dsh-tool-pwsh` | `pwsh`（`pwsh -Command`） | 同 `:280-282`；`packages/shell/tool-pwsh/src/index.ts:386` |
| `tool-jobs` | `dsh-tool-jobs` | `job_output` / `job_list` / `job_kill` | 同 `:284-285`；`packages/jobs/tool-jobs/src/index.ts:311,352,372` |
| `tool-workflow` | `dsh-tool-workflow` | `workflow`：跑模型写的 **JavaScript**（新 Node 进程） | 同 `:400-401`；`packages/workflow/tool-workflow/src/index.ts:329` |
| `ptc-runtime` | `dsh-ptc-runtime-node` | 上面那个 JS 的执行面 | 同 `:392-393` |
| `workflow-ptc` | `dsh-workflow-ptc` | 上一行那个 JS 的执行引擎（两行是同一件事的两半） | 同 `:395-399` |

为什么要连 workflow 一起摘：它是**真的代码执行**，不是只调 `agent()` 的编排 API ——
脚本在 vm 里编译执行，`packages/workflow/workflow-ptc/README.md:59,158`
（`the VM is not a security boundary … OS policy governs code that reaches Node`），
且 `packages/workflow/workflow-ptc/src/host.ts:287-294` 把会话的 `sandboxPolicy` 传进 PTC，
由 `packages/ptc-runtime/ptc-runtime-node/src/index.ts:228` 落实成 OS confinement。
所以它**受文件沙箱管**，但它是「跑代码」这条路，`shell = false` 时不该留着。

`network = false` 时摘掉 `tool-web`（`web_search` / `web_fetch`；
`packages/bundle/base/cordis.patch.yml:474-478`，工具名见
`packages/web/tool-web/src/search.ts:326`、`fetch.ts:457`）。底层的 provider 行
（`web` / `web-search-deepseek` / `web-fetch-http`，同文件 `:460-472`）留挂：
它们是服务与 provider，自己不发起请求（`packages/web/web/src/index.ts` 只注册 provider）。

**`tools` 行没有 allow / deny 列表**：它的配置只有 `mode`（`native` / `ptc`）与
`maxParallelSubCalls`（`packages/core/tools/src/index.ts:675-694`），
按 agent 的 `restrict()` 是**程序内调用**、不是配置项。所以「摘行」是这里唯一的配置级手段。
默认 `native`（`:853`），因此 `run_code` 不进工具表（`:1221-1222`）；这一条在 §4 复核。

### 2.4 Plan 模式

Arc Deck 的 Plan 由三件东西共同承担，**没有一件依赖 DSH 的 plan-mode**：

1. **沙箱 read-only**：`write_paths` 被清空（`authz.rs::resolve_with_ceiling` 的第三层），
   `execution_guard` 因此把档压到 `read-only`；文件工具的写入被 DSH 的文件栅栏拒
   （`packages/fs/fs-sandbox/src/index.ts:126-128`，报 `FS_SANDBOX_DENIED`）。
2. **摘掉六行 shell 工具**（§2.3）。
3. **提示词**（`agents/plan.md` + `prompts/plan-behavior.md`）—— 只作行为引导，不算隔离。

`plan-mode` 那一行**一律 `disabled: true`**：DSH 的 plan-mode 是**协作状态**，官方 README
写着 `It does not restrict the agent: every tool stays callable, so use sandbox mode and
approval prompts when you need enforced limits`（`packages/plan/plan-mode/README.md`），
它的出口工具 `exit_plan_mode` 要一个人工评审通道，而 headless 没有 ——
PRODUCT_PLAN §5.3 也明确「不得不经适配直接要求 headless 调用其人工评审工具」。

## 3 · 做不到的部分，以及 Arc Deck 怎么处理

**判定规则**（`authz.rs::execution_guard`）：能表达的就表达；表达不了时，
只在该项**没有任何替代表达、且收紧到更严的档会把任务整个废掉**时才留档 —— 其余一律**拒绝启动**。

| 做得到的 | 现在的处理 | 为什么这么选 |
|---|---|---|
| 读范围收窄（`read_paths` 不覆盖整个 workspace） | **拒绝启动**，`AUTH_GUARD_UNSUPPORTED` | DSH 对读取**完全不设边界**：三个档位都允许读任意绝对路径（`packages/fs/fs-sandbox/src/index.ts:7-8`）。跑了就等于把「只许读 crates/」放宽到整台机器 |
| 写范围收窄（`write_paths` 非空且不覆盖 workspace 根） | **拒绝启动** | `workspace-write` 的写根恒为 workspace 根 + 临时区，没有按路径列表收窄的表达（`roots.ts:52-54`） |
| `network = false` 但 shell 还开着 | **拒绝启动** | 唯一的网络开关是摘 web 两行；命令自己 `curl` 拦不住。这里**存在更严的可表达状态**（把 shell 也关掉），所以要求调用方去拿它，而不是照原样跑 |
| `commit = false` / `push = false` | **留档**：进 `result.unverified` + 一条 `AUTH_GUARD_GAP` 诊断事件；启动时 stderr 也打一行 | DSH **没有按 git 子命令区分权限的能力**（提交就是 shell 里的一条 `git commit`）。而 `allow_commit` 缺省是 `false`、shell 缺省是开 —— 拒绝启动会拒掉几乎每一次派工。**这是一个已知缺口，不是遗漏**：它就在结果里明文写着「这条没有被执行层拦住」 |

第三条的取舍单独说明：**「宁可拒绝启动」不等于「凡是表达不了就拒绝」**。
写范围收窄拒绝启动，是因为 Arc Deck 有能力给出更严的表达（不许写）却会废掉任务，
所以正确动作是把问题交回主控去改授权；而 commit 那条连更严的表达都不存在
（唯一的更严是「整个 shell 关掉」），拒绝就等于拒绝所有派工。

### 明确记下来的残留缺口

1. **提交 / 推送拦不住**（上表）。`m3b` 应当用一个真实的越权负例把它量出来：
   在一个**非 worktree** 的 git 仓里（`.git` 在 workspace 内）以 `workspace-write` 跑
   `git commit`，会成功。要真拦住，得写一个 DSH 工具 guard 插件（按参数判断，不是前缀匹配），
   本轮没做。
2. **读取不受限**（上表）。`readonly` 档的「不许写」是真的，但「不许读别处」不是 ——
   `agents/plan.md` 里那句「你被允许读取的范围仍由授权决定」目前只在
   `read_paths` 覆盖整个 workspace 时才成立。
3. **Windows 的文件沙箱是「部分强制」，且只强制写**：
   `packages/sandbox/sandbox-windows-acl/README.md` 的 Summary ——
   `workspace-write` 给 workspace 与私有临时目录写权限、`read-only` 两样都不给，
   机制是 `WRITE_RESTRICTED` 受限令牌 + capability SID 的 allow ACE + Low 完整性标签；
   **读取不在此列**（`WRITE_RESTRICTED intersects write accesses only`），
   且保证被明确记为部分（硬链接别名同一个文件对象、别的 AppContainer 工具 ACL 过的树读不到）。
   `sandbox-local` 侧同时记录「每次受限运行都会在 stderr 上自报部分强制」
   （`packages/sandbox/sandbox-local/src/index.ts:8-19,165-166`）。
4. **文件工具那一侧的栅栏不是内核边界**：`dsh-fs-sandbox` 的写入是全进程内的策略检查
   （`canonicalize-then-contain`），它自己的注释就写着这是
   `a policy check in TRUSTED code over a MODEL-CONTROLLED path, NOT a kernel boundary`
   （`packages/fs/fs-sandbox/src/index.ts:10-18`）。命令那条路才走 OS 沙箱。
5. **预设表被收窄之后，`DSH_HOME` 必须是每任务独有的空目录**（worker 就是这么做的：
   `<run_dir>/dsh-home`）。若复用一份带 settings 的真实 home，那里的
   `permission.defaultPreset` 可能指向我们不再保留的档名（例如 `danger-full-access`），
   而 `PermissionPresetService` 在构造时就 `resolve(defaultPreset)` —— 未知名字直接抛
   （`packages/interaction/permission-presets/src/index.ts:222-232`）。
   结局是**失败关闭**（部署默认的沙箱档仍然生效），但那个插件会激活失败 ——
   与 §5 live 对照实验里看到的是同一种形状。

## 4 · 子 Agent / Skill / 插件 / project instructions / MCP：会不会绕过

| 路径 | 结论 | 证据 |
|---|---|---|
| **子 Agent**（`subagent` / `subagent_fork`） | **不能提权**。委派工具的参数只有 cwd / description / prompt（+ 可选模型），**没有 sandbox / permission 参数**；子 Agent 只继承父会话**显式的那条** `sandbox/mode`，审批策略被钉成 `never` | `packages/subagent/tool-subagent/src/index.ts:231-263`；`packages/subagent/subagent/src/child-agent.ts:246-253`（`sandboxMode: …overrideOf(parent.session)` / `approvalPolicy: … 'never'`）、`:264-277`（以 `source: 'delegation'` 落进子会话日志） |
| 子 Agent 继承的是**覆盖值**，没有折进部署默认 | 父会话没有显式覆盖时，子 Agent 自己解析部署默认 —— 而部署默认就是我们的 `sandbox-policy`。两条路都落在同一个档上 | `packages/sandbox/sandbox-policy/src/index.ts:167`；本段 patch 又保证每个新会话都被预设钉出一条显式覆盖（§5） |
| **Skill** | **不能提权**。`skill` 工具只取文本（`packages/skill/tool-skill/src/index.ts:143,156`），技能没有工具授予 / 沙箱 / 审批字段，唯一的策略是 `invocation` 的两个布尔（`packages/skill/skill/src/index.ts:449,789`）。技能脚本要走什么工具，就走那条工具的授权 | `packages/skill/tool-skill/src/index.ts:143`（`ctx.skills.get`）、`:156`（只返回 `content`）；`packages/skill/skill/src/index.ts:449`（`invocation` 是唯一的策略字段） |
| 工作区可以放 Skill | 是。`.dsh/skills` 与 `.agents/skills` 会被扫（`packages/skill/skill-filesystem/src/index.ts:249-250`）—— 但那带的是**说明文本**，不带权限 | `packages/skill/skill-filesystem/src/index.ts:249-250`；`packages/skill/skill/src/index.ts:449`（技能自身没有工具/沙箱/审批字段） |
| **插件**（`plugin_manager`） | 组合里**没有这个工具**：`tool-plugin-manager` 在 base 就是 `disabled: true`（`packages/bundle/base/cordis.patch.yml:16-18`）。它本来是唯一能改 profile、装包（可跑安装脚本）的模型可见工具，且要求提到 `danger-full-access`（`packages/boot/plugin-manager/src/tools.ts:20-21,39-43`） | `packages/bundle/base/cordis.patch.yml:16-18`（`- id: tool-plugin-manager` / `disabled: true`）；工具本体 `packages/boot/plugin-manager/src/tools.ts:19-21,37-43` |
| **project instructions**（`AGENTS.md` / `CLAUDE.md`） | **不能改组合、不能设权限、不能加工具**：只作为 `user/message`（`source.kind = 'agent-instructions'`）注入文本（`packages/context/agent-instructions/src/index.ts:198`），包里没有 `tools.register` / `sandbox` / `approval` 的写入 | `packages/context/agent-instructions/src/index.ts:198`（`kind: 'agent-instructions'`，注入的是 `user/message`）；`packages/context/agent-instructions/src/config.ts:12-13`（候选文件名 `AGENTS.md` / `CLAUDE.md` 及其 `.local`） |
| **MCP** | `sdk` profile **不挂任何 MCP 服务器**：base 只有 `mcp-resources`（一个"消费"端，没有服务器时它连工具都不注册）。而真正起服务器的 `dsh-mcp-client` 不在 base 里，仓库里也**没有**项目级 MCP 配置文件读取器（无 `.mcp.json` / `.mcp.dsh.json` 这类入口，ACP 那条路要 `acp` profile） | `packages/bundle/base/cordis.patch.yml:480-481`；`packages/mcp/mcp-resources/src/index.ts:91` |
| MCP 的**潜在**问题（本轮不存在） | 一旦谁挂上 MCP 服务器，它的子进程**不走沙箱**：`packages/mcp/mcp-client/src/transport.ts:17-19,34-39`（`The MCP SDK owns the actual spawn`，只做环境变量清洗）。记在这里是因为「将来加 MCP」时这是第一个要处理的地方 | `packages/mcp/mcp-client/src/transport.ts:17-19`（`The MCP SDK owns the actual spawn, so this transport shares the scrub definition rather than the spawn path`）、`:34-39`（`StdioClientTransport` 直接收 command/args/env/cwd）；`packages/mcp/**` 里没有 `ctx.sandbox` / `sandboxPolicy` 的引用 |
| **PTC / workflow 代码** | 受**文件沙箱**管（会话 policy 被传进 PTC，`host.ts:291` → `ptc-runtime-node/src/index.ts:228`），但**不受工具注册表管**（vm 里的全局只有 `agent`/`parallel`/… 那几个钩子，`tools.restrict()` 管不到它）。`shell = false` 时我们直接摘掉这三行 | `packages/workflow/workflow-ptc/src/host.ts:287-294`（`sandboxPolicy: this.policy`）；`packages/ptc-runtime/ptc-runtime-node/src/index.ts:228`（`policy.mode === 'danger-full-access' ? undefined : await this.ctx.sandbox.confine(...)`）；`packages/workflow/workflow-ptc/README.md:59,158`；`packages/core/tools/src/index.ts:1117-1118`（`restrict()` 不能点名 `run_code`） |
| **运行时改权限** | `sdk` 的 JSON-RPC 面上**没有任何**命令 / 审批 / 沙箱方法（只有 `initialize` / `session/prompt` / `session/wait` / `session/working-directory/*` / `shutdown`，`packages/sdk/protocol/src/types.ts:139-145`）；`setSandboxMode` 的源码调用者只有预设服务（`packages/interaction/permission-presets/src/index.ts:410,441,451`），而 `/permission` 命令要一个命令提供者（同文件 `:255-276`），模型在 headless 里够不到它 | `packages/sdk/protocol/src/types.ts:139-145`（请求方法只有那六个）；`packages/interaction/permission-presets/src/index.ts:255`（`/permission` 要一个命令注册表）、`:272`（写入口） |
| **提权请求（escalation）** | **关死**。我们恒发 `approval.policy = never`，而 `never` 在 `ApprovalService.decide` 里**先于任何 answerer 分派**就返回 `rejected`（`packages/interaction/user-approval/src/index.ts:275`），于是 `approveEscalation` 抛出（`packages/sandbox/sandbox/src/escalation.ts:203`）。这正是 `never` 比 `ask` 强的地方：`ask` 依赖「组合里没有应答者」，而 `never` 是**策略层决定**的 | `packages/interaction/user-approval/src/index.ts:275`（`if (this.effectivePolicy(session) === 'never') return 'rejected'`，在任何 answerer 之前）；`packages/sandbox/sandbox/src/escalation.ts:203`（`case 'rejected': throw …`）、`:180-186`（没有 approver / agent 时更早抛出） |

## 5 · 实现与测试对照

实现（三处）：

| 文件 | 做了什么 |
|---|---|
| `crates/arc-deck-core/src/authz.rs` | `Permission::from_sandbox` / `stricter`；`ExecutionGuard`；`EffectivePolicy::execution_guard` —— **判定的唯一出处**，表达不了就在这里报 `AUTH_GUARD_UNSUPPORTED` |
| `crates/arc-deck-dsh/src/runtime.rs` | `PatchInput.guard`；`render_patch` 按映射表生成限制行；`runtime_env` 收 `Permission` |
| `crates/arc-deck-cli/src/worker.rs` | `dsh_runtime_command` 把 `plan.policy` 投影成 guard、把档名交给环境变量、把留档的缺口带进结果 |

生成的 patch 行（每次运行都有）：

| 行 id | 内容 | 作用 |
|---|---|---|
| `sandbox-policy` | `mode` = 交集后的档；`workspaceRoot` = 绝对 workspace | OS 沙箱边界 |
| `approval` | `policy: never` | 提权请求一律拒 |
| `permission` | 预设表只留当前那一格（名字用产品层级别名） | 会话被钉出显式覆盖；没有更宽的档可切 |
| `plan-mode` | `disabled: true` | 不引入没有应答者的评审通道 |
| 六行 shell 工具 | `disabled: true`（`shell = false` 时） | 工具不存在 |
| `tool-web` | `disabled: true`（`network = false` 时） | 同上 |

测试对照：

| 要求 | 测试 |
|---|---|
| 每种组合生成的 patch 断言到具体行与取值 | `arc-deck-dsh` 单测 `every_policy_combination_lands_on_concrete_rows`（9 格：Echo/Build/Plan × readonly/workspace/full × shell 开关 × 收窄路径，逐格核对档名、预设名、shell 行、web 行，并核对 `DSH_PERMISSION_MODE` 与 patch 逐字相同） |
| Plan / readonly 拒写拒跑 | 同上两条 Plan 格 + `m3_live_plan_mode_patch_is_tightened_to_read_only` |
| 收窄路径 → 拒绝启动 | `narrowed_write_paths_refuse_to_start`、`narrowed_read_paths_refuse_to_start`、`offline_profile_has_no_dsh_expression` |
| `network = false` + shell → 拒绝启动 | `network_off_with_shell_open_refuses_to_start` |
| commit / push 只留档 | `commit_and_push_limits_are_recorded_not_enforced` |
| 「只能收紧」的负例 | `task_cannot_widen_what_the_startup_level_allows`（提档 / 越界写 / 未授权提交 / Plan 请求写入）、`stricter_only_tightens`、`sandbox_names_round_trip` |
| live：DSH 接受 patch 且限制在 | `crates/arc-deck-cli/tests/m3_guard_live.rs` 的 5 条 `--dump-config` 用例（临时 `DSH_HOME`，不发 prompt） |
| live：组合真的挂得起来 | `m3_live_patch_boots_and_answers_initialize`（真起内核、只走 `initialize`，并断言 stderr 没有 `did not activate`） |
| live：断言有牙齿 | `m3_live_unknown_row_id_is_reported_on_stderr`（未知 row id → 退出码仍是 0、只有 stderr 警告）、`m3_live_missing_permission_row_breaks_the_permission_plugin`（去掉 `permission` 行 → 插件激活失败） |

## 6 · 验证分级

**已验证（本地实测过）**

- 生成的 patch 被真实运行时接受：`--dump-config` 退出码 0、stderr 无 `not found`，
  且 dump 里每一行都在 `patched by <我们的 patch>` 分组下，取值逐项核对（8 条 live 用例，本轮跑过）。
- 组合**真的挂得起来**：真起内核 + `initialize` 握手成功，且 stderr 无 `did not activate`。
- **对照实验**：未知 row id → 退出码仍 0（所以「stderr 干净」这条断言有牙齿）；
  去掉 `permission` 行 → `dsh: warning: 1 entry did not activate` +
  `permission: composed sandbox and approval defaults match no preset`（所以那一行不是装饰）。
  **这条实验纠正过一个想当然的说法**（原先写的是「漏了这行内核起不来」）：
  内核**照样起得来**，坏掉的是那个插件与会话日志里的授权钉点。
- 单元层：策略 → guard → patch 行的全矩阵；拒绝启动的每一条路径。

**推测（有源码支持，但本机没跑过 —— 未实测）**

- 「真实工具调用会被拒」：`read-only` 下 `write` / `edit` 被 `FS_SANDBOX_DENIED` 拒、
  命令的写被 Windows ACL 受限令牌拒。**这需要脚本化的假模型端点，属于 m3b**（见 §7）。
- 子 Agent / skill / MCP / workflow 那几条绕过路径的结论来自源码与官方 README，不是执行结果。
- 会话被钉出显式 `sandbox/mode`：来自 `permission-presets` 的 `pinInitialPermission` 源码
  （`:428-456`），我们没有在真实会话日志里看过它（要看会话日志就得发 prompt）。

**无法替你验证**

- Windows 沙箱的**部分强制**边界（硬链接、AppContainer ACL）在实际任务里的影响 ——
  上游自己就把它记成 partial，只能用真实越权尝试去量，而那会破坏本机文件。
- 一次 `danger-full-access`（`--permission full`）运行的**内部**行为：本段只断言了配置组合。
- 用户对「提交 / 推送只留档」这个取舍是否接受：这是产品决定，不是工程结论。

## 7 · m3b 应当覆盖的负例（每条：构造什么工具调用，期望 DSH 怎么拒）

前提：需要**脚本化的假模型端点**（或直接驱动 `ctx.tools` 的进程内驱动），
把「模型说要调什么」这件事变成可编程的输入。以下每条都写清期望的**可见拒绝形状**。

| # | 构造 | 期望 |
|---|---|---|
| 1 | `read-only` 会话（`--permission readonly`）调 `write`，目标 `<workspace>/x.txt` | `write` 返回 `FS_SANDBOX_DENIED`，正文含 `[sandbox: file access denied under read-only mode]`；文件不存在 |
| 2 | 同上调 `edit`，目标 `<workspace>` 里已存在的文件 | 同上，文件内容一字不变 |
| 3 | `read-only` + `shell=false`（Plan）：调 `bash` / `pwsh` | 工具**不存在**（`UNKNOWN_TOOL`），而不是「执行后被拒」—— 这是摘行的判据 |
| 4 | `workspace-write`：`write` 目标 `<workspace>/../outside.txt` | `FS_SANDBOX_DENIED`（`file access denied under workspace-write mode`） |
| 5 | `workspace-write`：`pwsh` 跑 `Set-Content ..\outside.txt x` | 命令失败，stderr 带沙箱拒绝（Windows ACL 受限令牌的失败形状要在这里记下来） |
| 6 | 第 1 条之后补一次提权重试：`write` 带 `sandbox_permissions: "danger-full-access"` + `justification` | 被拒，理由来自我们的 `never` 策略（`the user rejected escalating this operation to "danger-full-access"`），**不是** `no approval channel is available` —— 两者形状不同，说明 `never` 是策略决定而不是「恰好没人应答」 |
| 7 | `--permission full`：同一个 `write` 到 workspace 外 | **成功** —— 反向对照，证明前 6 条不是「什么都失败」的假阳性 |
| 8 | `commit = false`（缺省）+ 非 worktree 的 git 仓：`pwsh` 跑 `git commit` | **会成功** —— 这就是 §3 的缺口，量出来写进结果，不要假装它被拦住了 |
| 9 | `readonly` 会话跑 `read` 读 workspace 之外的文件（如 `C:\Windows\win.ini`） | **会成功** —— 第二处缺口（读范围），同样量出来 |
| 10 | Plan 会话调 `subagent`（`subagent` 行还在），子 Agent 里再试 `write` / `bash` | 子会话的 `sandbox/mode` 事件是 `read-only`（`source: 'delegation'`），写与命令同样被拒；且子 Agent 的工具表里没有 shell 类工具 |
| 11 | `--permission full`（`danger-full-access`）调 `workflow` | 允许执行；确认它**受文件沙箱**（`danger-full-access` 下不设防）—— 记下 PTC 的 `sandbox.enforcement` 报告 |
| 12 | `network = false` + shell 关：调 `web_search` | 工具不存在（`tool-web` 被摘）；再跑一次 `pwsh`（若有）确认也拿不到 |
| 13 | 未知 / 拼错的工具名 | `UNKNOWN_TOOL`；**不要**把它当成拒权证据（这属于第 3 条那一类，要分开记） |
| 14 | 每次都读一遍会话日志：断言 `permission/preset`、`sandbox/mode`、`approval/policy` 三条事件与本次 guard 逐字一致 | 这是「钉点真的写进去了」的唯一直接证据（本轮只从源码推断，见 §6） |
