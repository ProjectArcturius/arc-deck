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

use super::events::{self, Forwarded};
use super::host::{WebKernel, WebKernelOptions};
use super::launch::HomeLock;
use super::session::{self, SessionClient, SessionUpdate};

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
        Ok(kernel) => {
            if let (Some(lock), Some(pid)) = (&lock, kernel.pid()) {
                lock.record_kernel(pid);
            }
            kernel
        }
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
            if kernel.shutdown().await.exited
                && let Some(lock) = &lock
            {
                lock.kernel_exited();
            }
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
    // 上一次列表里的会话，按 id。只用来给子 agent 会话配父地址（见 OpenSession）。
    let mut known: HashMap<String, arc_deck_core::conversation::SessionSummary> = HashMap::new();

    // ── 审批通道 ──────────────────────────────────────────────────────────
    //
    // 为什么常开而不是等 `confirm` 才开：审批请求是**内核发给连着它的客户端**的，
    // 没有客户端开着这条流时 ApprovalService 按 `unavailable` 失败关闭 —— 也就是工具被拒。
    // 常开意味着「有界面在，就能问人」；策略是 `never` 的那些档位根本不会提问，开着也只是
    // 一条空闲的流。断开时如实告诉界面，不让它以为还能应答。
    let (forwarded_tx, mut forwarded) = mpsc::unbounded_channel::<Forwarded>();
    let mut events_client: Option<String> = None;
    match client.subscribe_events().await {
        Ok(mut subscription) => {
            events_client = Some(subscription.client_id().to_string());
            tokio::spawn(async move {
                loop {
                    let item = subscription.next().await;
                    let ended = matches!(item, Forwarded::Ended(_));
                    if forwarded_tx.send(item).is_err() || ended {
                        break;
                    }
                }
            });
        }
        Err(error) => send(Event::Notice {
            level: NoticeLevel::Warning,
            text: format!(
                "审批通道没有建起来（{error}）；`confirm` 授权下的审批请求会被内核按「无人应答」拒绝"
            ),
        }),
    }

    loop {
        let command = tokio::select! {
            command = commands.recv() => command,
            // 通道任务退出后 `recv()` 会立刻返回 `None`；不把它关掉就会空转。
            item = forwarded.recv(), if events_client.is_some() => {
                match item {
                    Some(Forwarded::Approval(request)) => {
                        send(Event::Approval { request });
                    }
                    Some(Forwarded::Waterfall { request_id, event }) => {
                        // 不是审批的 waterfall 一律交回 Host 的链。不回答等于把它短路掉。
                        match &events_client {
                            Some(client_id) => {
                                if let Err(error) = client
                                    .answer_event(client_id, &request_id, events::Answer::Next)
                                    .await
                                {
                                    send(Event::Notice {
                                        level: NoticeLevel::Warning,
                                        text: format!("回答 {event} 失败：{error}"),
                                    });
                                }
                            }
                            None => send(Event::Notice {
                                level: NoticeLevel::Warning,
                                text: format!("收到 {event} 但没有审批通道可用"),
                            }),
                        }
                    }
                    Some(Forwarded::Cancelled { request_id }) => send(Event::ApprovalClosed {
                        request_id,
                        reason: "内核撤回了这条审批（这一轮被取消或已经结束）".into(),
                    }),
                    Some(Forwarded::Emit { .. } | Forwarded::Ignored) => {}
                    None => events_client = None,
                    Some(Forwarded::Ended(reason)) => {
                        events_client = None;
                        send(Event::Notice {
                            level: NoticeLevel::Warning,
                            text: format!(
                                "审批通道已断开：{reason}；交互内核可能已经退出（/reconnect 重新拉起）"
                            ),
                        });
                    }
                }
                continue;
            }
        };
        let Some(command) = command else { break };
        match command {
            Command::Shutdown => break,
            Command::Hello { .. } | Command::Unknown => {}
            Command::AnswerApproval { request_id, outcome } => {
                match &events_client {
                    Some(client_id) => {
                        let answer = events::outcome_value(outcome);
                        match client.answer_event(client_id, &request_id, answer).await {
                            Ok(()) => send(Event::Notice {
                                level: NoticeLevel::Info,
                                text: if answer == events::Answer::AllowOnce {
                                    "已允许这一次".into()
                                } else {
                                    "已拒绝".into()
                                },
                            }),
                            Err(error) => send(Event::Notice {
                                level: NoticeLevel::Error,
                                text: format!("审批应答没有送达：{error}"),
                            }),
                        }
                    }
                    None => send(Event::Notice {
                        level: NoticeLevel::Error,
                        text: "审批通道已断开，这条应答没有去处".into(),
                    }),
                }
            }
            Command::SearchSessions { query } => match client.search(&query).await {
                Ok(items) => send(Event::Search { query, items }),
                Err(error) => send(Event::Notice {
                    level: NoticeLevel::Error,
                    text: format!("检索失败：{error}"),
                }),
            },
            Command::ListSessions => match client.list().await {
                Ok(sessions) => {
                    known = sessions.iter().map(|s| (s.session_id.clone(), s.clone())).collect();
                    send(Event::Sessions { sessions });
                }
                Err(error) => send(Event::Notice { level: NoticeLevel::Error, text: format!("读会话列表失败：{error}") }),
            },
            Command::Catalog => match client.model_catalog().await {
                Ok(catalog) => send(Event::Catalog { catalog }),
                Err(error) => send(Event::Notice { level: NoticeLevel::Error, text: format!("读模型目录失败：{error}") }),
            },
            Command::OpenSession { session_id } => {
                close_pump(&mut pumps, &session_id).await;
                // 前端只给 id；子 agent 会话要父地址，地址从上一次列表里查（协议不变 ——
                // GUI 与 TUI 都不需要知道地址有两种形状）。没见过这个 id 就先刷新一次列表。
                if !known.contains_key(&session_id)
                    && let Ok(sessions) = client.list().await
                {
                    known = sessions.into_iter().map(|s| (s.session_id.clone(), s)).collect();
                }
                let address = session::address(&session_id, known.get(&session_id));
                if let Err(text) = open_pump(&client, &session_id, address, None, &events, &mut pumps).await {
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
                // follow 在取消/断流后可能已结束；HashMap 里的句柄存在不等于仍有读者。
                // 重开订阅后再发，避免请求已经被内核执行而前端永远等不到更新。
                if pumps.get(&session_id).is_some_and(|pump| pump.task.is_finished()) {
                    close_pump(&mut pumps, &session_id).await;
                }
                if !pumps.contains_key(&session_id) {
                    // 先选模型再打开：打开时的快照里就是选好的那条，界面不会先闪一下出厂默认。
                    if let Some(choice) = &wanted {
                        if let Err(error) = client.select_model(&session_id, choice).await {
                            send(Event::Notice { level: NoticeLevel::Error, text: format!("选模型失败：{error}") });
                            continue;
                        }
                    }
                    let address = session::address(&session_id, known.get(&session_id));
                    if let Err(text) = open_pump(&client, &session_id, address, wanted.clone(), &events, &mut pumps).await {
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
    } else if let Some(lock) = &lock {
        lock.kernel_exited();
    }
    drop(lock);
    send(Event::Backend { state: BackendState::Stopped });
}

async fn open_pump(
    client: &Arc<SessionClient>,
    session_id: &str,
    address: serde_json::Value,
    model: Option<ModelChoice>,
    events: &mpsc::UnboundedSender<Event>,
    pumps: &mut HashMap<String, Pump>,
) -> Result<(), String> {
    let mut open = client.open_at(address).await.map_err(|e| format!("打开会话失败：{e}"))?;
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
