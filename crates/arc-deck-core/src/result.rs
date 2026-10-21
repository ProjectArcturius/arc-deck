//! 最终结果与退出码（PRODUCT_PLAN §6.3）。
//!
//! 为什么把「模型的完成说明」和「程序采集的证据」分成两个字段：模型说
//! 「已修复并测试通过」是一句陈述，不是证据。主控要能只看 `verification`
//! 就判断任务是否真的满足交付条件，而不必相信一段自然语言。

use serde::{Deserialize, Serialize};

use crate::SCHEMA_VERSION;

/// 任务终态。取值空间与 [`crate::EXIT_SPEC_ERROR`]（`64`）刻意分开。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 已确认终止，且所有验收为 passed。
    Completed,
    /// 跑过，但内核失败或验收 failed。
    Failed,
    /// 跑过，但需要主控/人工介入才能继续（含 pending 验收）。
    Blocked,
    /// 收到并确认了取消请求。
    Cancelled,
    /// 超过墙钟预算。
    TimedOut,
    /// 缺失 terminal，无法判断任务结果（例如运行时提前退出）。
    Unknown,
}

impl TaskStatus {
    pub const fn exit_code(self) -> i32 {
        match self {
            TaskStatus::Completed => 0,
            TaskStatus::Failed => 1,
            TaskStatus::Blocked => 2,
            TaskStatus::Cancelled => 3,
            TaskStatus::TimedOut => 4,
            TaskStatus::Unknown => 5,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Cancelled => "cancelled",
            TaskStatus::TimedOut => "timed_out",
            TaskStatus::Unknown => "unknown",
        }
    }

    pub const fn is_terminal_for_state_root(self) -> bool {
        // 六个状态都是终态：状态目录里存的是「跑过一次的结论」。
        true
    }
}

/// 验收类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationKind {
    Command,
    Manual,
}

/// 单条验收的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    /// 观察到真实执行且退出 0。
    Passed,
    /// 观察到真实执行且退出非 0。
    Failed,
    /// 需要主控/人工判断。
    Pending,
    /// 没有观察到对应的执行记录。
    NotRun,
}

/// 一条验收的证据。`evidence` 指向完整输出所在位置，不塞大段文本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRecord {
    pub id: String,
    pub kind: VerificationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    pub outcome: VerificationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 相对基线的一处文件变化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub path: String,
    /// `added` / `modified` / `deleted` / `unknown`。
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// 阻塞项：主控据此决定补任务还是人工处理。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocker {
    pub code: String,
    pub message: String,
}

/// 受控的产物引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub kind: String,
    pub path: String,
}

/// provider 实际报告的用量。缺数据就整个字段缺席，不留一个假的 0。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// provider 报告的缓存命中输入 token。省钱与否看这个数，不看估算。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
}

/// 最终结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResult {
    pub schema_version: u32,
    pub task_id: String,
    pub status: TaskStatus,
    /// 模型撰写的摘要。**是陈述，不是证据**；判定请看 `verification`。
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    /// 自然退出时才记录；主动收树的退出码不冒充运行时自己的结论。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<ChangeRecord>,
    /// Saiku 自己从 git 采集的提交（`<sha> <标题>`），相对启动配置的 `git_base`。
    /// 不是模型自报。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verification: Vec<VerificationRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unverified: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<Blocker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// 生效的授权与执行上下文，便于事后核对「它当时到底被允许做什么」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<crate::authz::EffectivePolicy>,
}

impl TaskResult {
    pub fn new(task_id: &str, status: TaskStatus, summary: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            task_id: task_id.to_string(),
            status,
            summary: summary.into(),
            session_id: None,
            runtime_version: None,
            runtime_exit_code: None,
            changes: Vec::new(),
            commits: Vec::new(),
            verification: Vec::new(),
            unverified: Vec::new(),
            blockers: Vec::new(),
            artifacts: Vec::new(),
            usage: None,
            policy: None,
        }
    }

    pub fn blocker(&mut self, code: &str, message: impl Into<String>) {
        self.blockers.push(Blocker { code: code.to_string(), message: message.into() });
    }

    /// 由验收结论推导终态。
    ///
    /// 规则（PRODUCT_PLAN §6.3）：
    /// - 全部 passed（或没有验收）→ 内核若正常终止则 completed
    /// - 任一 failed → failed
    /// - 任一 pending / not_run → blocked
    ///
    /// `kernel_completed = false` 时调用方**不要**用这个函数：内核没问题才轮到
    /// 验收说话。把两者混在一起会得到「验收通过所以任务成功」这种错误结论。
    pub fn derive_status_from_verification(&self) -> TaskStatus {
        if self.verification.iter().any(|v| v.outcome == VerificationOutcome::Failed) {
            return TaskStatus::Failed;
        }
        if self
            .verification
            .iter()
            .any(|v| matches!(v.outcome, VerificationOutcome::Pending | VerificationOutcome::NotRun))
        {
            return TaskStatus::Blocked;
        }
        TaskStatus::Completed
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| format!("{{\"serialize_error\":\"{e}\"}}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_the_published_table() {
        assert_eq!(TaskStatus::Completed.exit_code(), 0);
        assert_eq!(TaskStatus::Failed.exit_code(), 1);
        assert_eq!(TaskStatus::Blocked.exit_code(), 2);
        assert_eq!(TaskStatus::Cancelled.exit_code(), 3);
        assert_eq!(TaskStatus::TimedOut.exit_code(), 4);
        assert_eq!(TaskStatus::Unknown.exit_code(), 5);
        assert_eq!(crate::EXIT_SPEC_ERROR, 64);
    }

    fn record(outcome: VerificationOutcome) -> VerificationRecord {
        VerificationRecord {
            id: "a".into(),
            kind: VerificationKind::Command,
            command: Some("cargo test".into()),
            cwd: None,
            exit_code: None,
            evidence: None,
            outcome,
            detail: None,
        }
    }

    #[test]
    fn verification_derives_status() {
        let mut r = TaskResult::new("t", TaskStatus::Unknown, "");
        assert_eq!(r.derive_status_from_verification(), TaskStatus::Completed);

        r.verification = vec![record(VerificationOutcome::Passed)];
        assert_eq!(r.derive_status_from_verification(), TaskStatus::Completed);

        r.verification = vec![record(VerificationOutcome::Pending)];
        assert_eq!(r.derive_status_from_verification(), TaskStatus::Blocked);

        r.verification = vec![record(VerificationOutcome::NotRun)];
        assert_eq!(r.derive_status_from_verification(), TaskStatus::Blocked);

        r.verification = vec![record(VerificationOutcome::Failed)];
        assert_eq!(r.derive_status_from_verification(), TaskStatus::Failed);
    }

    #[test]
    fn failed_wins_over_pending() {
        let mut r = TaskResult::new("t", TaskStatus::Unknown, "");
        r.verification = vec![record(VerificationOutcome::Pending), record(VerificationOutcome::Failed)];
        assert_eq!(r.derive_status_from_verification(), TaskStatus::Failed);
    }

    #[test]
    fn absent_optionals_are_omitted() {
        let r = TaskResult::new("t", TaskStatus::Completed, "done");
        let v: serde_json::Value = serde_json::from_str(&r.to_json_pretty()).unwrap();
        assert!(v.get("usage").is_none());
        assert!(v.get("verification").is_none());
        assert!(v.get("session_id").is_none());
    }
}
