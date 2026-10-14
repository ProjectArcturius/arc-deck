//! 授权交集。
//!
//! 三层收窄，顺序固定，**没有反向放宽**：
//!
//! ```text
//! 启动配置（上限） → Agent 定义档位（上限） → 任务书 constraints（收紧）
//! ```
//!
//! 之所以要把这件事写成一个纯函数而不是散在工作流里：一旦某处写成
//! 「后面的 allow 覆盖前面的 deny」，整套授权就只剩装饰作用。这里所有比较
//! 都是「取更小」，越权一律报错而不是静默裁剪 —— 静默裁剪会让主控以为
//! 请求被满足了。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::WorkerConfig;
use crate::error::{code, SpecError, SpecResult};
use crate::mode::{AgentId, AgentMode, Mode};
use crate::paths;
use crate::task::{Acceptance, TaskSpec};

/// 授权级别 —— **产品层的唯一说法**。
///
/// DSH 的沙箱档是实现细节，不该漏到命令行与界面上（用户 2026-10-14）。
/// 四个面各有自己的入口，但说的是同一件事：CLI 非工人用 `--permission` 显式划定、
/// 工人由任务书收紧、TUI 用 `/permission`、GUI 同 TUI 但默认不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// 只读工作区：不能写、不能跑命令。
    Readonly,
    /// 工作区内可读写、可执行。**TUI 的默认。**
    Workspace,
    /// 沙箱之外的全权。worktree 里提交必需（worktree 的 `.git` 指向主仓，在 workspace 之外）。
    Full,
    /// 与 `workspace` 同权限，但每次改动/执行前要确认。**GUI 的默认。**
    ///
    /// **headless 拒绝这一档**：它需要一个应答者，而无人值守时没有（§6.4）。
    /// 解析成功但在 `run` 里拒 —— 报错比静默降级成 `workspace` 好。
    Confirm,
}

impl Permission {
    pub fn parse(raw: &str) -> SpecResult<Self> {
        match raw {
            "readonly" => Ok(Self::Readonly),
            "workspace" => Ok(Self::Workspace),
            "full" => Ok(Self::Full),
            "confirm" => Ok(Self::Confirm),
            other => Err(SpecError::new(
                code::PROFILE_UNKNOWN,
                format!(
                    "未知授权级别 `{other}`；已知：readonly, workspace, full, confirm\
                     （confirm 只在交互面有意义，headless 会拒绝）"
                ),
            )),
        }
    }

    /// 映射到 DSH 的沙箱档。**这是实现细节，只在生成运行时环境时用。**
    pub const fn sandbox(self) -> &'static str {
        match self {
            Self::Readonly => "read-only",
            Self::Workspace | Self::Confirm => "workspace-write",
            Self::Full => "danger-full-access",
        }
    }

    /// 映射到 Arc Deck 的策略档（写法要能被 [`Profile::parse`] 认出来）。
    pub const fn profile(self) -> &'static str {
        match self {
            Self::Readonly => "workspace-readonly",
            _ => "workspace-development",
        }
    }

    /// 需要有人在旁边应答审批。headless 没有这样的人。
    pub const fn needs_responder(self) -> bool {
        matches!(self, Self::Confirm)
    }

    /// 从 DSH 沙箱档名**反解**回授权级别。
    ///
    /// 为什么需要反解：启动配置里只存了档名（§5.6 说档名是实现细节，只落启动配置），
    /// 而执行 guard 要按**级别**与策略取交集。两个方向写在同一个 `impl` 里，
    /// 「映射只有一份」才是真的 —— 拆到两个文件里迟早会漂移。
    ///
    /// `workspace-write` 只能反解成 [`Permission::Workspace`]：`confirm` 是交互面的
    /// 默认，headless 在 `run` 里就把它拒了，所以档名里不含这条信息。
    pub fn from_sandbox(raw: &str) -> SpecResult<Self> {
        match raw {
            "read-only" => Ok(Self::Readonly),
            "workspace-write" => Ok(Self::Workspace),
            "danger-full-access" => Ok(Self::Full),
            other => Err(SpecError::new(
                code::CONFIG_INVALID,
                format!("runtime.sandbox = `{other}` 不是已知的 DSH 沙箱档"),
            )),
        }
    }

    /// 严格程度：数值越小越严。`workspace` 与 `confirm` 权限相同，只是要不要应答者。
    const fn rank(self) -> u8 {
        match self {
            Self::Readonly => 0,
            Self::Workspace | Self::Confirm => 1,
            Self::Full => 2,
        }
    }

    /// 两个级别的交集 —— 取更严的那一个。授权只能收紧，所以这里没有「覆盖」。
    pub const fn stricter(self, other: Self) -> Self {
        if self.rank() <= other.rank() {
            self
        } else {
            other
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Readonly => "readonly",
            Self::Workspace => "workspace",
            Self::Full => "full",
            Self::Confirm => "confirm",
        }
    }
}

impl Default for Permission {
    /// 缺省是 [`Permission::Workspace`]：工作区内直接改、直接跑。
    /// 与 TUI 的默认一致；GUI 把它抬到 `confirm`（那一侧由界面自己决定）。
    fn default() -> Self {
        Self::Workspace
    }
}

/// 策略档。档位只规定**能力模板**；具体路径与网络上限仍必须由启动配置给出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    /// 无读取、无写入、无 shell、无网络。离线验收用。
    OfflineTest,
    /// 可读 workspace，不可写、不可 shell。
    WorkspaceReadonly,
    /// 可读写 workspace、可 shell；**不**自动获得提交、推送或整盘访问。
    WorkspaceDevelopment,
}

impl Profile {
    pub fn parse(raw: &str) -> SpecResult<Self> {
        match raw {
            "offline-test" => Ok(Profile::OfflineTest),
            "workspace-readonly" => Ok(Profile::WorkspaceReadonly),
            "workspace-development" => Ok(Profile::WorkspaceDevelopment),
            other => Err(SpecError::new(
                code::PROFILE_UNKNOWN,
                format!(
                    "未知策略档 `{other}`；已知档位：offline-test, workspace-readonly, workspace-development"
                ),
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Profile::OfflineTest => "offline-test",
            Profile::WorkspaceReadonly => "workspace-readonly",
            Profile::WorkspaceDevelopment => "workspace-development",
        }
    }

    pub const fn allows_write(self) -> bool {
        matches!(self, Profile::WorkspaceDevelopment)
    }

    /// 档位是否允许读取 workspace。`offline-test` 连读都不允许。
    pub const fn allows_read(self) -> bool {
        !matches!(self, Profile::OfflineTest)
    }

    pub const fn allows_shell(self) -> bool {
        matches!(self, Profile::WorkspaceDevelopment)
    }

    /// 档位的默认读取范围（相对 workspace）。空表示什么都不许读。
    pub const fn default_read_paths(self) -> &'static [&'static str] {
        match self {
            Profile::OfflineTest => &[],
            Profile::WorkspaceReadonly | Profile::WorkspaceDevelopment => &["."],
        }
    }

    pub const fn default_write_paths(self) -> &'static [&'static str] {
        match self {
            Profile::OfflineTest | Profile::WorkspaceReadonly => &[],
            Profile::WorkspaceDevelopment => &["."],
        }
    }
}

impl std::fmt::Display for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一个任务解析完成后的有效授权。
///
/// 路径一律是**相对 workspace 的归一化路径**：这样序列化进诊断日志时不含
/// 机器相关前缀，比较时也不受大小写与分隔符影响。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    pub agent: AgentId,
    pub mode: Mode,
    pub profile: String,
    #[serde(skip)]
    pub workspace: PathBuf,
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
    pub shell: bool,
    pub network: bool,
    pub commit: bool,
    pub push: bool,
}

/// DSH 执行层真正能表达的限制 —— 由 [`EffectivePolicy`] 投影出来的**可执行**结果。
///
/// 为什么要有这一层（对照见 `docs/M3_GUARD.md`）：`EffectivePolicy` 是产品语义
/// （「只许写 `crates/`」、「不许推送」），而 DSH 能表达的是「整个 workspace 可写 /
/// 完全不可写」加若干工具行开关。差集里表达不了的那些**必须在这里报错**，
/// 不能等渲染 patch 时静默丢掉 —— §5.4 写得很清楚：文件范围声明不是 OS 沙箱。
/// 静默丢掉等于把「以为收紧了」当成「已经收紧了」。
///
/// 渲染侧（`arc-deck-dsh::runtime::render_patch`）只读这个结构、不自己再判一次：
/// 判定与渲染分开，两份判断迟早会漂移。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionGuard {
    /// 这份 guard 是给哪份策略算的。
    pub policy: EffectivePolicy,
    /// 最终 DSH 沙箱档 —— 启动级别与策略上限的交集，只能更严。
    pub sandbox: Permission,
    /// shell 类工具（bash / pwsh / jobs / workflow / ptc）可不可执行。
    pub shell_tools: bool,
    /// `web_search` / `web_fetch` 可不可执行。
    pub web_tools: bool,
    /// 授权说得比执行层细、因此**只留档**的项。
    ///
    /// 它们会进 `result.unverified` 与事件流：留着是为了让主控看见「这一条没有被
    /// 执行层保证」，而不是把缺口藏进一份看起来完整的策略留档里。
    pub recorded: Vec<String>,
}

impl ExecutionGuard {
    /// 写进 `DSH_PERMISSION_MODE` 与 patch 的档名。**唯一出处是 [`Permission::sandbox`]。**
    pub const fn sandbox_mode(&self) -> &'static str {
        self.sandbox.sandbox()
    }
}

/// 路径列表是否覆盖整个 workspace。
///
/// 归一化之后，workspace 根本身是 `.`（见 [`paths::workspace_relative`]）。
/// 只有覆盖根才是 DSH 能表达的范围：「整个 workspace」或「什么都没有」，没有中间态。
fn covers_workspace(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| p.as_os_str() == "." || p.as_os_str().is_empty())
}

impl EffectivePolicy {
    /// 把产品语义的策略投影成 DSH 执行层能落地的限制。
    ///
    /// `startup` 是启动配置给的那一级（`runtime.sandbox` 反解而来），它本身就是上限：
    /// 这里只会算得更严。表达不了的项按「宁可拒绝启动」办 —— 除了 commit / push，
    /// DSH 没有按 git 子命令区分权限的能力，那两条进 `recorded`（见 `docs/M3_GUARD.md` §3）。
    pub fn execution_guard(&self, startup: Permission) -> SpecResult<ExecutionGuard> {
        // `confirm` 与 `workspace` 同权限，档名里反解不出来；真到手上一律按 workspace 算。
        let startup = match startup {
            Permission::Confirm => Permission::Workspace,
            other => other,
        };

        // ── 写范围 ────────────────────────────────────────────────────────
        //
        // `workspace-write` 的写边界恒等于会话 cwd（= workspace 根）加平台临时区
        // （dsh-fs-sandbox → writableRoots），**没有按路径列表收窄的表达**
        // （参 docs/M3_GUARD.md §2）。所以收窄的 write_paths 只能拒绝启动：
        // 照原样跑下去等于把「只许写 crates/」放宽成「整个 workspace 都能写」。
        //
        // 空列表是**可表达**的（那就是 read-only 档「什么都不许写」），所以只挑非空且不覆盖根的。
        if !self.write_paths.is_empty() && !covers_workspace(&self.write_paths) {
            return Err(SpecError::new(
                code::AUTH_GUARD_UNSUPPORTED,
                format!(
                    "授权只允许写 {}，但 DSH 的 workspace-write 写边界是整个 workspace `{}`，\
                     没有按路径列表收窄的表达；照这样启动等于把写范围放宽。\
                     要么把 write_paths 放成整个 workspace（`.`），要么改用夹具运行时（参 docs/M3_GUARD.md §2）",
                    describe(&self.write_paths),
                    self.workspace.display()
                ),
            ));
        }

        // ── 读范围 ────────────────────────────────────────────────────────
        //
        // DSH 对读取**完全不设边界**：fs-sandbox 只拦写入，三个档位都允许读任意绝对路径
        // （dsh-fs-sandbox 的注释：Reads pass through untouched）。
        // 所以任何「只许读某些目录」的声明在这里都是空头承诺。
        if !covers_workspace(&self.read_paths) {
            return Err(SpecError::new(
                code::AUTH_GUARD_UNSUPPORTED,
                format!(
                    "授权只允许读 {}，但 DSH 的读取不受沙箱限制（三个档位都能读任意路径），\
                     没有表达读范围的配置；照这样启动等于把读范围放宽到整台机器。\
                     要么把 read_paths 放成整个 workspace（`.`）（参 docs/M3_GUARD.md §2）",
                    describe(&self.read_paths)
                ),
            ));
        }

        // ── 网络 ──────────────────────────────────────────────────────────
        //
        // 唯一的网络开关是「把 web_search / web_fetch 两行摘掉」；shell 还开着时
        // 任何命令都能自己联网，而 DSH 没有网络沙箱可表达这件事。
        if !self.network && self.shell {
            return Err(SpecError::new(
                code::AUTH_GUARD_UNSUPPORTED,
                "授权不允许联网，但 shell 还开着：DSH 只有「摘掉 web_search / web_fetch 两行」\
                 这一种网络限制，命令自己发请求拦不住（参 docs/M3_GUARD.md §2）。\
                 要么把 network 打开，要么把 shell 关掉（shell_policy: workspace-readonly）",
            ));
        }

        // ── 沙箱档：启动级别 ∩ 策略上限 ────────────────────────────────────
        //
        // 上限只由一个事实决定：**这份策略还允许写吗**。不许写 → `read-only`（DSH 里
        // 唯一能表达「文件不被修改」的档）；允许写 → 不额外设限，上限就是 DSH 的最宽档，
        // 于是结果等于启动级别。
        //
        // 这里刻意**不**因为 `commit == false` 就把 full 降到 workspace-write：full 是
        // 用户显式给的级别（worktree 里连 `git status` 都要它），降档会把「只读一眼仓库」
        // 这种正常任务一起弄坏。commit / push 的缺口如实记进 `recorded`。
        let cap = if self.write_paths.is_empty() { Permission::Readonly } else { Permission::Full };
        let sandbox = startup.stricter(cap);

        let mut recorded = Vec::new();
        // 只有 shell 可执行时这两条才有意义：命令都跑不了，也就没法提交 / 推送。
        if self.shell {
            if !self.commit {
                recorded.push(
                    "本次授权不允许提交，但 DSH 没有按 git 子命令区分权限的能力：\
                     agent 用 shell 跑 `git commit` 不会被执行层拒绝（参 docs/M3_GUARD.md §3）"
                        .to_string(),
                );
            }
            if !self.push {
                recorded.push(
                    "本次授权不允许推送，但 DSH 没有按 git 子命令区分权限的能力：\
                     agent 用 shell 跑 `git push` 不会被执行层拒绝（参 docs/M3_GUARD.md §3）"
                        .to_string(),
                );
            }
        }

        Ok(ExecutionGuard {
            policy: self.clone(),
            sandbox,
            shell_tools: self.shell,
            web_tools: self.network,
            recorded,
        })
    }
}

/// 报错信息里的路径列表 —— 空列表要说成「什么都没有」，不能只打一对括号。
fn describe(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "（空）".to_string();
    }
    paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
}

/// 校验通过、可以交给 worker 执行的任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub task_id: String,
    pub instruction: String,
    #[serde(skip)]
    pub workspace: PathBuf,
    pub policy: EffectivePolicy,
    pub acceptance: Vec<Acceptance>,
    pub timeout_seconds: u64,
}

impl ExecutionPlan {
    /// 解析任务书并算出有效授权（不含 Agent 定义的档位上限）。
    pub fn resolve(config: &WorkerConfig, task: &TaskSpec) -> SpecResult<Self> {
        Self::resolve_with_ceiling(config, task, None)
    }

    /// 带 Agent 定义档位上限的解析。
    ///
    /// Agent 定义里的 `permission_profile` 是**第三层上限**：它只能比启动配置更严。
    /// 没有这一层，一个定义成只读的研究 Agent 在被配置成 `workspace-development`
    /// 的环境里就会拿到写权限 —— 而「这个 Agent 本来就不该写」正是定义想表达的事。
    pub fn resolve_with_ceiling(
        config: &WorkerConfig,
        task: &TaskSpec,
        agent_ceiling: Option<Profile>,
    ) -> SpecResult<Self> {
        task.validate()?;

        if !paths::same_path(&task.workspace, &config.authorization.workspace) {
            return Err(SpecError::new(
                code::AUTH_WORKSPACE_MISMATCH,
                format!(
                    "任务书 workspace `{}` 与启动配置 workspace `{}` 不是同一个位置",
                    task.workspace.display(),
                    config.authorization.workspace.display()
                ),
            ));
        }

        let config_profile = Profile::parse(&config.authorization.profile)?;
        // 第三层上限：Agent 定义只能收紧，不能放宽。
        let base_profile = match agent_ceiling {
            Some(ceiling) => config_profile.min(ceiling),
            None => config_profile,
        };
        let workspace = task.workspace.clone();

        let agent_raw = task.agent.clone().unwrap_or_else(|| AgentId::Echo.as_str().to_string());
        let agent = AgentId::parse(&agent_raw)?;
        if !config.authorization.allowed_agents.iter().any(|a| a == &agent_raw) {
            return Err(SpecError::new(
                code::AGENT_NOT_AUTHORIZED,
                format!(
                    "Agent `{agent_raw}` 未在启动配置的 allowed_agents 中授权（当前授权：{}）",
                    if config.authorization.allowed_agents.is_empty() {
                        "空 —— 默认不授权任何 Agent".to_string()
                    } else {
                        config.authorization.allowed_agents.join(", ")
                    }
                ),
            ));
        }
        let agent_mode = AgentMode::resolve(Some(agent), task.mode)?;

        // ── 第一层：启动配置（档位模板 ∪ 显式路径）────────────────────────
        //
        // 这里是「档位布尔 × 路径列表」两个条件都成立才有权限：一个更严的
        // Agent 档位（例如 workspace-readonly）即使配置里写了 write_paths，
        // 也不该让这个 Agent 拿到写权限。
        let base_read = if base_profile.allows_read() {
            resolve_scope(
                &workspace,
                if config.authorization.read_paths.is_empty() {
                    profile_defaults(base_profile.default_read_paths())
                } else {
                    config.authorization.read_paths.clone()
                },
            )?
        } else {
            Vec::new()
        };
        let base_write = if base_profile.allows_write() {
            resolve_scope(
                &workspace,
                if config.authorization.write_paths.is_empty() {
                    profile_defaults(base_profile.default_write_paths())
                } else {
                    config.authorization.write_paths.clone()
                },
            )?
        } else {
            Vec::new()
        };

        let mut shell = base_profile.allows_shell();
        let network = config.authorization.network && base_profile != Profile::OfflineTest;
        let mut commit = config.authorization.allow_commit;
        let mut push = config.authorization.allow_push;

        // ── 第二层：任务书 constraints（只能收紧）────────────────────────
        let constraints = task.constraints.clone().unwrap_or_default();

        if let Some(raw) = &constraints.shell_policy {
            let requested = Profile::parse(raw)?;
            if requested > base_profile {
                let ceiling_note = match agent_ceiling {
                    Some(c) if c < config_profile => format!("，Agent 定义上限 `{c}`"),
                    _ => String::new(),
                };
                return Err(SpecError::new(
                    code::AUTH_EXCEEDED,
                    format!(
                        "任务书请求策略档 `{requested}`，高于当前上限 `{base_profile}`（启动配置 `{config_profile}`{ceiling_note}）；任务只能收紧授权，不能提权"
                    ),
                ));
            }
            shell = shell && requested.allows_shell();
        }

        let read_paths = match &constraints.read_paths {
            None => base_read.clone(),
            Some(requested) => intersect_scope(&workspace, &base_read, requested, "read_paths")?,
        };
        let mut write_paths = match &constraints.write_paths {
            None => base_write.clone(),
            Some(requested) => intersect_scope(&workspace, &base_write, requested, "write_paths")?,
        };

        if constraints.allow_commit == Some(true) && !commit {
            return Err(SpecError::new(
                code::AUTH_EXCEEDED,
                "任务书请求 allow_commit=true，但启动配置未授权提交",
            ));
        }
        if constraints.allow_push == Some(true) && !push {
            return Err(SpecError::new(
                code::AUTH_EXCEEDED,
                "任务书请求 allow_push=true，但启动配置未授权推送",
            ));
        }
        commit = commit && constraints.allow_commit.unwrap_or(true);
        push = push && constraints.allow_push.unwrap_or(true);

        // ── 第三层：模式限制 ──────────────────────────────────────────────
        if agent_mode.mode == Mode::Plan {
            // 档位允许写入不代表这个任务要写 —— 只有**任务显式请求**写入时才是
            // 矛盾，那时必须报错而不是静默清空：静默清空会让主控以为自己要的
            // 写入被安排了，实际什么都没发生。
            let asked_to_write =
                constraints.write_paths.as_ref().is_some_and(|p| !p.is_empty());
            if asked_to_write {
                return Err(SpecError::new(
                    code::MODE_CONFLICT,
                    "Plan 模式不接受 write_paths；写入请求必须改用 build 模式",
                ));
            }
            if constraints.allow_commit == Some(true) || constraints.allow_push == Some(true) {
                return Err(SpecError::new(code::MODE_CONFLICT, "Plan 模式不接受提交或推送授权"));
            }
            // Plan 本身就是「只研究与规划」：无论档位给到什么，写与 shell 都关掉。
            write_paths.clear();
            shell = false;
            commit = false;
            push = false;
        }

        // ── 验收预检 ─────────────────────────────────────────────────────
        for acceptance in &task.acceptance {
            if let Acceptance::Command { id, cwd, .. } = acceptance {
                if !shell {
                    return Err(SpecError::new(
                        code::ACCEPTANCE_NOT_ALLOWED,
                        format!(
                            "验收 `{id}` 是 command，但当前授权不允许执行命令（档位 `{base_profile}`、模式 `{}`）；验收身份不能绕过授权",
                            agent_mode.mode
                        ),
                    ));
                }
                if let Some(cwd) = cwd {
                    // cwd 不需要落在可写范围内，但必须在 workspace 之内 ——
                    // 否则一条验收命令就能在别处运行。
                    paths::workspace_relative(&workspace, cwd)?;
                }
            }
        }

        let timeout_seconds = task.timeout_seconds.unwrap_or(config.timeouts.default_task_seconds);
        if timeout_seconds == 0 {
            return Err(SpecError::new(code::TASK_INVALID, "timeout_seconds 必须大于 0"));
        }

        // Plan 的 write_paths 在进入这里之前已被清空；显式请求写入的组合在上面
        // 就报错了。这一行是给「档位允许写但模式是 plan」的正常路径留的断言。
        debug_assert!(agent_mode.mode != Mode::Plan || write_paths.is_empty());

        Ok(ExecutionPlan {
            task_id: task.task_id.clone(),
            instruction: task.instruction.clone(),
            workspace: workspace.clone(),
            policy: EffectivePolicy {
                agent: agent_mode.agent,
                mode: agent_mode.mode,
                profile: base_profile.to_string(),
                workspace,
                read_paths,
                write_paths,
                shell,
                network,
                commit,
                push,
            },
            acceptance: task.acceptance.clone(),
            timeout_seconds,
        })
    }

    /// 所有 `command` 验收，按声明顺序。
    pub fn command_acceptances(&self) -> Vec<&Acceptance> {
        self.acceptance.iter().filter(|a| a.is_command()).collect()
    }
}

fn profile_defaults(raw: &[&str]) -> Vec<PathBuf> {
    raw.iter().map(PathBuf::from).collect()
}

/// 把一串范围路径归一化成「相对 workspace」的形式，并拒绝逃逸。
fn resolve_scope(workspace: &Path, raw: Vec<PathBuf>) -> SpecResult<Vec<PathBuf>> {
    raw.iter().map(|p| paths::workspace_relative(workspace, p)).collect()
}

/// 交集：请求的每个路径都必须落在某个上限之内。
fn intersect_scope(
    workspace: &Path,
    base: &[PathBuf],
    requested: &[PathBuf],
    field: &str,
) -> SpecResult<Vec<PathBuf>> {
    let mut out = Vec::with_capacity(requested.len());
    for raw in requested {
        let rel = paths::workspace_relative(workspace, raw)?;
        let abs = paths::resolve_against(workspace, &rel);
        let inside = base.iter().any(|b| {
            let base_abs = paths::resolve_against(workspace, b);
            paths::is_within(&abs, &base_abs)
        });
        if !inside {
            return Err(SpecError::new(
                code::AUTH_EXCEEDED,
                format!(
                    "{field} 里的 `{}` 超出启动配置允许的范围（允许：{}）",
                    raw.display(),
                    base.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        out.push(rel);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_json(profile: &str, extra_auth: &str) -> String {
        format!(
            r#"{{
                "schema_version": 1,
                "runtime": {{ "kind": "fixture-sdk" }},
                "state_root": "state",
                "authorization": {{
                    "workspace": ".",
                    "profile": "{profile}",
                    "allowed_agents": ["echo", "build", "plan"]
                    {extra_auth}
                }},
                "agents": {{ "dir": "agents", "prompts_dir": "prompts" }}
            }}"#
        )
    }

    fn task_json(extra: &str) -> String {
        format!(
            r#"{{
                "schema_version": 1,
                "task_id": "t1",
                "workspace": ".",
                "instruction": "hello"
                {extra}
            }}"#
        )
    }

    fn resolve(config_raw: &str, task_raw: &str) -> SpecResult<ExecutionPlan> {
        let cfg = WorkerConfig::parse(config_raw, &paths::test_abs("work")).unwrap();
        let task = TaskSpec::parse(task_raw, &paths::test_abs("work")).unwrap();
        ExecutionPlan::resolve(&cfg, &task)
    }

    /// `arcdeck run` 生成的默认授权（`commands.rs`：读写路径留空、`network: true`）必须能落到执行层。
    /// 新加的三条「拒绝启动」规则任何一条误伤它，派工就全部 64 —— 包括正在用它开发本仓的工人。
    #[test]
    fn default_dispatch_shape_is_not_refused_by_the_execution_guard() {
        let cfg = config_json("workspace-development", r#", "network": true"#);
        let plan = resolve(&cfg, &task_json("")).unwrap();
        for startup in [Permission::Workspace, Permission::Full] {
            let guard = plan.policy.execution_guard(startup).expect("默认派工形状必须能启动");
            assert_eq!(guard.sandbox, startup, "允许写时不降档（worktree 里的 git 靠 full）");
            assert!(guard.shell_tools && guard.web_tools);
        }
        // 同样的默认形状换成 Plan：不拒绝启动，但收紧成只读、摘掉 shell。
        let plan = resolve(&cfg, &task_json(r#", "agent": "plan""#)).unwrap();
        let guard = plan.policy.execution_guard(Permission::Full).expect("Plan 也必须能启动");
        assert_eq!(guard.sandbox, Permission::Readonly);
        assert!(!guard.shell_tools);
    }

    #[test]
    fn development_profile_allows_write_and_shell() {
        let cfg = config_json("workspace-development", r#", "write_paths": ["crates/"]"#);
        let plan = resolve(&cfg, &task_json("")).unwrap();
        assert!(plan.policy.shell);
        assert_eq!(plan.policy.write_paths, vec![PathBuf::from("crates")]);
        assert_eq!(plan.policy.agent, AgentId::Echo);
        assert_eq!(plan.policy.mode, Mode::Build);
    }

    #[test]
    fn task_cannot_widen_write_scope() {
        let cfg = config_json("workspace-development", r#", "write_paths": ["crates/"]"#);
        let task = task_json(r#", "constraints": { "write_paths": ["crates/", "uwp/"] }"#);
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::AUTH_EXCEEDED), "{err}");
    }

    #[test]
    fn task_cannot_upgrade_profile() {
        let cfg = config_json("workspace-readonly", "");
        let task = task_json(r#", "constraints": { "shell_policy": "workspace-development" }"#);
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::AUTH_EXCEEDED), "{err}");
    }

    #[test]
    fn command_acceptance_requires_shell() {
        let cfg = config_json("workspace-readonly", "");
        let task = task_json(r#", "acceptance": [{ "id": "a", "kind": "command", "command": "cargo test" }]"#);
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::ACCEPTANCE_NOT_ALLOWED), "{err}");
    }

    #[test]
    fn plan_rejects_write_requests() {
        let cfg = config_json("workspace-development", r#", "write_paths": ["crates/"]"#);
        let task = task_json(
            r#", "agent": "echo", "mode": "plan", "constraints": { "write_paths": ["crates/"] }"#,
        );
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::MODE_CONFLICT), "{err}");
    }

    #[test]
    fn plan_rejects_command_acceptance() {
        let cfg = config_json("workspace-development", "");
        let task = task_json(
            r#", "agent": "plan", "mode": "plan", "acceptance": [{ "id": "a", "kind": "command", "command": "cargo test" }]"#,
        );
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::ACCEPTANCE_NOT_ALLOWED), "{err}");
    }

    #[test]
    fn plan_allows_manual_acceptance_and_disables_shell() {
        let cfg = config_json("workspace-development", "");
        let task = task_json(
            r#", "agent": "plan", "mode": "plan", "acceptance": [{ "id": "a", "kind": "manual", "description": "看一遍" }]"#,
        );
        let plan = resolve(&cfg, &task).unwrap();
        assert!(!plan.policy.shell);
        assert!(plan.policy.write_paths.is_empty());
        assert_eq!(plan.policy.mode, Mode::Plan);
    }

    #[test]
    fn unauthorized_agent_is_rejected() {
        let cfg = config_json("workspace-development", "")
            .replace(r#""allowed_agents": ["echo", "build", "plan"]"#, r#""allowed_agents": ["build"]"#);
        let task = task_json(r#", "agent": "echo""#);
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::AGENT_NOT_AUTHORIZED), "{err}");
    }

    #[test]
    fn workspace_mismatch_is_rejected() {
        let cfg = config_json("workspace-development", "");
        let elsewhere = format!(r#""workspace": "{}""#, paths::test_abs_str("elsewhere"));
        let task = task_json("").replace(r#""workspace": ".""#, &elsewhere);
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::AUTH_WORKSPACE_MISMATCH), "{err}");
    }

    #[test]
    fn agent_ceiling_can_only_tighten() {
        let cfg = config_json("workspace-development", r#", "write_paths": ["crates/"]"#);
        let task = task_json("");
        let cfg_parsed = WorkerConfig::parse(&cfg, &paths::test_abs("work")).unwrap();
        let task_parsed = TaskSpec::parse(&task, &paths::test_abs("work")).unwrap();

        // Agent 定义成只读：即使启动配置允许写，也不该拿到写权限。
        let read_only = ExecutionPlan::resolve_with_ceiling(
            &cfg_parsed,
            &task_parsed,
            Some(Profile::WorkspaceReadonly),
        )
        .unwrap();
        assert!(!read_only.policy.shell);
        assert!(read_only.policy.write_paths.is_empty());
        assert_eq!(read_only.policy.profile, "workspace-readonly");

        // 反过来：Agent 定义比启动配置宽松时，启动配置仍然赢。
        let dev = ExecutionPlan::resolve_with_ceiling(
            &cfg_parsed,
            &task_parsed,
            Some(Profile::WorkspaceDevelopment),
        )
        .unwrap();
        assert_eq!(dev.policy.profile, "workspace-development");
        assert!(dev.policy.shell);
    }

    #[test]
    fn escaping_cwd_is_rejected() {
        let cfg = config_json("workspace-development", "");
        let task = task_json(
            r#", "acceptance": [{ "id": "a", "kind": "command", "command": "x", "cwd": "../.." }]"#,
        );
        let err = resolve(&cfg, &task).unwrap_err();
        assert!(err.is(code::AUTH_EXCEEDED), "{err}");
    }

    #[test]
    fn offline_profile_denies_everything_even_with_explicit_paths() {
        let cfg = config_json("offline-test", "");
        let task = task_json("");
        let plan = resolve(&cfg, &task).unwrap();
        assert!(!plan.policy.shell);
        assert!(plan.policy.read_paths.is_empty());
        assert!(plan.policy.write_paths.is_empty());
    }
}
