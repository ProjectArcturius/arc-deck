//! **会消耗模型配额**的 live 测试（`#[ignore]`，名字以 `remote_quota_` 开头）。
//!
//! 与 `live.rs` 的分工：那边不发 prompt、不要密钥；这边按**用户的真实配置**起交互内核
//! （`launch::interactive`，与 TUI 同一条路），发**一条极短**的消息，把线上来的帧原样落盘 ——
//! 离线夹具的形状来自这里，而不是来自我们对源码的理解（P9：论证不等于证据）。
//!
//! 用户已允许少量用 `opencode-go/deepseek-v4.1-flash` 测试。`DSH_HOME` 是临时目录（P15）。
//!
//! ```text
//! ARCDECK_CAPTURE=<文件> cargo test -p arc-deck-dsh --offline --lib -- --ignored remote_quota_ --nocapture
//! ```

#![cfg(test)]

use std::io::Write as _;
use std::time::{Duration, Instant};

use arc_deck_core::authz::Permission;
use arc_deck_core::userconfig::UserConfig;
use serde_json::{json, Value};

use super::launch::{interactive, InteractiveInput};
use super::{RemoteFrame, RemoteMux, RemoteRpc, WebKernel};

#[tokio::test]
#[ignore = "会消耗一次极小的模型配额；需要用户配置、密钥与已安装的运行时"]
async fn remote_quota_one_turn_is_captured_end_to_end() {
    let (config, _) = match UserConfig::locate_and_load(None) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("跳过：没有可用的用户配置（{}）", error.message);
            return;
        }
    };
    let home = tempfile::Builder::new().prefix("arcdeck-quota-").tempdir().unwrap();
    let workspace = tempfile::Builder::new().prefix("arcdeck-quota-ws-").tempdir().unwrap();
    let mut launch = interactive(InteractiveInput {
        config: &config,
        workspace: workspace.path(),
        permission: Permission::Readonly,
        dsh_home: home.path(),
        persona_suffix: "",
        kernel_id: "arcdeck-quota-capture",
    })
    .expect("交互内核的启动参数");
    launch.options.startup_timeout = Duration::from_secs(180);
    let mut kernel = WebKernel::start(launch.options).await.expect("起 web 内核");

    let rpc = RemoteRpc::for_kernel(&kernel).unwrap();
    let mux = RemoteMux::connect(&kernel).await.unwrap();
    let mut capture: Vec<Value> = Vec::new();

    let catalog = rpc.call("session/modelCatalog", json!({})).await;
    capture.push(json!({ "call": "session/modelCatalog", "result": format!("{catalog:?}") }));
    if let Ok(value) = &catalog {
        capture.push(json!({ "call": "session/modelCatalog", "value": value }));
    }

    let created = rpc
        .call(
            "session/create",
            json!({ "request": { "cwd": workspace.path().display().to_string() } }),
        )
        .await
        .expect("session/create");
    capture.push(json!({ "call": "session/create", "value": created }));
    let session_id = created["sessionId"].as_str().expect("sessionId").to_string();

    let selected = rpc
        .call(
            "session/selectModel",
            json!({ "request": {
                "sessionId": session_id,
                "provider": launch.provider,
                "model": launch.model,
            } }),
        )
        .await;
    capture.push(json!({ "call": "session/selectModel", "result": format!("{selected:?}") }));

    let address = json!({ "kind": "session", "sessionId": session_id });
    let mut follow = mux
        .open("session/follow", json!({ "request": { "address": address, "assistantStream": true } }))
        .await
        .expect("session/follow");

    let prompted = rpc
        .call(
            "session/prompt",
            json!({ "request": {
                "requestId": "arcdeck-capture-1",
                "sessionId": session_id,
                "mode": "queue",
                "content": [{ "type": "text", "text": "只回复两个字：收到" }],
            } }),
        )
        .await;
    capture.push(json!({ "call": "session/prompt", "result": format!("{prompted:?}") }));

    let deadline = Instant::now() + Duration::from_secs(150);
    let mut ended = false;
    while Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_secs(30), follow.next()).await;
        let Ok(Some(frame)) = next else {
            capture.push(json!({ "note": "follow 30 秒无帧或已结束" }));
            break;
        };
        match frame {
            RemoteFrame::Item(value) => {
                let is_turn_end = value["type"] == "event" && value["event"]["type"] == "turn/end";
                capture.push(json!({ "frame": value }));
                if is_turn_end {
                    ended = true;
                    break;
                }
            }
            other => {
                capture.push(json!({ "terminal": format!("{other:?}") }));
                break;
            }
        }
    }
    // turn/end 之后再收一小会儿：end 帧与 durable 事件的先后要看真实顺序。
    while let Ok(Some(RemoteFrame::Item(value))) =
        tokio::time::timeout(Duration::from_millis(1500), follow.next()).await
    {
        capture.push(json!({ "frame": value }));
    }
    follow.close();

    let listed = rpc.call("session/list", json!({ "_request": {} })).await;
    capture.push(json!({ "call": "session/list", "result": listed.as_ref().map_err(|e| e.to_string()).ok() }));
    let renamed = rpc
        .call("session/rename", json!({ "request": { "sessionId": session_id, "title": "抓帧" } }))
        .await;
    capture.push(json!({ "call": "session/rename", "result": format!("{renamed:?}") }));

    if let Ok(path) = std::env::var("ARCDECK_CAPTURE") {
        let mut file = std::fs::File::create(&path).expect("写抓帧文件");
        for line in &capture {
            writeln!(file, "{line}").unwrap();
        }
        eprintln!("抓帧写进 {path}（{} 行）", capture.len());
    }
    let termination = kernel.shutdown().await;
    eprintln!("内核收尾：{}", termination.detail);
    assert!(ended, "150 秒内没有等到 turn/end；抓到 {} 帧", capture.len());
}

/// 起一个按用户配置装好的交互内核（与 TUI 同一条路），临时 home 与临时工作区。
async fn start_configured(
    name: &str,
) -> Option<(WebKernel, super::launch::InteractiveLaunch, tempfile::TempDir, tempfile::TempDir)> {
    let (config, _) = match UserConfig::locate_and_load(None) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("跳过 {name}：没有可用的用户配置（{}）", error.message);
            return None;
        }
    };
    let home = tempfile::Builder::new().prefix("arcdeck-live-").tempdir().unwrap();
    let workspace = tempfile::Builder::new().prefix("arcdeck-live-ws-").tempdir().unwrap();
    let mut launch = interactive(InteractiveInput {
        config: &config,
        workspace: workspace.path(),
        permission: Permission::Readonly,
        dsh_home: home.path(),
        persona_suffix: "",
        kernel_id: name,
    })
    .expect("交互内核的启动参数");
    let mut options = std::mem::replace(
        &mut launch.options,
        super::host::WebKernelOptions::new("node", "bin.js", home.path()),
    );
    options.startup_timeout = Duration::from_secs(180);
    let kernel = WebKernel::start(options).await.expect("起 web 内核");
    Some((kernel, launch, home, workspace))
}

/// 会话操作走一遍，**不发 prompt、不耗配额**：目录、新建、打开空会话、改名、列表。
#[tokio::test]
#[ignore = "起真内核；需要用户配置、密钥与已安装的运行时（不发 prompt）"]
async fn remote_live_session_operations_without_a_prompt() {
    use super::session::SessionClient;
    let Some((mut kernel, launch, _home, workspace)) =
        start_configured("remote_live_session_operations_without_a_prompt").await
    else {
        return;
    };
    let client = SessionClient::connect(&kernel).await.unwrap();

    let catalog = client.model_catalog().await.expect("模型目录");
    assert!(
        catalog.providers.iter().any(|p| p.id == launch.provider && p.models.iter().any(|m| m.id == launch.model)),
        "配置的路由 {}/{} 不在内核报告的目录里：{:?}",
        launch.provider,
        launch.model,
        catalog.providers.iter().map(|p| &p.id).collect::<Vec<_>>()
    );

    let id = client.create(workspace.path()).await.expect("新建");
    let selected = client
        .select_model(&id, &arc_deck_core::conversation::ModelChoice {
            provider: launch.provider.clone(),
            model: launch.model.clone(),
            effort: None,
        })
        .await
        .expect("选模型");
    assert_eq!(selected.model, launch.model);

    let mut open = client.open(&id).await.expect("打开空会话");
    assert!(open.view().messages.is_empty());
    assert!(open.view().history_complete);
    assert_eq!(open.view().model.as_ref().map(|m| m.model.as_str()), Some(launch.model.as_str()));

    let title = client.rename(&id, "改过的名字").await.expect("改名");
    assert_eq!(title, "改过的名字");
    let list = client.list().await.expect("列表");
    let row = list.iter().find(|s| s.session_id == id).expect("新会话在列表里");
    assert_eq!(row.title.as_deref(), Some("改过的名字"));
    open.close();
    let termination = kernel.shutdown().await;
    eprintln!("内核收尾：{}", termination.detail);
}

/// 产品路径上的一轮：`SessionClient` 发一条极短的消息，视图里长出一问一答。**耗配额。**
#[tokio::test]
#[ignore = "会消耗一次极小的模型配额"]
async fn remote_quota_one_turn_through_the_session_client() {
    use super::session::{SessionClient, SessionUpdate};
    use arc_deck_core::conversation::{Message, TurnState};
    let Some((mut kernel, launch, _home, workspace)) =
        start_configured("remote_quota_one_turn_through_the_session_client").await
    else {
        return;
    };
    let client = SessionClient::connect(&kernel).await.unwrap();
    let id = client.create(workspace.path()).await.unwrap();
    client
        .select_model(&id, &arc_deck_core::conversation::ModelChoice {
            provider: launch.provider.clone(),
            model: launch.model.clone(),
            effort: None,
        })
        .await
        .unwrap();
    let mut open = client.open(&id).await.unwrap();
    // 只看「这一轮」新增的部分（P16 第三条）：发之前记下条数。
    let before = open.view().messages.len();
    client.prompt(&id, "只回复两个字：收到").await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(150);
    let mut saw_streaming = false;
    loop {
        assert!(Instant::now() < deadline, "150 秒内没有答完");
        match tokio::time::timeout(Duration::from_secs(60), open.next()).await.expect("60 秒没有帧") {
            SessionUpdate::Changed => {
                let view = open.view();
                if let Some(Message::Assistant(turn)) = view.messages.last() {
                    if turn.blocks.iter().any(|b| matches!(b,
                        arc_deck_core::conversation::Block::Text(p) if p.streaming)) {
                        saw_streaming = true;
                    }
                    if turn.state.is_terminal() {
                        break;
                    }
                }
            }
            SessionUpdate::Unchanged => {}
            SessionUpdate::Ended(reason) => panic!("follow 提前结束：{reason}"),
        }
    }
    let new: Vec<&Message> = open.view().messages[before..].iter().collect();
    assert_eq!(new.len(), 2, "{new:#?}");
    let Message::Assistant(turn) = new[1] else { panic!("{new:#?}") };
    assert_eq!(turn.state, TurnState::Completed, "{:?}", turn.state);
    assert!(!turn.text().trim().is_empty());
    eprintln!("回答：{:?}（见过流式块：{saw_streaming}）", turn.text());
    open.close();
    kernel.shutdown().await;
}
