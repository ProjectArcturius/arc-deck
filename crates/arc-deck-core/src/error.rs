//! 规格错误的统一形状。
//!
//! worker 的失败分两类，混在一起会让「任务没跑起来」和「任务跑了但失败」看起来
//! 一样：**规格错误**（任务书/配置/Agent 定义有问题，退出码 `64`）与**任务终态**
//! （`TaskStatus`，退出码 `0..=5`）。这里只表达前者。
//!
//! `code` 是给脚本判断用的稳定标识，`message` 给人读；两者都不许为空。
//! 参 PRODUCT_PLAN §6.1/§6.2。

use serde::{Deserialize, Serialize};

/// 机器可读的错误码。字符串保持稳定，脚本按它分支。
pub mod code {
    pub const CONFIG_MISSING: &str = "CONFIG_MISSING";
    pub const CONFIG_UNREADABLE: &str = "CONFIG_UNREADABLE";
    pub const CONFIG_VERSION_UNSUPPORTED: &str = "CONFIG_VERSION_UNSUPPORTED";
    pub const CONFIG_INVALID: &str = "CONFIG_INVALID";
    pub const RUNTIME_NOT_SUPPORTED: &str = "RUNTIME_NOT_SUPPORTED";
    pub const RUNTIME_ENTRY_MISSING: &str = "RUNTIME_ENTRY_MISSING";
    pub const PROFILE_UNKNOWN: &str = "PROFILE_UNKNOWN";

    pub const TASK_UNREADABLE: &str = "TASK_UNREADABLE";
    pub const TASK_VERSION_UNSUPPORTED: &str = "TASK_VERSION_UNSUPPORTED";
    pub const TASK_INVALID: &str = "TASK_INVALID";
    pub const TASK_ALREADY_SEEN: &str = "TASK_ALREADY_SEEN";

    pub const AUTH_WORKSPACE_MISMATCH: &str = "AUTH_WORKSPACE_MISMATCH";
    pub const AUTH_EXCEEDED: &str = "AUTH_EXCEEDED";
    /// 授权本身解析得出来，但**执行层表达不了** —— 拒绝启动，不静默放宽。
    ///
    /// 与 `AUTH_EXCEEDED` 分开是因为方向不同：那个是「你要的比允许的多」，
    /// 这个是「你允许的，DSH 保证不了」（参 docs/M3_GUARD.md）。
    pub const AUTH_GUARD_UNSUPPORTED: &str = "AUTH_GUARD_UNSUPPORTED";
    pub const AGENT_UNKNOWN: &str = "AGENT_UNKNOWN";
    pub const AGENT_NOT_AUTHORIZED: &str = "AGENT_NOT_AUTHORIZED";
    pub const MODE_CONFLICT: &str = "MODE_CONFLICT";
    pub const ACCEPTANCE_NOT_ALLOWED: &str = "ACCEPTANCE_NOT_ALLOWED";

    pub const AGENT_DIR_MISSING: &str = "AGENT_DIR_MISSING";
    pub const AGENT_PARSE: &str = "AGENT_PARSE";
    pub const AGENT_DUPLICATE_ID: &str = "AGENT_DUPLICATE_ID";
    pub const AGENT_UNKNOWN_TOOL: &str = "AGENT_UNKNOWN_TOOL";
    pub const AGENT_PROFILE_UNKNOWN: &str = "AGENT_PROFILE_UNKNOWN";
    pub const AGENT_REF_MISSING: &str = "AGENT_REF_MISSING";
    pub const AGENT_UNSUPPORTED: &str = "AGENT_UNSUPPORTED";
}

/// 规格错误：任务书、启动配置或 Agent 定义不合法。
///
/// 刻意不实现 `std::error::Error` 之外的便利转换 —— 调用点必须显式给出 `code`，
/// 免得所有错误都退化成一句自由文本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct SpecError {
    /// 机器可读错误码，见 [`code`]。
    pub code: String,
    /// 面向人的说明；必须能让人不看源码就知道哪一处不合法。
    pub message: String,
}

impl SpecError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }

    pub fn config_missing(path: impl std::fmt::Display) -> Self {
        Self::new(code::CONFIG_MISSING, format!("启动配置不存在：{path}（--config 不接受默认值）"))
    }

    pub fn task_unreadable(path: impl std::fmt::Display, why: impl std::fmt::Display) -> Self {
        Self::new(code::TASK_UNREADABLE, format!("任务书无法读取或不是合法 JSON：{path} —— {why}"))
    }

    pub fn is(&self, code: &str) -> bool {
        self.code == code
    }
}

/// 便于 `?` 传播的别名。
pub type SpecResult<T> = Result<T, SpecError>;
