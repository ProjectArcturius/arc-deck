//! worker 事件流（PRODUCT_PLAN §6.3）。
//!
//! 信封固定为 `schema_version / task_id / seq / type / data`，`seq` 从 1 递增。
//! 形状刻意保持最小：主控只需要「现在到哪一步了」和「跑过什么」，
//! 而**逐 token 的输出不在这里** —— SDK 没有当前 attempt 的实时通道，
//! 假装有会得到一个稳定但一直为空的字段。
//!
//! 事件流是控制面：它走 stdout，而 stdout 只允许出现协议行。

use serde::{Deserialize, Serialize};

use crate::result::TaskResult;
use crate::SCHEMA_VERSION;

/// 产品级阶段。**不**直接等同 DSH 的某一帧：一次 `turn/end` 可能对应
/// 从 `running` 走到 `verifying`，而 `starting` 在离线流程里可能只有几毫秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Accepted,
    Starting,
    Running,
    Verifying,
    Terminal,
}

impl WorkerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkerState::Accepted => "accepted",
            WorkerState::Starting => "starting",
            WorkerState::Running => "running",
            WorkerState::Verifying => "verifying",
            WorkerState::Terminal => "terminal",
        }
    }
}

/// 事件类型名。字符串是公开契约的一部分，脚本会按它分支。
pub mod event_type {
    pub const STATE: &str = "state";
    pub const TOOL_CALL: &str = "tool_call";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const DIAGNOSTIC: &str = "diagnostic";
    pub const RESULT: &str = "result";
}

/// 一条事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub task_id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: serde_json::Value,
}

impl EventEnvelope {
    pub fn new(task_id: &str, seq: u64, kind: &str, data: serde_json::Value) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            task_id: task_id.to_string(),
            seq,
            kind: kind.to_string(),
            data,
        }
    }

    pub fn state(task_id: &str, seq: u64, state: WorkerState, detail: &str) -> Self {
        Self::new(
            task_id,
            seq,
            event_type::STATE,
            serde_json::json!({ "state": state.as_str(), "detail": detail }),
        )
    }

    pub fn tool_call(task_id: &str, seq: u64, call_id: &str, tool: &str, summary: &str) -> Self {
        Self::new(
            task_id,
            seq,
            event_type::TOOL_CALL,
            serde_json::json!({ "call_id": call_id, "tool": tool, "summary": summary }),
        )
    }

    pub fn tool_result(
        task_id: &str,
        seq: u64,
        call_id: &str,
        tool: &str,
        status: &str,
        summary: &str,
    ) -> Self {
        Self::new(
            task_id,
            seq,
            event_type::TOOL_RESULT,
            serde_json::json!({
                "call_id": call_id,
                "tool": tool,
                "status": status,
                "summary": summary
            }),
        )
    }

    pub fn diagnostic(task_id: &str, seq: u64, level: &str, code: Option<&str>, message: &str) -> Self {
        let mut data = serde_json::json!({ "level": level, "message": message });
        if let Some(code) = code {
            data["code"] = serde_json::Value::String(code.to_string());
        }
        Self::new(task_id, seq, event_type::DIAGNOSTIC, data)
    }

    pub fn result(task_id: &str, seq: u64, result: &TaskResult) -> Self {
        let data = serde_json::to_value(result).unwrap_or_else(|e| {
            // 结果本身序列化失败意味着契约已破，用一个显式的哨兵代替静默丢事件。
            serde_json::json!({ "serialize_error": e.to_string() })
        });
        Self::new(task_id, seq, event_type::RESULT, data)
    }

    /// 序列化成一行 JSONL（含换行）。
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"schema_version":1,"error":"event serialization failed"}"#.to_string()
        });
        line.push('\n');
        line
    }
}

/// 事件序列号分配器。
///
/// 独立成一个类型是为了让「seq 从 1 开始且只在这里递增」成为唯一事实 ——
/// 在别处手写 `seq + 1` 迟早会漏掉某条事件或写出重复序号。
#[derive(Debug, Default)]
pub struct SeqCounter {
    next: u64,
}

impl SeqCounter {
    pub fn new() -> Self {
        Self { next: 0 }
    }

    pub fn next(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    pub fn current(&self) -> u64 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_starts_at_one_and_increments() {
        let mut c = SeqCounter::new();
        assert_eq!(c.next(), 1);
        assert_eq!(c.next(), 2);
        assert_eq!(c.current(), 2);
    }

    #[test]
    fn envelope_has_fixed_envelope_fields() {
        let ev = EventEnvelope::state("t1", 1, WorkerState::Running, "started");
        let line = ev.to_line();
        assert!(line.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["task_id"], "t1");
        assert_eq!(v["seq"], 1);
        assert_eq!(v["type"], "state");
        assert_eq!(v["data"]["state"], "running");
    }

    #[test]
    fn diagnostic_carries_code_only_when_present() {
        let with = EventEnvelope::diagnostic("t", 1, "error", Some("PROTOCOL_ERROR"), "bad frame");
        assert_eq!(with.data["code"], "PROTOCOL_ERROR");
        let without = EventEnvelope::diagnostic("t", 2, "info", None, "fine");
        assert!(without.data.get("code").is_none());
    }
}
