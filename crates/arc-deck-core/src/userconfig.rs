//! 用户级配置 `saiku.json` 与标准目录（PRODUCT_PLAN §6.0）。
//!
//! 查找顺序：`--config` → `$SAIKU_CONFIG` → `<配置目录>/saiku/saiku.json`。
//! **永远不从 workspace 里找** —— 任务书与工作区文件不得扩大启动时授权。
//!
//! 这份配置是调度者（主控）那一侧的东西：模型路由、供应商、沙箱档、是否允许提交、
//! worktree 模板。`saiku run` 把它与任务书一起物化成 M1 的启动配置与任务书，
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
    /// 缺省 **false**：一般模式下不注入任何 Saiku 文本。
    #[serde(default)]
    pub enabled: bool,
}

/// `saiku.json`。
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
    /// 任务状态目录；省略时 `<数据目录>/saiku/state`。
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
    /// 运行时目录；省略时 `<数据目录>/saiku/runtime/dsh-<版本>`。
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

/// 改过名的字段仍然**拒绝**（产品层只有一个说法，不收别名），但报错要说清改成什么。
/// 4e1eae6 把 `sandbox` 改名后，开发机上那份旧配置让第一次真派工直接 64 退出，
/// 报错只有 `expected one of …`，看不出旧值该对应哪一档。
fn renamed_field_hint(serde_error: &str) -> &'static str {
    if serde_error.contains("unknown field `sandbox`") {
        "\n  `worker.sandbox` 已改名为 `worker.permission`：read-only → readonly，\
         workspace-write → workspace，danger-full-access → full"
    } else if serde_error.contains("unknown field `reasoning_effort`") {
        "\n  `reasoning_effort` 已改名为 `effort`，取值不变"
    } else {
        ""
    }
}

impl UserConfig {
    pub fn parse(raw: &str, origin: &Path) -> SpecResult<Self> {
        let config: UserConfig = serde_json::from_str(raw).map_err(|e| {
            let e = e.to_string();
            SpecError::new(
                code::CONFIG_INVALID,
                format!("{} 不是合法的 saiku.json —— {e}{}", origin.display(), renamed_field_hint(&e)),
            )
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
            None => match std::env::var_os("SAIKU_CONFIG") {
                Some(p) if !p.is_empty() => PathBuf::from(p),
                _ => config_dir().join("saiku").join("saiku.json"),
            },
        };
        if !path.is_file() {
            return Err(SpecError::new(
                code::CONFIG_MISSING,
                format!(
                    "找不到 saiku 配置：{}。用 `saiku config init` 生成一份，或用 --config / $SAIKU_CONFIG 指定",
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
    data_dir().join("saiku").join("auth.json")
}

pub fn default_state_root() -> PathBuf {
    local_data_dir().join("saiku").join("state")
}

pub fn default_runtime_dir(version: &str) -> PathBuf {
    local_data_dir().join("saiku").join("runtime").join(format!("dsh-{version}"))
}

/// 产品 2026-10-21 由 Arc Deck 改名为 Saiku（`arcdeck` 这个名字已有人用）。改名前的
/// 安装把配置、密钥、运行时、会话都放在 `<基目录>/arcdeck` 下 —— 不搬的话升级后
/// 「找不到配置」「密钥缺失」，而且旧目录名正好撞上那个同名产品。
const LEGACY_APP_DIR: &str = "arcdeck";

/// 把旧名目录一次性搬到 `saiku` 下。只在新目录**还不存在**时搬：新旧都在说明用户
/// 已经手动处理过，这时不猜谁新谁旧。返回给用户看的提示，每条一行。
///
/// 搬不动（典型是 Windows 上旧版 `arcdeck` 还开着、文件被占用）返回 `Err` ——
/// 调用方不能继续：继续跑会建出空的新目录，下次就再也不会迁移了。
pub fn migrate_legacy_dirs() -> Result<Vec<String>, String> {
    let mut bases = vec![config_dir(), data_dir(), local_data_dir()];
    bases.dedup();
    migrate_legacy_in(&bases)
}

fn migrate_legacy_in(bases: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for base in bases {
        let old = base.join(LEGACY_APP_DIR);
        let new = base.join("saiku");
        if !old.is_dir() || new.exists() {
            continue;
        }
        std::fs::rename(&old, &new).map_err(|e| {
            format!(
                "旧目录 {} 没能改名为 {}：{e}。先关掉所有正在运行的 arcdeck（含 TUI 与工人）再重试",
                old.display(),
                new.display()
            )
        })?;
        notes.push(format!("已把 {} 迁到 {}", old.display(), new.display()));
        // 目录里以旧名命名的文件：配置本身，以及交互会话目录里的锁、内核 pid 与补丁标记
        //（launch.rs 按新名字找它们，留着旧名就认不出上一个进程）。
        for (dir, from, to) in [
            (new.clone(), "arcdeck.json", "saiku.json"),
            (new.join("dsh-home"), "arcdeck.lock", "saiku.lock"),
            (new.join("dsh-home"), "arcdeck.kernel", "saiku.kernel"),
            (new.join("dsh-home"), "arcdeck-web-patch.json", "saiku-web-patch.json"),
        ] {
            let (from, to) = (dir.join(from), dir.join(to));
            if from.is_file() && !to.exists() {
                std::fs::rename(&from, &to).map_err(|e| format!("{} 没能改名：{e}", from.display()))?;
            }
        }
    }
    Ok(notes)
}

/// OpenCode 的 auth.json 位置（用于 `saiku auth import-opencode`）。
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
    fn legacy_arcdeck_dirs_move_once_and_never_over_a_new_one() {
        let roaming = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let old = roaming.path().join("arcdeck");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("arcdeck.json"), "{}").unwrap();
        std::fs::write(old.join("auth.json"), "{}").unwrap();
        let old_home = local.path().join("arcdeck").join("dsh-home");
        std::fs::create_dir_all(&old_home).unwrap();
        std::fs::write(old_home.join("arcdeck.lock"), "1").unwrap();

        let bases = [roaming.path().to_path_buf(), local.path().to_path_buf()];
        let notes = migrate_legacy_in(&bases).unwrap();
        assert_eq!(notes.len(), 2, "{notes:?}");
        let new = roaming.path().join("saiku");
        assert!(new.join("saiku.json").is_file() && new.join("auth.json").is_file());
        assert!(!old.exists());
        assert!(local.path().join("saiku").join("dsh-home").join("saiku.lock").is_file());

        // 新旧并存时不动：用户已经自己处理过，不猜谁新谁旧。
        std::fs::create_dir_all(&old).unwrap();
        assert!(migrate_legacy_in(&bases).unwrap().is_empty());
        assert!(old.exists());
    }

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

    /// 旧字段名仍被拒绝，但报错给出新名字与取值对应 —— 不是静默接受成别名。
    #[test]
    fn renamed_fields_are_rejected_with_the_new_name() {
        let raw = starter_config().replace("\"permission\": \"workspace\"", "\"sandbox\": \"workspace-write\"");
        let err = UserConfig::parse(&raw, Path::new("x")).unwrap_err();
        assert!(err.message.contains("worker.permission") && err.message.contains("workspace-write → workspace"), "{err}");

        let raw = starter_config().replacen('{', "{\n  \"reasoning_effort\": \"high\",", 1);
        let err = UserConfig::parse(&raw, Path::new("x")).unwrap_err();
        assert!(err.message.contains("改名为 `effort`"), "{err}");
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
