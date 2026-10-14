//! 用户级配置 `arcdeck.json` 与标准目录（PRODUCT_PLAN §6.0）。
//!
//! 查找顺序：`--config` → `$ARCDECK_CONFIG` → `<配置目录>/arcdeck/arcdeck.json`。
//! **永远不从 workspace 里找** —— 任务书与工作区文件不得扩大启动时授权。
//!
//! 这份配置是调度者（主控）那一侧的东西：模型路由、供应商、沙箱档、是否允许提交、
//! worktree 模板。`arcdeck run` 把它与任务书一起物化成 M1 的启动配置与任务书，
//! 落在任务状态目录里 —— 这样每次运行实际用了什么都留档可查。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::authz::Permission;
use crate::error::{code, SpecError, SpecResult};
use crate::provider::ProviderConfig;

/// 当前钉住的 DSH 版本（PRODUCT_PLAN §9.2a）。升级是刻意的工程动作。
pub const PINNED_DSH_VERSION: &str = "0.2.1-alpha.2";

/// 个性化设置。**默认关。**（用户 2026-10-14）
///
/// 关着的时候：不加风格、不加工程规则、不写风格相关的留档 ——
/// 直接按模型供应商本体的行为行事。
///
/// 哪些东西归这里，见 `prompts/constitution.md` 第四节：提交粒度与权限、语言与称呼、
/// 工具偏好、测试多少。判据是「**改掉之后产品还可不可信**」—— 改了只是换种口味的是偏好，
/// 改了就让回报失去意义的才是宪法。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Personalization {
    /// 缺省 **false**：一般模式下不注入任何 Arc Deck 文本。
    #[serde(default)]
    pub enabled: bool,
}

/// `arcdeck.json`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// `<provider>/<model>`。没有内置默认值：缺了就报错，不编一个路由。
    pub model: String,
    /// 个性化：管风格与个人习惯。**默认关**。
    #[serde(default)]
    pub personalization: Personalization,
    /// 模型强度。**产品名是 `effort`** —— opencode 叫 `variant`，那个名字不用（用户 2026-10-14）。
    /// 协议里它仍是 `reasoningEffort`（照上游），那层映射在适配器里做。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// OpenCode 形状的供应商块。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    /// 任务状态目录；省略时 `<数据目录>/arcdeck/state`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_root: Option<PathBuf>,
    #[serde(default)]
    pub worker: WorkerDefaults,
    /// 额外环境变量，值里 `{task_id}`、`{workspace}`、`{run_dir}` 会被替换。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    /// 省略时 [`PINNED_DSH_VERSION`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh_version: Option<String>,
    /// 运行时目录；省略时 `<数据目录>/arcdeck/runtime/dsh-<版本>`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// node 可执行文件；省略时用 PATH 上的 `node`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<PathBuf>,
    /// DSH 默认把会话日志随请求上传 DeepSeek（只对官方路由生效）。用户裁定前缺省关。
    #[serde(default)]
    pub session_log_upload: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerDefaults {
    /// 授权级别 —— 见 [`Permission`]。**产品层只有这一个说法。**
    ///
    /// `workspace` 只允许在工作区内写；`full` 是沙箱之外的全权，**worktree 里提交必须用它**
    /// （worktree 的 `.git` 指向主仓，而主仓在 workspace 之外 —— 实测每条 git 命令都返回 128）。
    /// 隔离由 worktree 本身承担。
    #[serde(default)]
    pub permission: Permission,
    #[serde(default)]
    pub allow_commit: bool,
    /// 每件任务都一样的工人守则（如 Ether 的 PREAMBLE.md）。进 system prompt，稳定可缓存。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preamble: Option<PathBuf>,
    #[serde(default = "defaults::timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "defaults::max_attempts")]
    pub max_attempts: u32,
    /// 等模型时多久没有任何进展算卡死。
    #[serde(default = "defaults::model_idle_seconds")]
    pub model_idle_seconds: u64,
    /// 单个工具调用跑多久算卡死（长构建是正常的，所以给得宽）。
    #[serde(default = "defaults::tool_stall_seconds")]
    pub tool_stall_seconds: u64,
}

impl Default for WorkerDefaults {
    fn default() -> Self {
        Self {
            permission: Permission::default(),
            allow_commit: false,
            preamble: None,
            timeout_seconds: defaults::timeout_seconds(),
            max_attempts: defaults::max_attempts(),
            model_idle_seconds: defaults::model_idle_seconds(),
            tool_stall_seconds: defaults::tool_stall_seconds(),
        }
    }
}

mod defaults {
    // Ether 的工人任务常跑一两个小时（DISPATCHER §三）。
    pub fn timeout_seconds() -> u64 {
        4 * 3600
    }
    pub fn max_attempts() -> u32 {
        3
    }
    // DSH 自己的流空闲上限是 300 s，且失败请求会在 step 边界重试；
    // 给它两倍余量，免得把 DSH 正在做的重试判成卡死。
    pub fn model_idle_seconds() -> u64 {
        600
    }
    // 与 Ether runner 的 30 分钟看门狗同量级。
    pub fn tool_stall_seconds() -> u64 {
        1800
    }
}

/// worktree 模板。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeConfig {
    /// worktree 目录模板，`{repo_parent}`、`{task_id}` 会被替换。
    pub dir: String,
    /// 新分支的起点。
    #[serde(default = "default_base")]
    pub base: String,
}

fn default_base() -> String {
    "main".into()
}

impl UserConfig {
    pub fn parse(raw: &str, origin: &Path) -> SpecResult<Self> {
        let config: UserConfig = serde_json::from_str(raw).map_err(|e| {
            SpecError::new(code::CONFIG_INVALID, format!("{} 不是合法的 arcdeck.json —— {e}", origin.display()))
        })?;
        crate::provider::split_model(&config.model)?;
        if config.worker.max_attempts == 0 {
            return Err(SpecError::new(code::CONFIG_INVALID, "worker.max_attempts 至少为 1"));
        }
        Ok(config)
    }

    /// 按查找顺序加载。返回配置与它的路径。
    pub fn locate_and_load(explicit: Option<&Path>) -> SpecResult<(Self, PathBuf)> {
        let path = match explicit {
            Some(p) => p.to_path_buf(),
            None => match std::env::var_os("ARCDECK_CONFIG") {
                Some(p) if !p.is_empty() => PathBuf::from(p),
                _ => config_dir().join("arcdeck").join("arcdeck.json"),
            },
        };
        if !path.is_file() {
            return Err(SpecError::new(
                code::CONFIG_MISSING,
                format!(
                    "找不到 arcdeck 配置：{}。用 `arcdeck config init` 生成一份，或用 --config / $ARCDECK_CONFIG 指定",
                    path.display()
                ),
            ));
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            SpecError::new(code::CONFIG_UNREADABLE, format!("{} —— {e}", path.display()))
        })?;
        Ok((Self::parse(&raw, &path)?, path))
    }

    pub fn dsh_version(&self) -> String {
        self.runtime.dsh_version.clone().unwrap_or_else(|| PINNED_DSH_VERSION.to_string())
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.runtime
            .dir
            .clone()
            .unwrap_or_else(|| default_runtime_dir(&self.dsh_version()))
    }

    pub fn state_root(&self) -> PathBuf {
        self.state_root.clone().unwrap_or_else(default_state_root)
    }
}

/// 起步配置。缺省路由取用户 2026-10-14 指定的 OpenCode Go 上的 DeepSeek V4.1 Flash。
pub fn starter_config() -> String {
    r#"{
  "model": "opencode-go/deepseek-v4.1-flash",
  "provider": {},
  "runtime": { "session_log_upload": false },
  "personalization": { "enabled": false },
  "worker": {
    "permission": "workspace",
    "allow_commit": false,
    "timeout_seconds": 14400,
    "max_attempts": 3
  },
  "env": {}
}
"#
    .to_string()
}

/// 配置目录：Windows `%APPDATA%`，其他 `$XDG_CONFIG_HOME` 或 `~/.config`。
pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("APPDATA") {
            return PathBuf::from(p);
        }
    }
    if let Some(p) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    home_dir().join(".config")
}

/// 数据目录：Windows `%APPDATA%`（与 auth.json 同处，便于找），其他 `$XDG_DATA_HOME` 或 `~/.local/share`。
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("APPDATA") {
            return PathBuf::from(p);
        }
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    home_dir().join(".local").join("share")
}

/// 本机大文件目录：Windows `%LOCALAPPDATA%`（不漫游 —— 运行时约 550 MB），其他同 [`data_dir`]。
pub fn local_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(p);
        }
    }
    data_dir()
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn auth_path() -> PathBuf {
    data_dir().join("arcdeck").join("auth.json")
}

pub fn default_state_root() -> PathBuf {
    local_data_dir().join("arcdeck").join("state")
}

pub fn default_runtime_dir(version: &str) -> PathBuf {
    local_data_dir().join("arcdeck").join("runtime").join(format!("dsh-{version}"))
}

/// OpenCode 的 auth.json 位置（用于 `arcdeck auth import-opencode`）。
pub fn opencode_auth_path() -> PathBuf {
    if let Some(p) = std::env::var_os("XDG_DATA_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p).join("opencode").join("auth.json");
    }
    home_dir().join(".local").join("share").join("opencode").join("auth.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_config_parses() {
        let c = UserConfig::parse(&starter_config(), Path::new("x")).unwrap();
        assert_eq!(c.model, "opencode-go/deepseek-v4.1-flash");
        assert_eq!(c.worker.max_attempts, 3);
        assert!(!c.runtime.session_log_upload);
        assert_eq!(c.dsh_version(), PINNED_DSH_VERSION);
        // 个性化默认关 —— 一般模式下一个字都不注入（用户 2026-10-14）。
        assert!(!c.personalization.enabled);
    }

    /// 授权级别是**闭集**：拼错要当场报错，不能静默退回默认值。
    /// （`deny_unknown_fields` 管不到枚举的取值，所以这条测试是必需的。）
    #[test]
    fn unknown_permission_is_rejected() {
        let raw = starter_config().replace("\"permission\": \"workspace\"", "\"permission\": \"yolo\"");
        let err = UserConfig::parse(&raw, Path::new("x")).unwrap_err();
        assert!(err.message.contains("yolo"), "报错要指出错在哪：{err}");
    }

    #[test]
    fn permission_maps_to_the_dsh_sandbox_and_profile() {
        use crate::authz::Permission;
        assert_eq!(Permission::Readonly.sandbox(), "read-only");
        assert_eq!(Permission::Readonly.profile(), "workspace-readonly");
        assert_eq!(Permission::Workspace.sandbox(), "workspace-write");
        assert_eq!(Permission::Full.sandbox(), "danger-full-access");
        // confirm 与 workspace 同权限，差别只在「要不要有人应答」。
        assert_eq!(Permission::Confirm.sandbox(), "workspace-write");
        assert!(Permission::Confirm.needs_responder());
        assert!(!Permission::Workspace.needs_responder());
        // 缺省是「工作区内直接改、直接跑」——与 TUI 的默认一致。
        assert_eq!(Permission::default(), Permission::Workspace);
    }

    #[test]
    fn model_without_provider_is_rejected() {
        let raw = starter_config().replace("opencode-go/deepseek-v4.1-flash", "deepseek-v4.1-flash");
        assert!(UserConfig::parse(&raw, Path::new("x")).is_err());
    }
}
