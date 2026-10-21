//! 运行时规格与启动配置（PRODUCT_PLAN §6.1）。
//!
//! 启动配置是**主控**在进程启动时给的授权上限，与任务书分离。它可以被省略的
//! 字段极少：没有默认 runtime、没有默认 workspace、没有默认权限 ——
//! 「找不到就回退到真实 DSH / Desktop home」正是要避免的失败模式。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::PromptScope;
use crate::error::{code, SpecError, SpecResult};
use crate::paths;
use crate::SCHEMA_VERSION;

/// 运行时类型。
///
/// `DshSdk` 在 M1 **可以解析但会拒绝执行**：这样配置里的错别字仍能报出准确
/// 原因，而不是笼统的「解析失败」；而真实内核路径在 M2 之前不会被意外走上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RuntimeSpec {
    /// 假 SDK 对端：只在 `test-runtime` feature 下可用，不联网、不读真实凭据。
    FixtureSdk {
        /// 显式可执行入口；省略时用当前可执行文件重新执行自身的隐藏子命令。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<PathBuf>,
        /// 夹具场景名（success / failure / blocked / eof / bad-frame / wrong-id /
        /// split-utf8 / hang / cancel）。夹具自己解析，core 不校验。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scenario: Option<String>,
    },
    /// 真实 DSH SDK stdio（PRODUCT_PLAN §9.2a）：`node <entry> --profile <profile> --patch <生成的 patch>`。
    ///
    /// 这里只记**密钥在哪**（`route.key_source`），从不记密钥本身 —— 本结构会被物化进
    /// 任务状态目录，而那个目录不该成为凭据的第二个存放处。
    DshSdk {
        /// `node` 可执行文件；写 `node` 时按 PATH 查找。
        #[serde(default = "default_node")]
        node: PathBuf,
        /// `@deepseek-ai/dsh/lib/bin.js`。
        entry: PathBuf,
        version: String,
        profile: String,
        /// 路由。缺了它就不知道该往 patch 里写哪个适配器。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        route: Option<RouteSpec>,
        /// DSH 沙箱档：`read-only` / `workspace-write` / `danger-full-access`。
        #[serde(default = "default_sandbox")]
        sandbox: String,
        /// DSH 默认把会话日志随官方请求上传；不显式打开就关掉。
        #[serde(default)]
        session_log_upload: bool,
        /// 工人守则文件，进 system prompt（persona 后缀）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preamble: Option<PathBuf>,
    },
}

fn default_node() -> PathBuf {
    PathBuf::from("node")
}

fn default_sandbox() -> String {
    "workspace-write".to_string()
}

/// 物化后的模型路由（由 [`crate::provider::resolve`] 产出）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteSpec {
    pub provider_id: String,
    /// `deepseek-official` 或 `pi-ai`。
    pub adapter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<RouteModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 运行时子进程里承载密钥的环境变量名。
    pub key_env: String,
    /// `auth`（saiku auth.json）、`env:<变量>`、`file:<路径>`。
    pub key_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// 探活与自救的阈值（PRODUCT_PLAN §6.5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Liveness {
    #[serde(default = "liveness_defaults::heartbeat")]
    pub heartbeat_seconds: u64,
    #[serde(default = "liveness_defaults::model_idle")]
    pub model_idle_seconds: u64,
    #[serde(default = "liveness_defaults::tool_stall")]
    pub tool_stall_seconds: u64,
    /// 卡死后最多跑几次（含第一次）。夹具与 M1 语义是 1：不续跑。
    #[serde(default = "liveness_defaults::max_attempts")]
    pub max_attempts: u32,
}

impl Default for Liveness {
    fn default() -> Self {
        Self {
            heartbeat_seconds: liveness_defaults::heartbeat(),
            model_idle_seconds: liveness_defaults::model_idle(),
            tool_stall_seconds: liveness_defaults::tool_stall(),
            max_attempts: liveness_defaults::max_attempts(),
        }
    }
}

mod liveness_defaults {
    pub fn heartbeat() -> u64 {
        15
    }
    pub fn model_idle() -> u64 {
        600
    }
    pub fn tool_stall() -> u64 {
        1800
    }
    pub fn max_attempts() -> u32 {
        1
    }
}

/// Agent 定义与提示词的来源目录。显式给出，不做自动发现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSources {
    pub dir: PathBuf,
    pub prompts_dir: PathBuf,
}

/// 各项等待上限。
///
/// 给的是上限而不是「够用就行」的小值：内核加载整棵插件树可能真的要几十秒，
/// 上限太小会让慢机器上随机失败，而那种失败看起来像协议不对。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timeouts {
    #[serde(default = "defaults::initialize_seconds")]
    pub initialize_seconds: u64,
    #[serde(default = "defaults::prompt_seconds")]
    pub prompt_seconds: u64,
    #[serde(default = "defaults::wait_seconds")]
    pub wait_seconds: u64,
    /// 任务书没有 `timeout_seconds` 时用的墙钟预算。
    #[serde(default = "defaults::default_task_seconds")]
    pub default_task_seconds: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            initialize_seconds: defaults::initialize_seconds(),
            prompt_seconds: defaults::prompt_seconds(),
            wait_seconds: defaults::wait_seconds(),
            default_task_seconds: defaults::default_task_seconds(),
        }
    }
}

mod defaults {
    pub fn initialize_seconds() -> u64 {
        120
    }
    pub fn prompt_seconds() -> u64 {
        30
    }
    pub fn wait_seconds() -> u64 {
        600
    }
    pub fn default_task_seconds() -> u64 {
        900
    }
}

/// 模型路由。
///
/// `provider` 与 `model` **不是**同一个东西：把 provider 名当模型名填进去时，
/// 握手会成功、轮次也会起，错误只在 `finish.reason.failure` 里出现 ——
/// 界面上表现为「发出去以后什么都没有」。所以这两个字段必须分别显式给出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoute {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// 授权上限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorization {
    /// 必须与任务书的 `workspace` 指向同一位置。
    pub workspace: PathBuf,
    /// 策略档名，必须已知（见 [`crate::authz::Profile`]）。
    pub profile: String,
    /// 省略时用该档的默认读取范围。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_paths: Vec<PathBuf>,
    /// 省略时用该档的默认写入范围；档位不允许写入时这里必须为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write_paths: Vec<PathBuf>,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub allow_commit: bool,
    #[serde(default)]
    pub allow_push: bool,
    /// 允许出现的 Agent。默认**空**：任务不显式授权就没有任何 Agent 可用。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_agents: Vec<String>,
}

/// 启动配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    pub schema_version: u32,
    pub runtime: RuntimeSpec,
    /// 装什么进 system prompt（工人 / 个性化 / 什么都不加）。见 [`PromptScope`]。
    #[serde(default)]
    pub prompt_scope: PromptScope,
    /// 状态目录：重复 task_id 的检测与原始事件落盘都在这里。必须显式给出。
    pub state_root: PathBuf,
    pub authorization: Authorization,
    pub agents: AgentSources,
    /// 模型路由。`dsh-sdk` 必须给出；夹具不需要（夹具不会真的调用模型）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRoute>,
    /// 显式环境变量。未列出的变量不继承给运行时。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub timeouts: Timeouts,
    #[serde(default)]
    pub liveness: Liveness,
    /// workspace 是 git 仓时，结果里的提交与变更相对哪个基线采集（如 `main`）。
    /// 省略就不采集 —— 不猜基线。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_base: Option<String>,
}

impl WorkerConfig {
    /// 从文件读取。相对路径以**配置文件所在目录**为基准。
    pub fn load(path: &Path) -> SpecResult<Self> {
        if !path.exists() {
            return Err(SpecError::config_missing(path.display()));
        }
        let raw = std::fs::read_to_string(path).map_err(|e| {
            SpecError::new(code::CONFIG_UNREADABLE, format!("{} —— {e}", path.display()))
        })?;
        // 相对基准必须锚成绝对路径，否则配置里的 `../..` 解析出来还是相对的。
        let origin = paths::absolutize(path);
        let origin = origin.parent().unwrap_or_else(|| Path::new("."));
        let mut config: WorkerConfig = serde_json::from_str(&raw).map_err(|e| {
            SpecError::new(code::CONFIG_INVALID, format!("{} 不是合法启动配置 —— {e}", path.display()))
        })?;
        config.resolve_paths(origin);
        config.validate()?;
        Ok(config)
    }

    /// 从 JSON 文本解析（测试用）。相对路径以 `origin_dir` 为基准。
    pub fn parse(raw: &str, origin_dir: &Path) -> SpecResult<Self> {
        let mut config: WorkerConfig = serde_json::from_str(raw)
            .map_err(|e| SpecError::new(code::CONFIG_INVALID, format!("不是合法启动配置 —— {e}")))?;
        config.resolve_paths(origin_dir);
        config.validate()?;
        Ok(config)
    }

    fn resolve_paths(&mut self, origin: &Path) {
        self.state_root = paths::resolve_against(origin, &self.state_root);
        self.authorization.workspace = paths::resolve_against(origin, &self.authorization.workspace);
        self.agents.dir = paths::resolve_against(origin, &self.agents.dir);
        self.agents.prompts_dir = paths::resolve_against(origin, &self.agents.prompts_dir);
        match &mut self.runtime {
            RuntimeSpec::FixtureSdk { entry: Some(entry), .. } => {
                if !entry.is_absolute() {
                    *entry = paths::resolve_against(origin, entry);
                }
            }
            RuntimeSpec::DshSdk { entry, preamble, .. } => {
                if !entry.is_absolute() {
                    *entry = paths::resolve_against(origin, entry);
                }
                if let Some(p) = preamble {
                    if !p.is_absolute() {
                        *p = paths::resolve_against(origin, p);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn validate(&self) -> SpecResult<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(SpecError::new(
                code::CONFIG_VERSION_UNSUPPORTED,
                format!(
                    "启动配置 schema_version={}，本客户端只支持 {}",
                    self.schema_version, SCHEMA_VERSION
                ),
            ));
        }
        if !self.authorization.workspace.is_absolute() {
            return Err(SpecError::new(
                code::CONFIG_INVALID,
                "authorization.workspace 解析后仍是相对路径",
            ));
        }
        // 档位合法性在这里就查，避免一个拼错的档名一路走到执行层才炸。
        let profile = crate::authz::Profile::parse(&self.authorization.profile)?;
        if !profile.allows_write() && !self.authorization.write_paths.is_empty() {
            return Err(SpecError::new(
                code::CONFIG_INVALID,
                format!(
                    "档位 `{profile}` 不允许写入，但 authorization.write_paths 非空（{} 项）",
                    self.authorization.write_paths.len()
                ),
            ));
        }
        if !self.authorization.allowed_agents.is_empty() {
            for raw in &self.authorization.allowed_agents {
                crate::mode::AgentId::parse(raw)?;
            }
        }
        if self.liveness.max_attempts == 0 || self.liveness.heartbeat_seconds == 0 {
            return Err(SpecError::new(code::CONFIG_INVALID, "liveness.max_attempts 与 heartbeat_seconds 必须大于 0"));
        }
        if self.timeouts.initialize_seconds == 0
            || self.timeouts.prompt_seconds == 0
            || self.timeouts.wait_seconds == 0
            || self.timeouts.default_task_seconds == 0
        {
            return Err(SpecError::new(code::CONFIG_INVALID, "timeouts 各项必须大于 0"));
        }
        match &self.runtime {
            RuntimeSpec::FixtureSdk { .. } => Ok(()),
            RuntimeSpec::DshSdk { version, profile, sandbox, .. } => {
                // 这里是**实现层**的 DSH 沙箱档。用户级说的是 `Permission`，
                // 由 `cmd_run` 映射过来（见 `authz::Permission::sandbox`）。
                if !matches!(sandbox.as_str(), "read-only" | "workspace-write" | "danger-full-access") {
                    return Err(SpecError::new(
                        code::CONFIG_INVALID,
                        format!("runtime.sandbox = `{sandbox}` 不是已知的 DSH 沙箱档"),
                    ));
                }
                if version.trim().is_empty() || profile.trim().is_empty() {
                    return Err(SpecError::new(
                        code::CONFIG_INVALID,
                        "runtime.dsh-sdk 的 version 与 profile 都必须显式给出",
                    ));
                }
                let model = self.model.as_ref().ok_or_else(|| {
                    SpecError::new(
                        code::CONFIG_INVALID,
                        "runtime.dsh-sdk 必须显式给出 model.provider 与 model.model（没有默认路由）",
                    )
                })?;
                if model.provider.trim().is_empty() || model.model.trim().is_empty() {
                    return Err(SpecError::new(
                        code::CONFIG_INVALID,
                        "model.provider 与 model.model 都不得为空",
                    ));
                }
                Ok(())
            }
        }
    }

    /// 运行时可执行入口。`FixtureSdk` 未给 `entry` 时回落到「重新执行自身」。
    pub fn runtime_entry_or_self(&self) -> Option<PathBuf> {
        match &self.runtime {
            RuntimeSpec::FixtureSdk { entry, .. } => entry.clone(),
            RuntimeSpec::DshSdk { entry, .. } => Some(entry.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{
        "schema_version": 1,
        "runtime": { "kind": "fixture-sdk", "scenario": "success" },
        "state_root": "state",
        "authorization": {
            "workspace": ".",
            "profile": "workspace-development",
            "write_paths": ["crates/"],
            "allowed_agents": ["echo"]
        },
        "agents": { "dir": "agents", "prompts_dir": "prompts" }
    }"#;

    fn origin() -> PathBuf {
        paths::test_abs("repo/tests/fixtures")
    }

    #[test]
    fn minimal_config_parses_and_resolves_relative_paths() {
        let cfg = WorkerConfig::parse(MINIMAL, &origin()).unwrap();
        assert_eq!(cfg.state_root, paths::test_abs("repo/tests/fixtures/state"));
        assert_eq!(cfg.authorization.workspace, paths::test_abs("repo/tests/fixtures"));
        assert_eq!(cfg.agents.dir, paths::test_abs("repo/tests/fixtures/agents"));
        assert_eq!(cfg.agents.prompts_dir, paths::test_abs("repo/tests/fixtures/prompts"));
        assert_eq!(cfg.timeouts.initialize_seconds, 120);
        assert_eq!(cfg.timeouts.default_task_seconds, 900);
    }

    #[test]
    fn unknown_profile_is_rejected() {
        // 档名尾部多空格必须报未知档，而不是模糊匹配到某个档。
        let raw = MINIMAL.replace(r#""profile": "workspace-development""#, r#""profile": "workspace-development ""#);
        let err = WorkerConfig::parse(&raw, &origin()).unwrap_err();
        assert!(err.is(code::PROFILE_UNKNOWN), "{err}");
    }

    #[test]
    fn readonly_profile_with_write_paths_is_rejected_at_config_time() {
        let raw = MINIMAL.replace("workspace-development", "workspace-readonly");
        let err = WorkerConfig::parse(&raw, &origin()).unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");
    }

    #[test]
    fn dsh_sdk_requires_an_explicit_model_route() {
        let raw = MINIMAL.replace(
            r#"{ "kind": "fixture-sdk", "scenario": "success" }"#,
            r#"{ "kind": "dsh-sdk", "entry": "dsh.js", "version": "0.2.1-alpha.2", "profile": "sdk" }"#,
        );
        // 没有模型路由：dsh-sdk 必须报错，而不是带着一个编造的路由起内核。
        let err = WorkerConfig::parse(&raw, &origin()).unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");

        let raw = raw.replace(
            r#""agents": { "dir": "agents", "prompts_dir": "prompts" }"#,
            r#""agents": { "dir": "agents", "prompts_dir": "prompts" },
               "model": { "provider": "deepseek-official", "model": "deepseek-flash" }"#,
        );
        let cfg = WorkerConfig::parse(&raw, &origin()).unwrap();
        assert!(matches!(cfg.runtime, RuntimeSpec::DshSdk { .. }));
        assert_eq!(cfg.model.as_ref().unwrap().model, "deepseek-flash");
    }

    #[test]
    fn fixture_runtime_does_not_need_a_model_route() {
        let cfg = WorkerConfig::parse(MINIMAL, &origin()).unwrap();
        assert!(cfg.model.is_none());
    }

    #[test]
    fn unknown_runtime_kind_is_rejected() {
        let raw = MINIMAL.replace("fixture-sdk", "fixture-sdkx");
        let err = WorkerConfig::parse(&raw, &origin()).unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let raw = MINIMAL.replace(r#""state_root": "state","#, r#""state_root": "state", "state_rot": "x","#);
        let err = WorkerConfig::parse(&raw, &origin()).unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");
    }

    #[test]
    fn missing_config_reports_config_missing() {
        let err = WorkerConfig::load(Path::new("/definitely/not/here.json")).unwrap_err();
        assert!(err.is(code::CONFIG_MISSING), "{err}");
    }
}
