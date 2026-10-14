//! M4a bridge: the UI consumes the same facts as `arcdeck status`.
//! Headless remains the sole execution path; no provider loop lives in the renderer.
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use arc_deck_core::{
    event::EventEnvelope,
    paths,
    result::TaskResult,
    userconfig::{self, UserConfig},
};
use arc_deck_tui::{Boot, Request, RunDetail, RunRecord, StartRequest, Update};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::status::{self, StatusFile};

const TEXT_LIMIT: u64 = 128 * 1024;
const EVENT_LIMIT: u64 = 2 * 1024 * 1024;

pub async fn run() -> i32 {
    let workspace = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("TUI: {error}");
            return 64;
        }
    };
    let (model, effort, config_error, root, config_path) = match UserConfig::locate_and_load(None) {
        Ok((cfg, path)) => (
            Some(cfg.model.clone()),
            cfg.effort.clone(),
            None,
            paths::absolutize(&cfg.state_root()),
            Some(paths::absolutize(&path)),
        ),
        Err(error) => (
            None,
            None,
            Some(error.to_string()),
            userconfig::default_state_root(),
            None,
        ),
    };
    let boot = Boot {
        workspace: workspace.clone(),
        model,
        effort,
        config_error,
        preferences: userconfig::data_dir()
            .join("arcdeck")
            .join("tui-preferences.json"),
    };
    let (request_tx, request_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    let bridge = tokio::spawn(serve(root, workspace, config_path, request_rx, update_tx));
    let ui =
        tokio::task::spawn_blocking(move || arc_deck_tui::run(boot, request_tx, update_rx)).await;
    // Also wait on failure: raw-mode startup errors and panics must not strand a worker.
    let cleanup = bridge.await;
    if let Err(error) = cleanup {
        eprintln!("TUI 收尾失败：{error}");
        return 5;
    }
    match ui {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("TUI: {error}");
            64
        }
        Err(error) => {
            eprintln!("TUI: {error}");
            5
        }
    }
}

struct OwnedRun {
    id: String,
    task: JoinHandle<Result<(i32, String), String>>,
}

async fn serve(
    root: PathBuf,
    workspace: PathBuf,
    config: Option<PathBuf>,
    requests: Receiver<Request>,
    updates: Sender<Update>,
) {
    let mut selected: Option<String> = None;
    let mut owned: Option<OwnedRun> = None;
    let mut previous_runs = Vec::new();
    let mut previous_detail: Option<RunDetail> = None;
    let mut cancel_tasks = Vec::<JoinHandle<()>>::new();
    let mut refresh = true;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        if owned.as_ref().is_some_and(|r| r.task.is_finished()) {
            let run = owned.take().unwrap();
            match run.task.await {
                Ok(Ok((code, error))) if code == 0 || code == 3 => {
                    let _ = error;
                }
                Ok(Ok((code, error))) => {
                    let _ = updates.send(Update::Notice(format!(
                        "运行退出 {code}：{}",
                        status::clip(&error, 1500)
                    )));
                }
                Ok(Err(error)) => {
                    let _ = updates.send(Update::Notice(error));
                }
                Err(error) => {
                    let _ = updates.send(Update::Notice(error.to_string()));
                }
            }
            let _ = updates.send(Update::Finished(run.id));
            refresh = true;
        }
        let mut quit = false;
        loop {
            match requests.try_recv() {
                Ok(Request::Start(request)) => {
                    if owned.is_some() {
                        let _ = updates.send(Update::Notice("已有活动运行".into()));
                        let _ = updates.send(Update::Finished(request.id));
                        continue;
                    }
                    if !valid_id(&request.id) {
                        let _ = updates.send(Update::Notice("运行 ID 不合法".into()));
                        continue;
                    }
                    selected = Some(request.id.clone());
                    previous_detail = None;
                    let run_root = root.clone();
                    let run_workspace = workspace.clone();
                    let run_config = config.clone();
                    owned = Some(OwnedRun {
                        id: request.id.clone(),
                        task: tokio::spawn(async move {
                            start(&run_root, &run_workspace, run_config.as_deref(), request).await
                        }),
                    });
                    refresh = true;
                }
                Ok(Request::Open(id)) => {
                    selected = id.filter(|id| valid_id(id));
                    previous_detail = None;
                    refresh = true;
                }
                Ok(Request::Cancel(id)) => {
                    // Never cancel an arbitrary directory supplied by a UI message.
                    if selected.as_deref() != Some(&id)
                        && owned.as_ref().map(|r| r.id.as_str()) != Some(&id)
                    {
                        continue;
                    }
                    let cancel_root = root.clone();
                    let tx = updates.clone();
                    cancel_tasks.push(tokio::spawn(async move {
                        match cancel(&cancel_root, &id).await {
                            Ok(text) => {
                                let _ = tx.send(Update::Notice(text));
                            }
                            Err(error) => {
                                let _ = tx.send(Update::Notice(format!("取消未确认：{error}")));
                            }
                        }
                    }));
                }
                Ok(Request::Refresh) => refresh = true,
                Ok(Request::Quit) | Err(TryRecvError::Disconnected) => {
                    quit = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if quit {
            if let Some(run) = owned {
                if !run.task.is_finished() {
                    if let Err(error) = cancel(&root, &run.id).await {
                        eprintln!(
                            "退出时取消未确认：{error}；请检查 arcdeck status {}",
                            run.id
                        );
                    }
                }
                // The supervisor owns a bounded shutdown; do not kill it and orphan DSH.
                if tokio::time::timeout(Duration::from_secs(15), run.task)
                    .await
                    .is_err()
                {
                    eprintln!("收尾仍在运行；请检查 arcdeck status {}", run.id);
                }
            }
            for task in cancel_tasks {
                let _ = task.await;
            }
            return;
        }
        if refresh {
            let runs = list_runs(&root, &workspace);
            if runs != previous_runs {
                let _ = updates.send(Update::Runs(runs.clone()));
                previous_runs = runs;
            }
            if let Some(id) = &selected
                && let Some(detail) = read_detail(&root, &workspace, id)
                && previous_detail.as_ref() != Some(&detail)
            {
                let _ = updates.send(Update::Detail(Box::new(detail.clone())));
                previous_detail = Some(detail);
            }
        }
        cancel_tasks.retain(|t| !t.is_finished());
        tick.tick().await;
        refresh = true;
    }
}

fn command() -> io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW: helper must not steal focus.
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    Ok(command)
}

async fn start(
    root: &Path,
    workspace: &Path,
    config: Option<&Path>,
    request: StartRequest,
) -> Result<(i32, String), String> {
    let mut command = command().map_err(|e| e.to_string())?;
    command
        .arg("run")
        .args([
            "--prompt",
            "-",
            "--format",
            "jsonl",
            "--id",
            &request.id,
            "--agent",
            &request.agent,
            "--permission",
            &request.permission,
        ])
        .arg("--workspace")
        .arg(workspace)
        .arg("--state-root")
        .arg(root)
        .stdin(Stdio::piped());
    if let Some(path) = config {
        command.arg("--config").arg(path);
    }
    if let Some(effort) = &request.effort {
        command.arg("--effort").arg(effort);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let mut stderr = child.stderr.take().ok_or("子进程缺 stderr")?;
    let drain = tokio::spawn(async move {
        let mut tail = Vec::new();
        let mut buffer = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buffer[..n]);
            if tail.len() > 65536 {
                tail.drain(..tail.len() - 65536);
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    });
    let mut stdin = child.stdin.take().ok_or("子进程缺 stdin")?;
    let input_result = stdin.write_all(request.prompt.as_bytes()).await;
    drop(stdin); // EOF lets --prompt - finish reading before it starts the runtime.
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let error = drain.await.map_err(|e| e.to_string())?;
    let dir = root.join("runs").join(&request.id);
    if dir.is_dir() {
        let _ = std::fs::write(dir.join("tui-console.err"), &error);
    }
    if status.code() == Some(64) && dir.is_dir() && !dir.join("status.json").is_file() {
        status::write_preflight_failure(
            &dir,
            status::PreflightFailure {
                task_id: request.id,
                workspace: workspace.display().to_string(),
                model: None,
                assigned_by: None,
                title: status::derive_title(&request.prompt),
                code: "TUI_RUN_PREFLIGHT".into(),
                message: error.clone(),
            },
        );
    }
    input_result.map_err(|e| format!("输入未完整送达：{e}"))?;
    Ok((status.code().unwrap_or(5), error))
}

async fn cancel(root: &Path, id: &str) -> Result<String, String> {
    if !valid_id(id) {
        return Err("运行 ID 不合法".into());
    }
    // Esc can arrive before the child has materialized its run directory/status.
    for _ in 0..20 {
        if root.join("runs").join(id).join("status.json").is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut command = command().map_err(|e| e.to_string())?;
    command
        .arg("cancel")
        .arg(id)
        .arg("--state-root")
        .arg(root)
        .stdout(Stdio::piped());
    let output = command.output().await.map_err(|e| e.to_string())?;
    // Exit 3 is cancelled, not a failed helper. Other terminal outcomes can win the race.
    match output.status.code() {
        Some(0..=5) => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        _ => {
            let error = if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            };
            Err(String::from_utf8_lossy(error).trim().to_string())
        }
    }
}

fn valid_id(id: &str) -> bool {
    let mut components = Path::new(id).components();
    matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !id.contains(['/', '\\'])
}

fn record(st: StatusFile) -> RunRecord {
    let health = status::health(&st, status::now_ms()).to_string();
    RunRecord {
        id: st.task_id.clone(),
        title: st.title.unwrap_or_else(|| st.task_id.clone()),
        state: st.state,
        health,
        activity: format!(
            "{} {}",
            st.activity.kind,
            st.activity.summary.unwrap_or_default()
        ),
        model: st.model,
        started_at: st.started_at,
        terminal: st.terminal == Some(true),
        usage: st.usage,
        notice: st
            .notice
            .map(|n| format!("{}: {}", n.code, n.message))
            .or_else(|| st.last_error.map(|e| format!("{}: {}", e.code, e.message))),
    }
}

fn list_runs(root: &Path, workspace: &Path) -> Vec<RunRecord> {
    let mut records = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("runs")) {
        for entry in entries.flatten() {
            if let Some(st) = status::read(&entry.path().join("status.json"))
                && entry.file_name().to_str() == Some(&st.task_id)
                && paths::same_path(Path::new(&st.workspace), workspace)
            {
                records.push(record(st));
            }
        }
    }
    records.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    records.truncate(200);
    records
}

fn bounded_text(path: &Path, limit: u64) -> Option<(String, bool)> {
    let file = File::open(path).ok()?;
    let truncated = file.metadata().ok()?.len() > limit;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    // If the bound cuts a CJK character, only the incomplete trailing bytes are omitted.
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if error.utf8_error().error_len().is_none() => {
            let end = error.utf8_error().valid_up_to();
            String::from_utf8(error.into_bytes()[..end].to_vec()).ok()?
        }
        Err(_) => return None,
    };
    Some((text, truncated))
}

fn read_events(path: &Path, id: &str) -> (Vec<EventEnvelope>, bool) {
    let Ok(mut file) = File::open(path) else {
        return (Vec::new(), false);
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(EVENT_LIMIT);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return (Vec::new(), true);
    }
    let mut bytes = Vec::new();
    if file.take(EVENT_LIMIT).read_to_end(&mut bytes).is_err() {
        return (Vec::new(), true);
    }
    let end = bytes
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|n| n + 1)
        .unwrap_or(0);
    let start = if start > 0 {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(end)
    } else {
        0
    };
    let mut omitted = size > EVENT_LIMIT;
    let mut events = Vec::new();
    for line in bytes[start..end]
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
    {
        match serde_json::from_slice::<EventEnvelope>(line) {
            Ok(event)
                if event.task_id == id && event.schema_version == arc_deck_core::SCHEMA_VERSION =>
            {
                events.push(event)
            }
            _ => omitted = true,
        }
    }
    (events, omitted)
}

fn read_detail(root: &Path, workspace: &Path, id: &str) -> Option<RunDetail> {
    if !valid_id(id) {
        return None;
    }
    let dir = root.join("runs").join(id);
    let st = status::read(&dir.join("status.json"))?;
    if st.task_id != id || !paths::same_path(Path::new(&st.workspace), workspace) {
        return None;
    }
    let (events, mut truncated) = read_events(&dir.join("events.jsonl"), id);
    let task_text = bounded_text(&dir.join("task.json"), EVENT_LIMIT);
    truncated |= task_text.as_ref().is_some_and(|(_, capped)| *capped);
    let instruction = task_text
        .and_then(|(s, _)| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("instruction")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| st.title.clone().unwrap_or_default());
    let result_text = bounded_text(&dir.join("result.json"), EVENT_LIMIT);
    truncated |= result_text.as_ref().is_some_and(|(_, capped)| *capped);
    let result = result_text
        .and_then(|(s, _)| serde_json::from_str::<TaskResult>(&s).ok())
        .filter(|r| r.task_id == id && r.schema_version == arc_deck_core::SCHEMA_VERSION);
    let final_text = bounded_text(&dir.join("final-message.md"), TEXT_LIMIT).map(|(s, t)| {
        truncated |= t;
        s
    });
    Some(RunDetail {
        record: record(st),
        instruction,
        events,
        result,
        final_text,
        evidence_dir: dir,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_ids_cannot_escape_the_run_directory() {
        for id in ["..", ".", "../x", "x/y", "x\\y", "C:\\x", ""] {
            assert!(!valid_id(id), "{id}");
        }
        assert!(valid_id("tui-123-4"));
    }
    #[test]
    fn active_logs_ignore_a_partial_record_and_filter_wrong_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let good = EventEnvelope::diagnostic("r", 1, "info", None, "中文").to_line();
        let wrong = EventEnvelope::diagnostic("other", 2, "info", None, "x").to_line();
        std::fs::write(&path, format!("{good}{wrong}{{\"unfinished\":")).unwrap();
        let (events, omitted) = read_events(&path, "r");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data["message"], "中文");
        assert!(omitted);
    }
    #[test]
    fn capped_text_does_not_replace_half_a_chinese_character() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("message");
        std::fs::write(&path, "甲乙丙").unwrap();
        let (text, truncated) = bounded_text(&path, 5).unwrap();
        assert_eq!(text, "甲");
        assert!(truncated);
        assert!(!text.contains('\u{fffd}'));
    }
}
