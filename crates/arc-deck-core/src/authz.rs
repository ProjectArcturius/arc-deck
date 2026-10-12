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
