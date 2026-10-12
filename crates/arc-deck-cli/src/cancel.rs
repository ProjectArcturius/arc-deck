//! 外部取消入口。
//!
//! SDK **没有单会话取消**（`REFS/deepseek-harness` 的 SDK 协议 README 原文：
//! 「No cancel or session-close methods」），所以跨平台的取消只能由主控
//! 通过一个控制文件提出，worker 观察到之后自己做有界收尾。
//!
//! 用文件而不是信号：Windows 的无人值守脚本给不了所有进程 Ctrl+C，
//! 而 `--cancel-file` 在两端的行为完全一样。

use std::path::{Path, PathBuf};

use arc_deck_core::error::{code, SpecError, SpecResult};

/// 一次轮询的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelSignal {
    None,
    /// 请求取消，带主控写下的原因（可为空）。
    Requested(String),
    /// 文件内容不是合法请求。**不触发取消**，只报一次诊断。
    Invalid(String),
}

/// 观察取消文件。
#[derive(Debug)]
pub struct CancelWatcher {
    path: Option<PathBuf>,
    task_id: String,
    reported_invalid: bool,
}

impl CancelWatcher {
    pub fn new(path: Option<PathBuf>, task_id: &str) -> Self {
        Self { path, task_id: task_id.to_string(), reported_invalid: false }
    }

    /// 启动前检查：取消文件在启动时**必须不存在**。
    ///
    /// 允许它预先存在的话，上一轮留下的文件会让这一次一启动就取消 ——
    /// 而那种失败看起来像「worker 坏了」。
    pub fn preflight(&self) -> SpecResult<()> {
        if let Some(path) = &self.path {
            if path.exists() {
                return Err(SpecError::new(
                    code::CONFIG_INVALID,
                    format!(
                        "--cancel-file `{}` 在启动时已经存在；请让主控在需要取消时才创建它",
                        path.display()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 轮询一次。文件不存在就是「还没请求」。
    pub fn poll(&mut self) -> CancelSignal {
        let Some(path) = &self.path else {
            return CancelSignal::None;
        };
        if !path.exists() {
            return CancelSignal::None;
        }
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) => return self.invalid(format!("取消文件不可读：{e}")),
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return self.invalid("取消文件不是合法 JSON".to_string());
        };
        if value.get("task_id").and_then(|v| v.as_str()) != Some(self.task_id.as_str()) {
            return CancelSignal::None;
        }
        if value.get("action").and_then(|v| v.as_str()) != Some("cancel") {
            return self.invalid("取消文件的 action 不是 \"cancel\"".to_string());
        }
        let reason = value
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("主控请求取消")
            .to_string();
        CancelSignal::Requested(reason)
    }

    fn invalid(&mut self, detail: String) -> CancelSignal {
        // 同一份坏内容只报一次，避免每 100 ms 往事件流里灌一条同样的诊断。
        if self.reported_invalid {
            return CancelSignal::None;
        }
        self.reported_invalid = true;
        CancelSignal::Invalid(detail)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_file_means_no_signal() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = CancelWatcher::new(Some(dir.path().join("cancel.json")), "t1");
        assert!(w.preflight().is_ok());
        assert_eq!(w.poll(), CancelSignal::None);
    }

    #[test]
    fn preflight_rejects_a_stale_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.json");
        std::fs::write(&path, "{}").unwrap();
        let w = CancelWatcher::new(Some(path), "t1");
        let err = w.preflight().unwrap_err();
        assert!(err.is(code::CONFIG_INVALID), "{err}");
    }

    #[test]
    fn matching_request_is_honoured() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.json");
        let mut w = CancelWatcher::new(Some(path.clone()), "t1");
        std::fs::write(
            &path,
            r#"{"schema_version":1,"task_id":"t1","action":"cancel","reason":"不要了"}"#,
        )
        .unwrap();
        assert_eq!(w.poll(), CancelSignal::Requested("不要了".to_string()));
    }

    #[test]
    fn other_task_id_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.json");
        let mut w = CancelWatcher::new(Some(path.clone()), "t1");
        std::fs::write(&path, r#"{"task_id":"t2","action":"cancel"}"#).unwrap();
        assert_eq!(w.poll(), CancelSignal::None);
    }

    #[test]
    fn malformed_content_is_reported_once_and_does_not_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.json");
        let mut w = CancelWatcher::new(Some(path.clone()), "t1");
        std::fs::write(&path, "not json").unwrap();
        assert!(matches!(w.poll(), CancelSignal::Invalid(_)));
        // 第二次不再重复报，但依然不取消。
        assert_eq!(w.poll(), CancelSignal::None);
    }

    #[test]
    fn wrong_action_is_not_a_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.json");
        let mut w = CancelWatcher::new(Some(path.clone()), "t1");
        std::fs::write(&path, r#"{"task_id":"t1","action":"pause"}"#).unwrap();
        assert!(matches!(w.poll(), CancelSignal::Invalid(_)));
    }
}
