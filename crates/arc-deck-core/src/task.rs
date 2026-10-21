//! 任务书 v1（PRODUCT_PLAN §6.2）。
//!
//! 任务书是**主控**写给 worker 的：它只能**收紧**启动配置给出的授权上限，
//! 不能自己声明更大的权限（见 [`crate::authz`]）。
//!
//! 结构体一律 `deny_unknown_fields`。代价是加字段会破坏旧文件，收益是
//! 拼错的策略名会当场报错 —— 而「`write_path` 少了个 s」被静默忽略，
//! 结果就是带着错误授权跑完整个任务。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{code, SpecError, SpecResult};
use crate::mode::Mode;
use crate::paths;
use crate::SCHEMA_VERSION;

/// 任务书。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub schema_version: u32,
    pub task_id: String,
    /// 绝对路径，或相对**任务书所在目录**的路径。
    pub workspace: PathBuf,
    /// 省略时为 `echo`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// 省略时取该 Agent 的默认模式。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// 谁派的这件任务（`claude` / `sol`，或某个任务 id）。缺席 = 没记录。
    ///
    /// 这**不是授权**，是派工关系的一条边。为什么是自由文本而不是 `parent_task_id`：
    /// Saiku 里几乎全是工人，树的上两层（Claude Code、Codex）**不是 Saiku
    /// 任务**，没有自己的 `status.json` —— 记任务 id 会指向一个查不到的东西。
    /// 用途是让外部监控程序画「任务书流向」，参 PRODUCT_PLAN §6.2 与 §6.5.1。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_by: Option<String>,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<TaskConstraints>,
    /// 省略等价于 `[]`；每项都是必需项（v1 没有可选验收）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<Acceptance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

/// 任务对授权的收紧请求。字段全是可选的：缺席表示「不额外收紧」。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConstraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_paths: Option<Vec<PathBuf>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_paths: Option<Vec<PathBuf>>,
    /// 请求的策略档；必须不高于启动配置的档位（见 [`crate::authz::Profile`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_commit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_push: Option<bool>,
}

/// 一条验收条件。
///
/// v1 只有两类，且**每项都必需**：没有 `required` 字段，省略 `acceptance`
/// 才是「没有额外交付门槛」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Acceptance {
    /// 由 DSH 工具链实际执行过的命令；证据取自工具执行记录，不另起 verifier。
    Command {
        id: String,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_seconds: Option<u64>,
    },
    /// 需要主控或人工判断，同步调用里只能记 `pending` 并返回 blocked。
    Manual { id: String, description: String },
}

impl Acceptance {
    pub fn id(&self) -> &str {
        match self {
            Acceptance::Command { id, .. } | Acceptance::Manual { id, .. } => id,
        }
    }

    pub fn is_command(&self) -> bool {
        matches!(self, Acceptance::Command { .. })
    }
}

impl TaskSpec {
    /// 从 JSON 文本解析并校验。
    ///
    /// `origin_dir` 是任务书所在目录，用于解析相对 `workspace`。
    pub fn parse(raw: &str, origin_dir: &Path) -> SpecResult<Self> {
        let mut task: TaskSpec =
            serde_json::from_str(raw).map_err(|e| SpecError::task_unreadable("<memory>", e))?;
        task.workspace = paths::resolve_against(origin_dir, &task.workspace);
        task.validate()?;
        Ok(task)
    }

    /// 从文件读取。相对路径以该文件所在目录为基准。
    pub fn load(path: &Path) -> SpecResult<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| SpecError::task_unreadable(path.display(), e))?;
        // 同配置：相对基准先锚成绝对路径，否则 `../..` 会解析出相对结果。
        let origin = paths::absolutize(path);
        let origin = origin.parent().unwrap_or_else(|| Path::new("."));
        let mut task: TaskSpec = serde_json::from_str(&raw)
            .map_err(|e| SpecError::task_unreadable(path.display(), e))?;
        task.workspace = paths::resolve_against(origin, &task.workspace);
        task.validate()?;
        Ok(task)
    }

    pub fn validate(&self) -> SpecResult<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(SpecError::new(
                code::TASK_VERSION_UNSUPPORTED,
                format!(
                    "任务书 schema_version={}，本客户端只支持 {}",
                    self.schema_version, SCHEMA_VERSION
                ),
            ));
        }
        validate_id(&self.task_id, "task_id")?;
        if self.instruction.trim().is_empty() {
            return Err(SpecError::new(code::TASK_INVALID, "任务书 instruction 不得为空"));
        }
        if !self.workspace.is_absolute() {
            return Err(SpecError::new(
                code::TASK_INVALID,
                format!("workspace 解析后仍是相对路径：{}", self.workspace.display()),
            ));
        }
        if let Some(0) = self.timeout_seconds {
            return Err(SpecError::new(code::TASK_INVALID, "timeout_seconds 必须大于 0"));
        }

        let mut seen = BTreeSet::new();
        for acceptance in &self.acceptance {
            validate_id(acceptance.id(), "acceptance.id")?;
            if !seen.insert(acceptance.id()) {
                return Err(SpecError::new(
                    code::TASK_INVALID,
                    format!("acceptance id 重复：`{}`", acceptance.id()),
                ));
            }
            match acceptance {
                Acceptance::Command { command, timeout_seconds, .. } => {
                    if command.trim().is_empty() {
                        return Err(SpecError::new(
                            code::TASK_INVALID,
                            format!("acceptance `{}` 的 command 不得为空", acceptance.id()),
                        ));
                    }
                    if let Some(0) = timeout_seconds {
                        return Err(SpecError::new(
                            code::TASK_INVALID,
                            format!("acceptance `{}` 的 timeout_seconds 必须大于 0", acceptance.id()),
                        ));
                    }
                }
                Acceptance::Manual { description, .. } => {
                    if description.trim().is_empty() {
                        return Err(SpecError::new(
                            code::TASK_INVALID,
                            format!("acceptance `{}` 的 description 不得为空", acceptance.id()),
                        ));
                    }
                }
            }
        }

        if let Some(mode) = self.mode {
            // Agent 是否合法由 AgentId::parse 负责；这里只做「两者同时给出」的早检。
            if let Some(raw_agent) = &self.agent {
                crate::mode::AgentMode::resolve(
                    Some(crate::mode::AgentId::parse(raw_agent)?),
                    Some(mode),
                )?;
            }
        }
        Ok(())
    }
}

/// `task_id` / 验收 ID 的字符集。刻意收窄：这些值会进文件名（状态记录）与
/// 命令行输出，放宽到任意 Unicode 会把路径与日志都变成不可读的东西。
fn validate_id(value: &str, field: &str) -> SpecResult<()> {
    if value.is_empty() {
        return Err(SpecError::new(code::TASK_INVALID, format!("{field} 不得为空")));
    }
    if value.len() > 96 {
        return Err(SpecError::new(code::TASK_INVALID, format!("{field} 过长（>96）：{value:?}")));
    }
    let ok = value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !ok {
        return Err(SpecError::new(
            code::TASK_INVALID,
            format!("{field} 只允许 ASCII 字母数字与 `.` `_` `-`：{value:?}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "task_id": "t1",
            "workspace": ".",
            "instruction": "do it"
        })
    }

    fn origin() -> PathBuf {
        paths::test_abs("work")
    }

    #[test]
    fn minimal_task_parses() {
        let task = TaskSpec::parse(&base().to_string(), &origin()).unwrap();
        assert_eq!(task.task_id, "t1");
        assert!(task.agent.is_none());
        assert!(task.acceptance.is_empty());
        assert_eq!(task.workspace, origin());
    }

    #[test]
    fn version_mismatch_is_rejected() {
        let mut v = base();
        v["schema_version"] = serde_json::json!(2);
        let err = TaskSpec::parse(&v.to_string(), &origin()).unwrap_err();
        assert!(err.is(code::TASK_VERSION_UNSUPPORTED), "{err}");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let mut v = base();
        // 真实踩点：少写一个 s 的 write_path 必须报错，而不是静默忽略。
        v["constraints"] = serde_json::json!({ "write_path": ["src/"] });
        let err = TaskSpec::parse(&v.to_string(), &origin()).unwrap_err();
        assert!(err.is(code::TASK_UNREADABLE), "{err}");
    }

    #[test]
    fn duplicate_acceptance_id_is_rejected() {
        let mut v = base();
        v["acceptance"] = serde_json::json!([
            { "id": "a", "kind": "command", "command": "cargo test" },
            { "id": "a", "kind": "manual", "description": "look at it" }
        ]);
        let err = TaskSpec::parse(&v.to_string(), &origin()).unwrap_err();
        assert!(err.is(code::TASK_INVALID), "{err}");
    }

    #[test]
    fn conflicting_agent_mode_fails_at_parse() {
        let mut v = base();
        v["agent"] = serde_json::json!("plan");
        v["mode"] = serde_json::json!("build");
        let err = TaskSpec::parse(&v.to_string(), &origin()).unwrap_err();
        assert!(err.is(code::MODE_CONFLICT), "{err}");
    }

    #[test]
    fn bad_task_id_charset_is_rejected() {
        let mut v = base();
        v["task_id"] = serde_json::json!("a/b");
        let err = TaskSpec::parse(&v.to_string(), &origin()).unwrap_err();
        assert!(err.is(code::TASK_INVALID), "{err}");
    }

    #[test]
    fn empty_instruction_is_rejected() {
        let mut v = base();
        v["instruction"] = serde_json::json!("   ");
        let err = TaskSpec::parse(&v.to_string(), &origin()).unwrap_err();
        assert!(err.is(code::TASK_INVALID), "{err}");
    }

    #[test]
    fn zero_timeout_is_rejected() {
        let mut v = base();
        v["timeout_seconds"] = serde_json::json!(0);
        let err = TaskSpec::parse(&v.to_string(), &origin()).unwrap_err();
        assert!(err.is(code::TASK_INVALID), "{err}");
    }
}
