//! 会话服务：独占一个交互内核，按 [`arc_deck_core::service`] 的消息为前端干活。
//!
//! 前端（TUI 在进程内；GUI 将来经 IPC）只发 [`Command`]、只收 [`Event`]，不碰 DSH 的任何形状。
//! 这一层负责的三件事：
//!
//! 1. **内核生命周期**：起、等就绪、退出时收进程树。起不来时给可操作的原因并**继续应答**
//!    （每条命令回一句「后端不可用：…」），而不是让前端对着一个死掉的通道发消息。
//! 2. **订阅**：每个打开的会话一个 follow 泵，变了就推一份视图。流式时每个 token 都是一次变化，
//!    整份视图每次都克隆会把长会话拖慢，所以**合并到最多每 [`PUSH_INTERVAL`] 一次**；
//!    终态（turn/end）不合并 —— 用户等的就是那一帧。
//! 3. **默认模型**：内核的出厂默认不是用户配置的那条（实测），新会话一律先选一次。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_deck_core::conversation::ModelChoice;
use arc_deck_core::service::{BackendState, Command, Event, NoticeLevel};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::host::{WebKernel, WebKernelOptions};
use super::launch::HomeLock;
use super::session::{SessionClient, SessionUpdate};

/// 流式更新推给前端的最短间隔。20 Hz 对人眼足够；再快只是在复制整份视图。
pub const PUSH_INTERVAL: Duration = Duration::from_millis(50);

/// 起服务要的东西。由调用方（cli）从用户配置构造 —— 见 `launch::interactive`。
pub struct ServiceInput {
    pub options: WebKernelOptions,
    /// 新会话默认用的模型（用户配置的路由）。
    pub default_model: Option<ModelChoice>,
    pub workspace: PathBuf,
    /// 持有到服务结束：内核收掉之前不能放锁。
    pub lock: Option<HomeLock>,
}

/// 前端手里的两头。
pub struct ServiceHandle {
    pub commands: mpsc::UnboundedSender<Command>,
    pub events: mpsc::UnboundedReceiver<Event>,
    pub task: JoinHandle<()>,
}

/// 起服务。内核在后台起，**立即返回** —— 前端先收到 `Backend{Starting}`，界面不用等。
pub fn spawn(input: ServiceInput) -> ServiceHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(input, command_rx, event_tx));
    ServiceHandle { commands: command_tx, events: event_rx, task }
}

/// 一个打开着的会话的泵。
struct Pump {
    close: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    /// 这个会话最后一次选定的模型。相同就不重复 `selectModel`。
    model: Option<ModelChoice>,
}

async fn run(input: ServiceInput, mut commands: mpsc::UnboundedReceiver<Command>, events: mpsc::UnboundedSender<Event>) {
    let send = |event: Event| {
        let _ = events.send(event);
    };
    send(Event::Backend { state: BackendState::Starting });
    let ServiceInput { options, default_model, workspace, lock } = input;

    let mut kernel = match WebKernel::start(options).await {
        Ok(kernel) => kernel,
        Err(error) => {
            let reason = error.to_string();
            send(Event::Backend { state: BackendState::Failed { reason: reason.clone() } });
            drop(lock);
            // 起不来也继续应答：前端发来的每条命令都得到同一个可操作的原因。
            while let Some(command) = commands.recv().await {
                if matches!(command, Command::Shutdown) {
                    break;
                }
                send(Event::Notice { level: NoticeLevel::Error, text: format!("后端不可用：{reason}") });
            }
            send(Event::Backend { state: BackendState::Stopped });
            return;
        }
    };
    let client = match SessionClient::connect(&kernel).await {
        Ok(client) => Arc::new(client),
        Err(error) => {
            send(Event::Backend { state: BackendState::Failed { reason: format!("连不上内核：{error}") } });
            kernel.shutdown().await;
            drop(lock);
            while let Some(command) = commands.recv().await {
                if matches!(command, Command::Shutdown) {
                    break;
                }
            }
            send(Event::Backend { state: BackendState::Stopped });
            return;
        }
    };
    send(Event::Backend { state: BackendState::Ready { model: default_model.clone() } });
    match client.model_catalog().await {
        Ok(catalog) => send(Event::Catalog { catalog }),
        Err(error) => send(Event::Notice { level: NoticeLevel::Warning, text: format!("读模型目录失败：{error}") }),
    }
    match client.list().await {
        Ok(sessions) => send(Event::Sessions { sessions }),
        Err(error) => send(Event::Notice { level: NoticeLevel::Warning, text: format!("读会话列表失败：{error}") }),
    }

    let mut pumps: HashMap<String, Pump> = HashMap::new();
    while let Some(command) = commands.recv().await {
        match command {
            Command::Shutdown => break,
            Command::Hello { .. } | Command::Unknown => {}
            Command::ListSessions => match client.list().await {
                Ok(sessions) => send(Event::Sessions { sessions }),
                Err(error) => send(Event::Notice { level: NoticeLevel::Error, text: format!("读会话列表失败：{error}") }),
            },
            Command::Catalog => match client.model_catalog().await {
                Ok(catalog) => send(Event::Catalog { catalog }),
                Err(error) => send(Event::Notice { level: NoticeLevel::Error, text: format!("读模型目录失败：{error}") }),
            },
            Command::OpenSession { session_id } => {
                close_pump(&mut pumps, &session_id).await;
                if let Err(text) = open_pump(&client, &session_id, None, &events, &mut pumps).await {
                    send(Event::Notice { level: NoticeLevel::Error, text });
                }
            }
            Command::CloseSession { session_id } => close_pump(&mut pumps, &session_id).await,
            Command::Prompt { session_id, text, model } => {
                let session_id = match session_id {
                    Some(id) => id,
                    None => match client.create(&workspace).await {
                        Ok(id) => {
                            send(Event::SessionStarted { session_id: id.clone() });
                            id
                        }
                        Err(error) => {
                            send(Event::Notice { level: NoticeLevel::Error, text: format!("新建会话失败：{error}") });
                            continue;
                        }
                    },
                };
                let wanted = model.or_else(|| {
                    // 已打开的会话没有指定模型就沿用它自己的；只有新会话才套默认值。
                    if pumps.contains_key(&session_id) { None } else { default_model.clone() }
                });
                if !pumps.contains_key(&session_id) {
                    // 先选模型再打开：打开时的快照里就是选好的那条，界面不会先闪一下出厂默认。
                    if let Some(choice) = &wanted {
                        if let Err(error) = client.select_model(&session_id, choice).await {
                            send(Event::Notice { level: NoticeLevel::Error, text: format!("选模型失败：{error}") });
                            continue;
                        }
                    }
                    if let Err(text) = open_pump(&client, &session_id, wanted.clone(), &events, &mut pumps).await {
                        send(Event::Notice { level: NoticeLevel::Error, text });
                        continue;
                    }
                } else if let Some(choice) = wanted {
                    let pump = pumps.get_mut(&session_id).expect("刚判过存在");
                    if pump.model.as_ref() != Some(&choice) {
                        match client.select_model(&session_id, &choice).await {
                            Ok(_) => pump.model = Some(choice),
                            Err(error) => {
                                send(Event::Notice { level: NoticeLevel::Error, text: format!("选模型失败：{error}") });
                                continue;
                            }
                        }
                    }
                }
                if let Err(error) = client.prompt(&session_id, &text).await {
                    send(Event::Notice { level: NoticeLevel::Error, text: format!("发送失败：{error}") });
                }
            }
            Command::Cancel { session_id } => match client.cancel(&session_id).await {
                Ok(()) => send(Event::Notice { level: NoticeLevel::Info, text: "已请求取消这一轮".into() }),
                Err(error) => send(Event::Notice { level: NoticeLevel::Error, text: format!("取消失败：{error}") }),
            },
            Command::Rename { session_id, title } => match client.rename(&session_id, &title).await {
                Ok(title) => {
                    send(Event::Notice { level: NoticeLevel::Info, text: format!("已改名：{title}") });
                    if let Ok(sessions) = client.list().await {
                        send(Event::Sessions { sessions });
                    }
                }
                Err(error) => send(Event::Notice { level: NoticeLevel::Error, text: format!("改名失败：{error}") }),
            },
        }
    }

    let ids: Vec<String> = pumps.keys().cloned().collect();
    for id in ids {
        close_pump(&mut pumps, &id).await;
    }
    drop(client);
    let termination = kernel.shutdown().await;
    if !termination.exited {
        send(Event::Notice { level: NoticeLevel::Warning, text: format!("内核收尾未确认：{}", termination.detail) });
    }
    drop(lock);
    send(Event::Backend { state: BackendState::Stopped });
}

async fn open_pump(
    client: &Arc<SessionClient>,
    session_id: &str,
    model: Option<ModelChoice>,
    events: &mpsc::UnboundedSender<Event>,
    pumps: &mut HashMap<String, Pump>,
) -> Result<(), String> {
    let mut open = client.open(session_id).await.map_err(|e| format!("打开会话失败：{e}"))?;
    let _ = events.send(Event::Conversation { conversation: Box::new(open.view().clone()) });
    let (close_tx, mut close_rx) = oneshot::channel::<()>();
    let tx = events.clone();
    let id = session_id.to_string();
    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(PUSH_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut dirty = false;
        let mut was_running = open.view().running;
        loop {
            tokio::select! {
                _ = &mut close_rx => {
                    open.close();
                    break;
                }
                _ = tick.tick(), if dirty => {
                    dirty = false;
                    let _ = tx.send(Event::Conversation { conversation: Box::new(open.view().clone()) });
                }
                update = open.next() => match update {
                    SessionUpdate::Changed => {
                        let running = open.view().running;
                        if was_running && !running {
                            // 一轮结束：立刻推，不等合并窗口。
                            dirty = false;
                            let _ = tx.send(Event::Conversation { conversation: Box::new(open.view().clone()) });
                        } else {
                            dirty = true;
                        }
                        was_running = running;
                    }
                    SessionUpdate::Unchanged => {}
                    SessionUpdate::Ended(reason) => {
                        if dirty {
                            let _ = tx.send(Event::Conversation { conversation: Box::new(open.view().clone()) });
                        }
                        let _ = tx.send(Event::SessionClosed { session_id: id.clone(), reason });
                        break;
                    }
                }
            }
        }
    });
    pumps.insert(session_id.to_string(), Pump { close: Some(close_tx), task, model });
    Ok(())
}

async fn close_pump(pumps: &mut HashMap<String, Pump>, session_id: &str) {
    if let Some(mut pump) = pumps.remove(session_id) {
        if let Some(close) = pump.close.take() {
            let _ = close.send(());
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), pump.task).await;
    }
}
