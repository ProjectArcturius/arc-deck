//! `arcdeck serve --stdio`：把会话服务的消息（`arc_deck_core::service`）按行搬到 stdin / stdout。
//!
//! 这是 GUI「热插拔」的第一种接法（docs/GUI_PLUGGABLE.md §3）：GUI 进程拉起 `arcdeck serve --stdio`
//! 作为子进程，一行一条 JSON。选它先做的理由：
//!
//! - **不需要发现与鉴权**：管道只连着拉起它的那个进程，没有端口、没有 socket 权限要设。
//! - **生命周期天然收口**：GUI 关掉 → stdin EOF → 服务收内核、放锁、退出。不会留孤儿。
//! - **与 TUI 同一份后端代码**：TUI 在进程内用 `remote::service`，这里只多一层按行编解码。
//!
//! 线规则：
//! - 第一行必须是 `Hello`，主版本不同回 `Welcome{accepted:false}` 并以 64 退出（谁该升级写在 reason 里）。
//! - 之后每行一条 `Command`；坏行回一条 `Notice{error}`，**不断开** —— 一行写坏不该让 GUI 丢掉会话。
//! - stdout 只写协议帧；人看的诊断走 stderr。

use std::path::{Path, PathBuf};

use arc_deck_core::authz::Permission;
use arc_deck_core::conversation::ModelChoice;
use arc_deck_core::error::{code, SpecError};
use arc_deck_core::service::{self, BackendState, Command, Event, NoticeLevel};
use arc_deck_core::userconfig::UserConfig;
use arc_deck_dsh::remote::{launch, service as session_service};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// 按用户配置起会话服务（TUI 与 `serve` 共用）。失败返回给人看的原因。
pub(crate) fn spawn_service(
    cfg: &UserConfig,
    workspace: &Path,
    permission: Permission,
    kernel_id: &str,
) -> Result<(session_service::ServiceHandle, Vec<String>), String> {
    let home = launch::default_dsh_home();
    let launch = launch::interactive(launch::InteractiveInput {
        config: cfg,
        workspace,
        permission,
        dsh_home: &home,
        // 个性化（§5.3.1）还没接进交互面：关着时本来就一段都不注入，开着时这里**尚未**注入 ——
        // 如实记在 docs/M4_PLAN.md，不假装生效。
        persona_suffix: "",
        kernel_id,
    })
    .map_err(|error| error.message)?;
    let notes = launch.guard_notes.clone();
    let handle = session_service::spawn(session_service::ServiceInput {
        options: launch.options,
        default_model: Some(ModelChoice { provider: launch.provider, model: launch.model, effort: launch.effort }),
        workspace: workspace.to_path_buf(),
        lock: Some(launch.lock),
    });
    Ok((handle, notes))
}

/// 一行输入的判定。单独成函数是为了离线可测：编解码与握手不需要内核。
#[derive(Debug, PartialEq)]
pub(crate) enum Line {
    Command(Command),
    /// 坏行：回给对端的那条通知。
    Bad(Event),
    Blank,
}

pub(crate) fn decode(line: &str) -> Line {
    let line = line.trim();
    if line.is_empty() {
        return Line::Blank;
    }
    match serde_json::from_str::<Command>(line) {
        Ok(command) => Line::Command(command),
        Err(error) => Line::Bad(Event::Notice {
            level: NoticeLevel::Error,
            text: format!("这一行不是合法的命令（{error}）；连接保持"),
        }),
    }
}

/// 握手：第一行必须是 `Hello`。返回要写回的 `Welcome` 与是否接受。
pub(crate) fn handshake(first: &Line) -> (Event, bool) {
    let backend = env!("CARGO_PKG_VERSION");
    match first {
        Line::Command(Command::Hello { protocol, .. }) => {
            let welcome = service::negotiate(*protocol, backend);
            let accepted = matches!(welcome, Event::Welcome { accepted: true, .. });
            (welcome, accepted)
        }
        _ => (
            Event::Welcome {
                protocol: service::PROTOCOL_VERSION,
                accepted: false,
                reason: Some("第一行必须是 {\"op\":\"hello\",\"protocol\":N,\"client\":\"…\"}".into()),
                backend: backend.into(),
            },
            false,
        ),
    }
}

fn frame(event: &Event) -> String {
    let mut line = serde_json::to_string(event).unwrap_or_else(|_| "{\"event\":\"unknown\"}".into());
    line.push('\n');
    line
}

pub(crate) async fn cmd_serve(argv: &[String]) -> Result<i32, SpecError> {
    let mut stdio = false;
    let mut permission = Permission::Workspace;
    let mut workspace: Option<PathBuf> = None;
    let mut rest = argv.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--stdio" => stdio = true,
            "--permission" => {
                let raw = rest.next().ok_or_else(|| SpecError::new(code::CONFIG_INVALID, "--permission 缺取值"))?;
                permission = Permission::parse(raw)?;
            }
            "--workspace" => {
                workspace = Some(PathBuf::from(
                    rest.next().ok_or_else(|| SpecError::new(code::CONFIG_INVALID, "--workspace 缺取值"))?,
                ))
            }
            other => return Err(SpecError::new(code::CONFIG_INVALID, format!("serve 不认识 `{other}`"))),
        }
    }
    if !stdio {
        // 共享 socket 的那种（多个前端同时连一个后端）是设计里的第二步，还没实现 —— 明说，不假装。
        return Err(SpecError::new(
            code::CONFIG_INVALID,
            "目前只有 `arcdeck serve --stdio`（GUI 拉起子进程、按行收发）；共享 socket 模式见 docs/GUI_PLUGGABLE.md，尚未实现",
        ));
    }
    let workspace = match workspace {
        Some(path) => arc_deck_core::paths::absolutize(&path),
        None => std::env::current_dir().map_err(|e| SpecError::new(code::CONFIG_INVALID, e.to_string()))?,
    };

    let mut stdout = tokio::io::stdout();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let first = loop {
        match lines.next_line().await {
            Ok(Some(line)) => match decode(&line) {
                Line::Blank => continue,
                other => break other,
            },
            _ => return Ok(0),
        }
    };
    let (welcome, accepted) = handshake(&first);
    let _ = stdout.write_all(frame(&welcome).as_bytes()).await;
    let _ = stdout.flush().await;
    if !accepted {
        return Ok(64);
    }

    let (cfg, _) = match UserConfig::locate_and_load(None) {
        Ok(found) => found,
        Err(error) => {
            let failed = Event::Backend { state: BackendState::Failed { reason: error.message.clone() } };
            let _ = stdout.write_all(frame(&failed).as_bytes()).await;
            return Ok(64);
        }
    };
    let kernel_id = format!("serve-{}", std::process::id());
    let (handle, notes) = match spawn_service(&cfg, &workspace, permission, &kernel_id) {
        Ok(started) => started,
        Err(reason) => {
            let failed = Event::Backend { state: BackendState::Failed { reason } };
            let _ = stdout.write_all(frame(&failed).as_bytes()).await;
            let _ = stdout.flush().await;
            return Ok(64);
        }
    };
    for note in notes {
        let text = format!("授权缺口（已留档）：{note}");
        let _ = stdout.write_all(frame(&Event::Notice { level: NoticeLevel::Warning, text }).as_bytes()).await;
    }
    let _ = stdout.flush().await;

    // stdout 只有一个写端：服务的事件与坏行的通知都经它出去，帧不会交错成半行。
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let writer = tokio::spawn(async move {
        while let Some(event) = out_rx.recv().await {
            if stdout.write_all(frame(&event).as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let forward_tx = out_tx.clone();
    let mut events = handle.events;
    let forward = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if forward_tx.send(event).is_err() {
                break;
            }
        }
    });
    let commands = handle.commands.clone();
    // stdin EOF（或读错）= 前端走了：收掉后端（stdio 模式下后端只属于它）。
    while let Ok(Some(line)) = lines.next_line().await {
        match decode(&line) {
            Line::Blank | Line::Command(Command::Hello { .. }) => {}
            Line::Command(Command::Shutdown) => break,
            Line::Command(command) => {
                let _ = commands.send(command);
            }
            Line::Bad(notice) => {
                let _ = out_tx.send(notice);
            }
        }
    }
    let _ = commands.send(Command::Shutdown);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(20), handle.task).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), forward).await;
    drop(out_tx);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), writer).await;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_line_must_be_a_compatible_hello() {
        let hello = decode(&format!(r#"{{"op":"hello","protocol":{},"client":"uwp"}}"#, service::PROTOCOL_VERSION));
        assert!(handshake(&hello).1);
        let newer = decode(r#"{"op":"hello","protocol":99,"client":"uwp"}"#);
        let (welcome, accepted) = handshake(&newer);
        assert!(!accepted);
        assert!(matches!(welcome, Event::Welcome { reason: Some(r), .. } if r.contains("请升级后端")));
        assert!(!handshake(&decode(r#"{"op":"list_sessions"}"#)).1, "没握手就发命令要被拒");
    }

    #[test]
    fn a_bad_line_is_reported_and_an_unknown_op_is_tolerated() {
        assert!(matches!(decode("{not json"), Line::Bad(Event::Notice { level: NoticeLevel::Error, .. })));
        assert_eq!(decode(r#"{"op":"from_the_future","x":1}"#), Line::Command(Command::Unknown));
        assert_eq!(decode("   "), Line::Blank);
    }

    #[test]
    fn frames_are_single_lines_even_with_newlines_in_the_text() {
        let line = frame(&Event::Notice { level: NoticeLevel::Info, text: "第一行\n第二行".into() });
        assert_eq!(line.matches('\n').count(), 1, "帧内的换行必须被转义：{line}");
        assert!(line.ends_with('\n'));
    }
}
