//! 事件输出与结果落盘。
//!
//! 两条纪律：
//!
//! - **stdout 只输出协议**。诊断走 stderr；而 stdout 上的每一行都必须是
//!   合法 JSONL，不能夹带半句人话。
//! - **stdout 断了不等于任务成功也不等于任务失败**。消费端断开时转成一次
//!   取消请求（参 PRODUCT_PLAN §6.4），并把结果尽量写进 `--result`。

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use arc_deck_core::event::{EventEnvelope, SeqCounter, WorkerState};
use arc_deck_core::result::TaskResult;

/// 一次写入的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Ok,
    /// stdout 消费端已经不在了。后续事件只进原始日志。
    StdoutBroken,
}

/// 事件写出器：同时维护 stdout 的 JSONL 与状态目录里的原始副本。
pub struct EventSink {
    task_id: String,
    seq: SeqCounter,
    raw: Option<BufWriter<File>>,
    stdout_alive: bool,
}

impl EventSink {
    pub fn new(task_id: &str, raw_path: Option<PathBuf>) -> Self {
        let raw = raw_path.and_then(|path| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(BufWriter::new)
        });
        Self {
            task_id: task_id.to_string(),
            seq: SeqCounter::new(),
            raw,
            stdout_alive: true,
        }
    }

    /// stdout 是否还活着。断了之后调用方应转成取消。
    pub fn stdout_alive(&self) -> bool {
        self.stdout_alive
    }

    pub fn emit(&mut self, mut envelope: EventEnvelope) -> WriteOutcome {
        envelope.seq = self.seq.next();
        let line = envelope.to_line();
        if let Some(raw) = self.raw.as_mut() {
            // 原始副本失败不改变任务结论：它是证据，不是判据。
            let _ = raw.write_all(line.as_bytes());
            let _ = raw.flush();
        }
        if !self.stdout_alive {
            return WriteOutcome::StdoutBroken;
        }
        let mut out = std::io::stdout().lock();
        match out.write_all(line.as_bytes()).and_then(|_| out.flush()) {
            Ok(()) => WriteOutcome::Ok,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                self.stdout_alive = false;
                WriteOutcome::StdoutBroken
            }
            Err(e) => {
                // 写 stdout 失败本身不该静默 —— 记进原始日志，然后按断开处理。
                if let Some(raw) = self.raw.as_mut() {
                    let _ = writeln!(
                        raw,
                        "{{\"type\":\"diagnostic\",\"data\":{{\"level\":\"error\",\"message\":\"stdout 写入失败：{e}\"}}}}"
                    );
                    let _ = raw.flush();
                }
                self.stdout_alive = false;
                WriteOutcome::StdoutBroken
            }
        }
    }

    pub fn state(&mut self, state: WorkerState, detail: &str) -> WriteOutcome {
        self.emit(EventEnvelope::state(&self.task_id, 0, state, detail))
    }

    pub fn diagnostic(&mut self, level: &str, code: Option<&str>, message: &str) -> WriteOutcome {
        self.emit(EventEnvelope::diagnostic(&self.task_id, 0, level, code, message))
    }

    pub fn tool_call(&mut self, call_id: &str, tool: &str, summary: &str) -> WriteOutcome {
        self.emit(EventEnvelope::tool_call(&self.task_id, 0, call_id, tool, summary))
    }

    pub fn tool_result(
        &mut self,
        call_id: &str,
        tool: &str,
        status: &str,
        summary: &str,
    ) -> WriteOutcome {
        self.emit(EventEnvelope::tool_result(&self.task_id, 0, call_id, tool, status, summary))
    }

    pub fn result(&mut self, result: &TaskResult) -> WriteOutcome {
        let envelope = EventEnvelope::result(&self.task_id, 0, result);
        self.emit(envelope)
    }
}

/// 完整写完再发布：先写临时文件，再改名。
///
/// 直接往目标文件里逐块写的话，中途失败会留下一个**看起来存在但不完整**的
/// 结果文件，而主控很可能就把它读走了。
pub fn write_result_file(path: &std::path::Path, result: &TaskResult) -> std::io::Result<()> {
    let json = result.to_json_pretty();
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = match path.file_name() {
        Some(name) => path.with_file_name(format!("{}.tmp", name.to_string_lossy())),
        None => path.with_extension("tmp"),
    };
    {
        let mut file = File::create(&tmp)?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows 上目标已存在时 rename 会失败；先删再改。
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp, path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_deck_core::result::TaskStatus;

    #[test]
    fn result_file_is_written_and_parsable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        let result = TaskResult::new("t1", TaskStatus::Completed, "done");
        write_result_file(&path, &result).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: TaskResult = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.task_id, "t1");
        assert_eq!(parsed.status, TaskStatus::Completed);

        // 临时文件不该留下。
        assert!(!dir.path().join("result.json.tmp").exists());
    }

    #[test]
    fn rewriting_an_existing_result_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        write_result_file(&path, &TaskResult::new("t1", TaskStatus::Failed, "old")).unwrap();
        write_result_file(&path, &TaskResult::new("t1", TaskStatus::Completed, "new")).unwrap();
        let parsed: TaskResult =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.status, TaskStatus::Completed);
    }

    #[test]
    fn unwritable_result_path_is_an_error_not_a_silent_success() {
        let dir = tempfile::tempdir().unwrap();
        // 把目录当成文件路径：写结果必须失败，而不是无声地什么都不做。
        let err = write_result_file(&dir.path().to_path_buf(), &TaskResult::new("t", TaskStatus::Completed, "x"));
        assert!(err.is_err());
    }
}
