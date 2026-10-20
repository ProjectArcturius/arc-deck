//! 真内核 + 本地假模型：取消正在执行的工具后立刻续聊，不消耗配额。
use std::time::Duration;

use arc_deck_core::{
    authz::Permission,
    conversation::{Block, Message, ModelChoice, ToolStatus},
    mode::{AgentId, Mode},
    service::{BackendState, Command, Event, NoticeLevel},
    userconfig::UserConfig,
};
use serde_json::json;

use super::{launch, service};

#[tokio::test]
#[ignore = "需要本机钉住的 DSH 与 Node；假模型端点，零配额"]
async fn remote_live_cancel_tool_then_immediately_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let shell = if cfg!(windows) { "pwsh" } else { "bash" };
    let command = if cfg!(windows) {
        "Start-Sleep -Seconds 60"
    } else {
        "sleep 60"
    };
    let provider = crate::fake_provider::Provider::start(
        crate::fake_provider::Script::from_value(&json!({
            "responses": [{"blocks": [{"tool": shell, "arguments": {"command": command}}]}],
            "default": {"blocks": [{"text": "RECOVERED"}]}
        }))
        .unwrap(),
        dir.path().join("requests.jsonl"),
    )
    .unwrap();
    let key_path = dir.path().join("fake-key");
    std::fs::write(&key_path, "local-test-key").unwrap();
    let mut cfg: UserConfig = serde_json::from_value(json!({
        "model": "local-test/test-model",
        "provider": {"local-test": {
            "api": "anthropic-messages",
            "options": {"baseURL": provider.url(), "apiKey": format!("{{file:{}}}", key_path.display())},
            "models": {"test-model": {"contextWindow": 32768}}
        }}
    })).unwrap();
    if let Some(path) = std::env::var_os("ARCDECK_DSH_RUNTIME_DIR") {
        cfg.runtime.dir = Some(path.into());
    }
    let home = dir.path().join("home");
    let launch = launch::interactive(launch::InteractiveInput {
        config: &cfg,
        workspace: &workspace,
        permission: Permission::Full,
        agent: AgentId::Echo,
        mode: Mode::Build,
        dsh_home: &home,
        persona_suffix: "",
        kernel_id: "cancel-regression",
    })
    .expect("需要已安装的钉住运行时");
    let model = ModelChoice {
        provider: launch.provider,
        model: launch.model,
        effort: None,
    };
    let mut handle = service::spawn(service::ServiceInput {
        options: launch.options,
        default_model: Some(model.clone()),
        workspace: workspace.clone(),
        lock: Some(launch.lock),
    });
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = handle.events.recv().await {
            if matches!(event, Event::Backend { state: BackendState::Ready { .. } }) { break }
            if let Event::Backend { state: BackendState::Failed { reason } } = event { panic!("{reason}") }
        }
        handle.commands.send(Command::Prompt { session_id: None, text: "run the tool".into(), model: Some(model.clone()) }).unwrap();
        let mut session = None;
        while let Some(event) = handle.events.recv().await {
            match event {
                Event::SessionStarted { session_id } => session = Some(session_id),
                Event::Conversation { conversation } if conversation.messages.iter().any(|m| matches!(m,
                    Message::Assistant(a) if a.blocks.iter().any(|b| matches!(b, Block::Tool(t) if t.status == ToolStatus::Running)))) => break,
                Event::Notice { level: NoticeLevel::Error, text } => panic!("{text}"),
                _ => {}
            }
        }
        let id = session.expect("新会话 id");
        tokio::time::sleep(Duration::from_millis(300)).await;
        // 不等 turn/end：用户可以在工具中止期间立即输入下一轮。
        handle.commands.send(Command::Cancel { session_id: id.clone() }).unwrap();
        handle.commands.send(Command::Prompt { session_id: Some(id.clone()), text: "continue".into(), model: Some(model.clone()) }).unwrap();
        while let Some(event) = handle.events.recv().await {
            match event {
                Event::Conversation { conversation } if !conversation.running && conversation.messages.iter().any(|m| matches!(m,
                    Message::Assistant(a) if a.blocks.iter().any(|b| matches!(b, Block::Text(t) if t.text == "RECOVERED")))) => return id,
                Event::Notice { level: NoticeLevel::Error, text } => panic!("{text}"),
                _ => {}
            }
        }
        panic!("服务在回答前关闭");
    }).await;
    handle.commands.send(Command::Shutdown).unwrap();
    tokio::time::timeout(Duration::from_secs(25), handle.task)
        .await
        .unwrap()
        .unwrap();
    let id = result.expect("取消工具后下一轮卡住");

    // 同一个前端进程重启内核（切授权/个性化 Agent、/reconnect）后还要能继续。
    // 1.0 前 SessionClient 的局部计数器重置，DSH 对 requestId 去重后什么都不做。
    let launch = launch::interactive(launch::InteractiveInput {
        config: &cfg,
        workspace: &workspace,
        permission: Permission::Full,
        agent: AgentId::Echo,
        mode: Mode::Build,
        dsh_home: &home,
        persona_suffix: "",
        kernel_id: "cancel-regression-reconnect",
    })
    .unwrap();
    let mut handle = service::spawn(service::ServiceInput {
        options: launch.options,
        default_model: Some(model.clone()),
        workspace,
        lock: Some(launch.lock),
    });
    handle
        .commands
        .send(Command::OpenSession {
            session_id: id.clone(),
        })
        .unwrap();
    handle
        .commands
        .send(Command::Prompt {
            session_id: Some(id),
            text: "after reconnect".into(),
            model: Some(model),
        })
        .unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(45), async {
        while let Some(event) = handle.events.recv().await {
            match event {
                Event::Conversation { conversation } if !conversation.running &&
                    conversation.messages.iter().filter(|m| matches!(m, Message::User(_))).count() >= 3 &&
                    conversation.messages.iter().filter(|m| matches!(m, Message::Assistant(a) if a.blocks.iter().any(|b| matches!(b, Block::Text(t) if t.text == "RECOVERED")))).count() >= 2 => return,
                Event::Notice { level: NoticeLevel::Error, text } => panic!("{text}"),
                _ => {}
            }
        }
        panic!("重连服务在回答前关闭");
    }).await;
    handle.commands.send(Command::Shutdown).unwrap();
    tokio::time::timeout(Duration::from_secs(25), handle.task)
        .await
        .unwrap()
        .unwrap();
    assert!(resumed.is_ok(), "重连后新 prompt 被去重或没有收到回答");
}
