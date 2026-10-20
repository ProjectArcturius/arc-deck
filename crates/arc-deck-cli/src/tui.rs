//! TUI 的后端桥。两条路，都不在渲染器里跑任何 provider 循环：
//!
//! - **交互会话**：后台起 `arc_deck_dsh::remote::service`（独占一个 web 内核），
//!   `Request::Session` / `Update::Session` 原样转发 —— 那组消息就是 GUI 将来经 IPC 用的那一份。
//! - **独立运行**：内核起不来时的退路，以及 `/runs` 里看工人（与 `arcdeck status` 同一份事实）。
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_deck_core::{
    authz::Permission,
    event::EventEnvelope,
    mode::{AgentId, Mode},
    paths,
    result::TaskResult,
    service::{BackendState, Command as SessionCommand, Event as SessionEvent},
    userconfig::{self, UserConfig},
};
use arc_deck_tui::{Boot, Request, RunDetail, RunRecord, StartRequest, Update, catalog};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::status::{self, StatusFile};

const TEXT_LIMIT: u64 = 128 * 1024;
const EVENT_LIMIT: u64 = 2 * 1024 * 1024;

/// 启动时从用户配置里取到的几样东西，避免 `run` 里再拆一次元组。
struct Loaded {
    model: Option<String>,
    effort: Option<String>,
    config_error: Option<String>,
    root: PathBuf,
    config_path: Option<PathBuf>,
    catalog: Vec<catalog::Provider>,
    config: Option<UserConfig>,
}

/// 模型目录的**临时**来源：用户配置的 `provider` 块（OpenCode 形状）。
///
/// ⚠️ 这不是权威目录。§7.2 说的是「provider/model 目录的权威来源是 DSH」，
/// 下一波接内核报告的可用路由。这里只是让对话框在真实目录接通前有东西可列：
/// provider 名与模型名的形状与 DSH 报告的一致，所以界面那一层不用改。
///
/// 两条刻意的取舍：
/// - `efforts` 一律留空。强度档按模型不同（DeepSeek 官方是 off/low/high/max，
///   pi-ai 路由来自它的目录），在界面上编几档再让运行时拒绝，比不给选择差得多。
/// - **配置的默认模型一定在目录里**。它可能指向内置 provider（用户的 `provider` 块里
///   没有这一条）或干脆不在块里，此时以 provider id 作名字补一条 —— 否则用户会在
///   模型列表里找不到自己正在用的那条路由。
fn catalog_from_config(config: &UserConfig) -> Vec<catalog::Provider> {
    let route = catalog::parse_route(&config.model);

    let mut providers: Vec<catalog::Provider> = config
        .provider
        .iter()
        .map(|(id, provider)| catalog::Provider {
            id: id.clone(),
            // 缺名字时用 id：OpenCode 形状里 `name` 是可选的。
            name: provider.name.clone().unwrap_or_else(|| id.clone()),
            models: provider
                .models
                .iter()
                .map(|(model_id, model)| catalog::Model {
                    id: model_id.clone(),
                    name: model.name.clone(),
                    // 这一波不编造代价数据：`None` 是「没有数据」，界面就不显示 Free。
                    free: None,
                    // 强度档来自 DSH 的目录，不在配置里（见函数头的说明）。
                    efforts: Vec::new(),
                    ..catalog::Model::default()
                })
                .collect(),
        })
        .collect();

    if let Some(catalog::ModelRef {
        provider_id,
        model_id,
    }) = route
        && !providers.iter().any(|provider| {
            provider.id == provider_id && provider.models.iter().any(|model| model.id == model_id)
        })
    {
        let model = catalog::Model {
            id: model_id.clone(),
            ..catalog::Model::default()
        };
        match providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
        {
            Some(provider) => provider.models.push(model),
            None => providers.push(catalog::Provider {
                id: provider_id.clone(),
                // provider 不在配置里（内置 provider）时就以 provider id 作名字。
                name: provider_id.clone(),
                models: vec![model],
            }),
        }
    }
    providers
}

fn model_prefs_path() -> PathBuf {
    userconfig::data_dir()
        .join("arcdeck")
        .join("model.json")
}

pub async fn run() -> i32 {
    let workspace = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("TUI: {error}");
            return 64;
        }
    };
    let loaded = match UserConfig::locate_and_load(None) {
        Ok((cfg, path)) => Loaded {
            model: Some(cfg.model.clone()),
            effort: cfg.effort.clone(),
            config_error: None,
            root: paths::absolutize(&cfg.state_root()),
            config_path: Some(paths::absolutize(&path)),
            catalog: catalog_from_config(&cfg),
            config: Some(cfg),
        },
        Err(error) => Loaded {
            model: None,
            effort: None,
            config_error: Some(error.to_string()),
            root: userconfig::default_state_root(),
            config_path: None,
            catalog: Vec::new(),
            config: None,
        },
    };
    let boot = Boot {
        workspace: workspace.clone(),
        model: loaded.model,
        effort: loaded.effort,
        config_error: loaded.config_error,
        preferences: userconfig::data_dir()
            .join("arcdeck")
            .join("tui-preferences.json"),
        personalization: loaded.config.as_ref().is_some_and(|cfg| cfg.personalization.enabled),
        catalog: loaded.catalog,
        model_prefs: model_prefs_path(),
    };
    let (request_tx, request_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    let bridge = tokio::spawn(serve(
        loaded.root,
        workspace,
        loaded.config_path,
        loaded.config,
        request_rx,
        update_tx,
    ));
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

/// 一个在跑的会话服务。
struct Backend {
    commands: tokio::sync::mpsc::UnboundedSender<SessionCommand>,
    task: JoinHandle<()>,
    forward: JoinHandle<()>,
}

/// 起会话服务。起不来（运行时没装、密钥缺失、另一个 arcdeck 占着会话目录）就把原因
/// 作为 `Backend{Failed}` 发给界面 —— 界面据此退回独立请求，而不是对着一个死通道发消息。
fn start_backend(
    cfg: Option<&UserConfig>,
    workspace: &Path,
    permission: Permission,
    agent: AgentId,
    mode: Mode,
    updates: &Sender<Update>,
    current: &Arc<Mutex<Option<String>>>,
) -> Option<Backend> {
    let fail = |reason: String| {
        let _ = updates.send(Update::Session(SessionEvent::Backend {
            state: BackendState::Failed { reason },
        }));
        None
    };
    let Some(cfg) = cfg else {
        return fail("没有可用的用户配置；先运行 `arcdeck config init`".into());
    };
    let kernel_id = format!("tui-{}", std::process::id());
    let (handle, notes) = match crate::serve::spawn_service(cfg, workspace, permission, agent, mode, &kernel_id) {
        Ok(started) => started,
        Err(reason) => return fail(reason),
    };
    for note in &notes {
        let _ = updates.send(Update::Notice(format!("授权缺口（已留档）：{note}")));
    }
    let mut events = handle.events;
    let tx = updates.clone();
    let current = Arc::clone(current);
    let forward = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let SessionEvent::SessionStarted { session_id } = &event
                && let Ok(mut current) = current.lock()
            {
                *current = Some(session_id.clone());
            }
            if tx.send(Update::Session(event)).is_err() {
                break;
            }
        }
    });
    Some(Backend { commands: handle.commands, task: handle.task, forward })
}

/// 收掉会话服务：它会先关掉所有 follow、再收内核进程树、最后放锁。
async fn stop_backend(backend: Backend) {
    let _ = backend.commands.send(SessionCommand::Shutdown);
    if tokio::time::timeout(Duration::from_secs(20), backend.task).await.is_err() {
        eprintln!("交互内核 20 秒内没有收尾；如有残留 node 进程请手动结束");
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), backend.forward).await;
}

async fn serve(
    root: PathBuf,
    workspace: PathBuf,
    config: Option<PathBuf>,
    user_config: Option<UserConfig>,
    requests: Receiver<Request>,
    updates: Sender<Update>,
) {
    // 当前会话：重启内核（改授权级别）之后要把它重新打开。会话在独立的 DSH home 里，所以这是「恢复」
    // 而不是「丢掉重来」。
    let current_session: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut backend = start_backend(
        user_config.as_ref(),
        &workspace,
        Permission::Workspace,
        AgentId::Echo,
        Mode::Build,
        &updates,
        &current_session,
    );
    let mut selected: Option<String> = None;
    let mut owned: Option<OwnedRun> = None;
    let mut previous_runs = Vec::new();
    let mut previous_detail: Option<RunDetail> = None;
    let mut cancel_tasks = Vec::<JoinHandle<()>>::new();
    let mut refresh = true;
    // 请求每 50 ms 收一次（发消息、取消要跟手），读运行目录的刷新仍是 500 ms 一次 ——
    // 后者要列目录、读文件，频繁做没有意义。
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let mut ticks: u32 = 0;
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
                Ok(Request::Session(command)) => {
                    if let Ok(mut current) = current_session.lock() {
                        match &command {
                            SessionCommand::OpenSession { session_id } => *current = Some(session_id.clone()),
                            SessionCommand::Prompt { session_id: Some(id), .. } => *current = Some(id.clone()),
                            SessionCommand::CloseSession { session_id }
                                if current.as_deref() == Some(session_id.as_str()) =>
                            {
                                *current = None
                            }
                            _ => {}
                        }
                    }
                    match &backend {
                        Some(running) => {
                            if running.commands.send(command).is_err() {
                                let _ = updates.send(Update::Notice("交互内核已经退出".into()));
                            }
                        }
                        None => {
                            let _ = updates.send(Update::Notice("交互会话不可用；提交会改为独立请求".into()));
                        }
                    }
                }
                Ok(Request::RestartBackend { permission, agent, mode }) => {
                    let level = match Permission::parse(&permission) {
                        Ok(level) => level,
                        Err(error) => {
                            let _ = updates.send(Update::Notice(error.message));
                            continue;
                        }
                    };
                    let (agent, mode) = match (AgentId::parse(&agent), Mode::parse(&mode)) {
                        (Ok(agent), Ok(mode)) => (agent, mode),
                        (Err(error), _) | (_, Err(error)) => {
                            let _ = updates.send(Update::Notice(error.message));
                            continue;
                        }
                    };
                    if let Some(old) = backend.take() {
                        stop_backend(old).await;
                    }
                    backend = start_backend(
                        user_config.as_ref(),
                        &workspace,
                        level,
                        agent,
                        mode,
                        &updates,
                        &current_session,
                    );
                    let reopen = current_session.lock().ok().and_then(|c| c.clone());
                    if let (Some(running), Some(session_id)) = (&backend, reopen) {
                        // 命令在服务的队列里等内核就绪，所以现在发就行。
                        let _ = running.commands.send(SessionCommand::OpenSession { session_id });
                    }
                }
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
            if let Some(running) = backend.take() {
                stop_backend(running).await;
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
        ticks = ticks.wrapping_add(1);
        refresh = ticks.is_multiple_of(10);
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
    // 模型来自对话框选定的那条路由；没选过时 `App` 会把配置里的路由带上。
    // 两者都不给就沿用子进程自己从配置读到的值（`--model` 是覆盖，不是必需项）。
    if let Some(model) = &request.model {
        command.arg("--model").arg(model);
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

    fn config(json: &str) -> UserConfig {
        UserConfig::parse(json, std::path::Path::new("test.json")).expect("夹具配置必须合法")
    }

    fn routes(providers: &[catalog::Provider]) -> Vec<String> {
        providers
            .iter()
            .flat_map(|provider| {
                provider
                    .models
                    .iter()
                    .map(|model| format!("{}/{}", provider.id, model.id))
            })
            .collect()
    }

    /// 目录来自 `provider` 块：OpenCode 形状的 `provider.<id>.name` 与
    /// `provider.<id>.models.<id>.name` 都要读出来。
    #[test]
    fn catalog_reads_provider_and_model_names_from_the_config_block() {
        let cfg = config(
            r#"{
              "model": "p/model-a",
              "provider": {
                "p": {
                  "name": "Provider P",
                  "models": { "model-a": { "name": "Model A" }, "model-b": {} }
                }
              }
            }"#,
        );
        let providers = catalog_from_config(&cfg);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "p");
        assert_eq!(providers[0].name, "Provider P");
        let names: Vec<(String, Option<String>)> = providers[0]
            .models
            .iter()
            .map(|model| (model.id.clone(), model.name.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("model-a".to_string(), Some("Model A".to_string())),
                ("model-b".to_string(), None),
            ]
        );
        // 强度档与代价这一波一律没有数据 —— 界面据此不显示 Free、也不进 effort 对话框。
        assert!(providers[0].models.iter().all(|model| model.efforts.is_empty()));
        assert!(providers[0].models.iter().all(|model| model.free.is_none()));
    }

    /// provider 块里缺 `name` 时用 id，不然列表上会出现空名字。
    #[test]
    fn provider_without_a_name_falls_back_to_its_id() {
        let cfg = config(
            r#"{
              "model": "p/a",
              "provider": { "p": { "models": { "a": {} } } }
            }"#,
        );
        let providers = catalog_from_config(&cfg);
        assert_eq!(providers[0].name, "p");
    }

    /// 默认模型不在 `provider` 块里（内置 provider 的情形）：目录里仍要能找到它，
    /// 否则用户在模型列表里找不到自己正在用的那条路由。
    #[test]
    fn default_model_is_present_even_when_its_provider_is_not_configured() {
        let cfg = config(
            r#"{
              "model": "opencode-go/deepseek-v4.1-flash",
              "provider": { "p": { "name": "P", "models": { "a": {} } } }
            }"#,
        );
        let providers = catalog_from_config(&cfg);
        let routes = routes(&providers);
        assert!(routes.contains(&"opencode-go/deepseek-v4.1-flash".to_string()));
        let added = providers
            .iter()
            .find(|provider| provider.id == "opencode-go")
            .expect("补进去的 provider");
        assert_eq!(added.name, "opencode-go", "不在配置里时以 provider id 作名字");
        assert_eq!(added.models.len(), 1);
    }

    /// provider 在块里、但默认模型不在它的 models 里：补进**已有的** provider，
    /// 而不是再开一条同 id 的 provider（那会让列表里出现两个同样的名字）。
    #[test]
    fn default_model_is_added_to_an_existing_provider() {
        let cfg = config(
            r#"{
              "model": "p/ghost",
              "provider": { "p": { "name": "Provider P", "models": { "a": {} } } }
            }"#,
        );
        let providers = catalog_from_config(&cfg);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "Provider P");
        assert_eq!(routes(&providers), vec!["p/a", "p/ghost"]);
    }

    /// 已经在块里的默认模型不能重复补一条。
    #[test]
    fn default_model_already_in_the_block_is_not_duplicated() {
        let cfg = config(
            r#"{
              "model": "p/a",
              "provider": { "p": { "models": { "a": { "name": "A" } } } }
            }"#,
        );
        let providers = catalog_from_config(&cfg);
        assert_eq!(routes(&providers), vec!["p/a"]);
        assert_eq!(providers[0].models[0].name.as_deref(), Some("A"));
    }

    /// 配置没有 provider 块（起步配置就是这样）：目录至少含默认模型那一条。
    #[test]
    fn empty_provider_block_still_yields_the_default_route() {
        let cfg = config(r#"{ "model": "opencode-go/deepseek-v4.1-flash", "provider": {} }"#);
        let providers = catalog_from_config(&cfg);
        assert_eq!(
            routes(&providers),
            vec!["opencode-go/deepseek-v4.1-flash".to_string()]
        );
    }

    /// 模型 id 里带 `/` 时按第一个 `/` 切分（§7.2），补进去的 id 要原样保留斜杠。
    #[test]
    fn default_route_splits_on_the_first_slash_only() {
        let cfg = config(r#"{ "model": "p/vendor/model-1", "provider": {} }"#);
        let providers = catalog_from_config(&cfg);
        assert_eq!(providers[0].id, "p");
        assert_eq!(providers[0].models[0].id, "vendor/model-1");
    }

    /// `model.json` 与 `tui-preferences.json` 同目录（都在 Arc Deck 的数据目录里）。
    #[test]
    fn model_prefs_live_next_to_the_ui_preferences() {
        let prefs = model_prefs_path();
        assert_eq!(
            prefs.file_name().and_then(|name| name.to_str()),
            Some("model.json")
        );
        let ui_prefs = userconfig::data_dir()
            .join("arcdeck")
            .join("tui-preferences.json");
        assert_eq!(prefs.parent(), ui_prefs.parent());
    }
}
